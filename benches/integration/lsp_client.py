#!/usr/bin/env python3
"""
Minimal LSP client for benchmarking php-lsp.

Sends Content-Length-framed JSON-RPC messages over stdin/stdout.
Outputs one JSON line per request: {"method": "...", "latency_ms": ...}

Usage:
    python3 lsp_client.py \\
        --binary /path/to/php-lsp \\
        --fixture /path/to/file.php \\
        --requests 100 \\
        --output results.jsonl \\
        --lsp-method hover        # or: definition, references
"""

import argparse
import json
import os
import select
import subprocess
import sys
import threading
import time


# ── Framing helpers ────────────────────────────────────────────────────────────

def encode_message(obj: dict) -> bytes:
    body = json.dumps(obj).encode("utf-8")
    header = f"Content-Length: {len(body)}\r\n\r\n".encode("ascii")
    return header + body


def read_message(proc) -> dict:
    """Read one Content-Length-framed message from proc.stdout."""
    headers = {}
    while True:
        line = proc.stdout.readline()
        if not line:
            raise EOFError("Server closed stdout")
        line = line.rstrip(b"\r\n")
        if line == b"":
            break
        key, _, value = line.partition(b":")
        headers[key.strip().lower()] = value.strip()

    length = int(headers[b"content-length"])
    body = b""
    while len(body) < length:
        chunk = proc.stdout.read(length - len(body))
        if not chunk:
            raise EOFError("Server closed stdout mid-message")
        body += chunk
    return json.loads(body.decode("utf-8"))


# ── Request builders ───────────────────────────────────────────────────────────

_id_counter = 0


def next_id() -> int:
    global _id_counter
    _id_counter += 1
    return _id_counter


def req_initialize(root_uri: str, init_options: dict | None = None) -> dict:
    return {
        "jsonrpc": "2.0",
        "id": next_id(),
        "method": "initialize",
        "params": {
            "processId": os.getpid(),
            "rootUri": root_uri,
            "capabilities": {
                # Advertise progress support so the server sends $/progress
                # notifications when the workspace index begins and ends.
                "window": {"workDoneProgress": True},
                "workspace": {
                    "configuration": True,
                    "didChangeConfiguration": {"dynamicRegistration": False},
                },
            },
            "initializationOptions": init_options or {},
        },
    }


def notif_did_change(uri: str, version: int, text: str) -> dict:
    """FULL sync: resend the whole document text on each change."""
    return {
        "jsonrpc": "2.0",
        "method": "textDocument/didChange",
        "params": {
            "textDocument": {"uri": uri, "version": version},
            "contentChanges": [{"text": text}],
        },
    }


def notif_initialized() -> dict:
    return {"jsonrpc": "2.0", "method": "initialized", "params": {}}


def req_shutdown() -> dict:
    return {"jsonrpc": "2.0", "id": next_id(), "method": "shutdown", "params": None}


def notif_exit() -> dict:
    return {"jsonrpc": "2.0", "method": "exit", "params": None}


def req_did_open(uri: str, text: str) -> dict:
    return {
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "php",
                "version": 1,
                "text": text,
            }
        },
    }


def req_hover(req_id: int, uri: str, line: int, character: int) -> dict:
    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "textDocument/hover",
        "params": {
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        },
    }


def req_definition(req_id: int, uri: str, line: int, character: int) -> dict:
    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "textDocument/definition",
        "params": {
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        },
    }


def req_references(req_id: int, uri: str, line: int, character: int) -> dict:
    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "textDocument/references",
        "params": {
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "context": {"includeDeclaration": False},
        },
    }


# ── Index-complete detection ───────────────────────────────────────────────────

def _answer_reverse_request(proc, msg: dict, init_options: dict | None = None) -> bool:
    """
    Answer server→client requests so the server keeps making progress.

    A server that advertises support for `workspace/configuration`,
    `window.workDoneProgress`, or dynamic capability registration may
    send these *requests* (with an `id`) to the client. If the client
    ignores them, the server blocks forever waiting for the reply — this
    was the root cause of the integration bench's 130 s wall time on
    Laravel.

    Returns True if the message was a reverse request and has been answered.
    """
    method = msg.get("method", "")
    if "id" not in msg:
        return False
    if method == "workspace/configuration":
        # Reply with the init_options for each requested section, or null.
        items = msg.get("params", {}).get("items", [])
        reply = []
        for it in items:
            sect = it.get("section", "")
            if init_options and sect in ("", "php-lsp", "phplsp", "php"):
                reply.append(init_options)
            else:
                reply.append(None)
        proc.stdin.write(encode_message({
            "jsonrpc": "2.0", "id": msg["id"], "result": reply,
        }))
        proc.stdin.flush()
        return True
    if method in (
        "client/registerCapability",
        "client/unregisterCapability",
        "window/workDoneProgress/create",
        "workspace/semanticTokens/refresh",
        "workspace/diagnostic/refresh",
        "workspace/inlayHint/refresh",
        "workspace/codeLens/refresh",
    ):
        proc.stdin.write(encode_message({
            "jsonrpc": "2.0", "id": msg["id"], "result": None,
        }))
        proc.stdin.flush()
        return True
    return False


def _drain_until_indexed(proc, index_wait: float, rss_samples: list, init_options: dict | None = None) -> None:
    """
    Read server notifications in a background thread until either:
      - a $/progress end-of-work-done notification is received, or
      - `index_wait` seconds have elapsed (safety fallback).

    While waiting, a second thread samples RSS every 500 ms and appends
    ``{"time_ms": ..., "rss_kb": ..., "event": "sample"}`` dicts to
    ``rss_samples``.  The caller can inspect this list for peak RSS.

    php-lsp sends ``$/progress`` with ``kind == "end"`` when the
    workspace scan finishes.  Advertising ``window.workDoneProgress``
    capability in ``initialize`` is required for the server to emit these.
    """
    deadline = time.monotonic() + index_wait
    done = threading.Event()
    t0 = time.monotonic()

    def _sampler():
        while not done.is_set():
            kb = rss_kb(proc.pid)
            if kb is not None:
                rss_samples.append({
                    "time_ms": round((time.monotonic() - t0) * 1000.0, 1),
                    "rss_kb": kb,
                    "event": "sample",
                })
            done.wait(timeout=0.5)

    def _reader():
        while not done.is_set() and time.monotonic() < deadline:
            # Use select so the loop stays responsive to the `done` flag
            # even when the server is quiet (pipe has no socket timeout).
            ready, _, _ = select.select([proc.stdout], [], [], 0.1)
            if not ready:
                continue
            try:
                msg = read_message(proc)
            except (EOFError, OSError, ValueError):
                break
            # Answer reverse requests so the server doesn't block.
            if _answer_reverse_request(proc, msg, init_options):
                continue
            method = msg.get("method", "")
            if method == "$/progress":
                kind = (
                    msg.get("params", {})
                    .get("value", {})
                    .get("kind", "")
                )
                if kind == "end":
                    done.set()
                    break

    sampler = threading.Thread(target=_sampler, daemon=True)
    sampler.start()
    t = threading.Thread(target=_reader, daemon=True)
    t.start()
    # Wait for the end signal or the deadline, whichever comes first.
    done.wait(timeout=index_wait)
    done.set()  # signal threads to stop
    t.join(timeout=1.0)
    sampler.join(timeout=1.0)


# ── RSS helper ─────────────────────────────────────────────────────────────────

def rss_kb(pid: int) -> int | None:
    """Return RSS in KB for the given PID, or None if unavailable."""
    try:
        # Linux
        with open(f"/proc/{pid}/status") as fh:
            for line in fh:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1])
    except FileNotFoundError:
        pass
    try:
        # macOS — use ps for a live RSS reading.
        import subprocess as sp
        result = sp.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True)
        if result.returncode == 0 and result.stdout.strip():
            return int(result.stdout.strip())
    except Exception:
        pass
    return None


# ── Main ───────────────────────────────────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser(description="Minimal LSP benchmark client")
    parser.add_argument("--binary", required=True, help="Path to php-lsp binary")
    parser.add_argument("--fixture", required=True, help="Path to PHP fixture file")
    parser.add_argument("--requests", type=int, default=100, help="Number of requests to send")
    parser.add_argument("--output", default="-", help="Output JSONL file (- for stdout)")
    parser.add_argument(
        "--index-wait",
        type=float,
        default=10.0,
        help=(
            "Maximum seconds to wait for the workspace index to finish before "
            "sampling RSS.  The client polls for $/progress end notifications "
            "and stops early when the server signals completion (default: 10)"
        ),
    )
    parser.add_argument(
        "--lsp-method",
        choices=["hover", "definition", "references", "diagnostics"],
        default="hover",
        help=(
            "LSP request method to benchmark (default: hover). "
            "`diagnostics` measures time-to-first + time-to-last publishDiagnostics "
            "after each simulated edit (requires semantic diagnostics enabled "
            "via --init-options)."
        ),
    )
    parser.add_argument(
        "--init-options",
        default=None,
        help=(
            "JSON string passed as initializationOptions. Use "
            '\'{\"diagnostics\": {\"enabled\": true}}\' to exercise the mir-analyzer '
            "pipeline in the diagnostics mode."
        ),
    )
    parser.add_argument(
        "--settle-ms",
        type=int,
        default=1500,
        help=(
            "Diagnostics mode only: quiet-window after the last publishDiagnostics "
            "before concluding a pass is complete (default: 1500ms)"
        ),
    )
    parser.add_argument(
        "--trace-file",
        default=None,
        help="Write server stderr (tracing spans) to this file instead of discarding it",
    )
    args = parser.parse_args()

    init_options: dict | None = None
    if args.init_options:
        init_options = json.loads(args.init_options)

    fixture_path = os.path.abspath(args.fixture)
    fixture_uri = "file://" + fixture_path
    root_uri = "file://" + os.path.dirname(fixture_path)

    with open(fixture_path, encoding="utf-8") as fh:
        fixture_text = fh.read()

    # Pin the position to a known symbol in medium_class.php:
    #   line 110 (LSP line 109), char 19 → `getTitle`.
    # Falls back gracefully if the fixture is shorter than expected.
    lines = fixture_text.splitlines()
    bench_line = min(109, len(lines) - 1)
    bench_char = 19

    lsp_method = args.lsp_method

    def build_request(req_id: int) -> dict:
        if lsp_method == "hover":
            return req_hover(req_id, fixture_uri, bench_line, bench_char)
        elif lsp_method == "definition":
            return req_definition(req_id, fixture_uri, bench_line, bench_char)
        else:
            return req_references(req_id, fixture_uri, bench_line, bench_char)

    # Spawn the server and record wall time immediately.
    spawn_t0 = time.perf_counter()
    if args.trace_file:
        stderr_dest = open(args.trace_file, "wb")  # noqa: SIM115
    else:
        stderr_dest = subprocess.DEVNULL
    proc = subprocess.Popen(
        [args.binary],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=stderr_dest,
        env={**os.environ},
    )

    output_fh = open(args.output, "w", encoding="utf-8") if args.output != "-" else sys.stdout

    def read_response(req_id: int, timeout_s: float = 30.0) -> dict:
        """Read messages until a response with `req_id` arrives, answering
        any reverse requests along the way."""
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            msg = read_message(proc)
            if _answer_reverse_request(proc, msg, init_options):
                continue
            if msg.get("id") == req_id:
                return msg
        raise TimeoutError(f"No response for id={req_id} within {timeout_s}s")

    try:
        # ── Startup: measure time from spawn to initialize response ──────────
        init_msg = req_initialize(root_uri, init_options)
        init_id = init_msg["id"]
        proc.stdin.write(encode_message(init_msg))
        proc.stdin.flush()
        _init_resp = read_response(init_id)
        startup_ms = (time.perf_counter() - spawn_t0) * 1000.0

        output_fh.write(json.dumps({
            "method": "startup",
            "latency_ms": round(startup_ms, 3),
        }) + "\n")
        output_fh.flush()

        proc.stdin.write(encode_message(notif_initialized()))
        proc.stdin.flush()

        # Open the document so the server can index it.
        proc.stdin.write(encode_message(req_did_open(fixture_uri, fixture_text)))
        proc.stdin.flush()

        # ── Wait for the workspace index to finish ────────────────────────────
        # Poll for $/progress end notifications; fall back to index_wait seconds
        # if the server does not emit them.  This avoids measuring RSS before
        # the index is built (which would undercount) while also bounding wait
        # time on workspaces without progress support.
        rss_samples: list = []
        _drain_until_indexed(proc, args.index_wait, rss_samples, init_options)

        rss_post_index = rss_kb(proc.pid)
        peak_rss = max((s["rss_kb"] for s in rss_samples), default=rss_post_index)
        output_fh.write(json.dumps({
            "method": "rss",
            "rss_kb": rss_post_index,
            "peak_rss_kb": peak_rss,
            "samples": len(rss_samples),
        }) + "\n")
        output_fh.flush()

        # Write per-sample timeline so callers can plot memory growth.
        for sample in rss_samples:
            output_fh.write(json.dumps({"method": "rss_sample", **sample}) + "\n")
        output_fh.flush()

        # ── Request latency ───────────────────────────────────────────────────
        if lsp_method == "diagnostics":
            # Edit-loop mode: simulate a keystroke with didChange and measure
            # time to first + time to last publishDiagnostics (settle-ms of
            # quiet = "done"). Requires init_options to enable diagnostics on
            # the server side.
            settle_s = args.settle_ms / 1000.0
            doc_version = 1
            for i in range(args.requests):
                doc_version += 1
                change = notif_did_change(fixture_uri, doc_version, fixture_text)
                t0 = time.perf_counter()
                proc.stdin.write(encode_message(change))
                proc.stdin.flush()

                t_first = None
                t_last = None
                diag_count = 0
                last_msg_t = t0
                # Read until quiet window elapses after the last diagnostics
                # (or we've been waiting > 30s total).
                hard_deadline = t0 + 30.0
                while True:
                    now = time.perf_counter()
                    if now >= hard_deadline:
                        break
                    if t_first is not None and (now - last_msg_t) >= settle_s:
                        break
                    remaining = hard_deadline - now
                    wait_s = min(remaining, settle_s)
                    ready, _, _ = select.select([proc.stdout], [], [], wait_s)
                    if not ready:
                        continue
                    try:
                        msg = read_message(proc)
                    except (EOFError, OSError, ValueError):
                        break
                    if _answer_reverse_request(proc, msg, init_options):
                        continue
                    if msg.get("method") == "textDocument/publishDiagnostics":
                        params = msg.get("params", {})
                        if params.get("uri") == fixture_uri:
                            now = time.perf_counter()
                            if t_first is None:
                                t_first = now - t0
                            t_last = now - t0
                            diag_count = len(params.get("diagnostics", []))
                            last_msg_t = now

                output_fh.write(json.dumps({
                    "method": "textDocument/publishDiagnostics",
                    "request_index": i,
                    "latency_ms": round((t_first or 0.0) * 1000.0, 3),
                    "time_to_last_ms": round((t_last or 0.0) * 1000.0, 3) if t_last is not None else None,
                    "diagnostic_count": diag_count,
                }) + "\n")
                output_fh.flush()
        else:
            for i in range(args.requests):
                rid = next_id()
                msg = build_request(rid)
                t0 = time.perf_counter()
                proc.stdin.write(encode_message(msg))
                proc.stdin.flush()
                _resp = read_response(rid)
                t1 = time.perf_counter()
                output_fh.write(json.dumps({
                    "method": f"textDocument/{lsp_method}",
                    "request_index": i,
                    "latency_ms": round((t1 - t0) * 1000.0, 3),
                }) + "\n")
                output_fh.flush()

    finally:
        # Send LSP shutdown/exit so the server exits cleanly (Drop runs, dhat writes).
        try:
            shutdown_msg = req_shutdown()
            proc.stdin.write(encode_message(shutdown_msg))
            proc.stdin.flush()
            # Drain any trailing reverse requests / notifications until the
            # shutdown response arrives.
            while True:
                msg = read_message(proc)
                if _answer_reverse_request(proc, msg, init_options):
                    continue
                if msg.get("id") == shutdown_msg["id"]:
                    break
            proc.stdin.write(encode_message(notif_exit()))
            proc.stdin.flush()
        except (OSError, EOFError):
            pass
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
        if args.output != "-":
            output_fh.close()
        if args.trace_file and stderr_dest is not subprocess.DEVNULL:
            stderr_dest.close()


if __name__ == "__main__":
    main()
