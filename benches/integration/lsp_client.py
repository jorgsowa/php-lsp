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


def req_initialize(root_uri: str) -> dict:
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
            },
            "initializationOptions": {},
        },
    }


def notif_initialized() -> dict:
    return {"jsonrpc": "2.0", "method": "initialized", "params": {}}


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

def _drain_until_indexed(proc, index_wait: float) -> None:
    """
    Read server notifications in a background thread until either:
      - a $/progress end-of-work-done notification is received, or
      - `index_wait` seconds have elapsed (safety fallback).

    php-lsp sends ``$/progress`` with ``kind == "end"`` when the
    workspace scan finishes.  Advertising ``window.workDoneProgress``
    capability in ``initialize`` is required for the server to emit these.
    """
    deadline = time.monotonic() + index_wait
    done = threading.Event()

    def _reader():
        while not done.is_set() and time.monotonic() < deadline:
            try:
                proc.stdout._sock.settimeout(0.1)  # noqa: SLF001
            except AttributeError:
                pass
            try:
                msg = read_message(proc)
            except (EOFError, OSError, ValueError):
                break
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

    t = threading.Thread(target=_reader, daemon=True)
    t.start()
    # Wait for the end signal or the deadline, whichever comes first.
    done.wait(timeout=index_wait)
    done.set()  # signal thread to stop if it's still running
    t.join(timeout=1.0)


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
        choices=["hover", "definition", "references"],
        default="hover",
        help="LSP request method to benchmark (default: hover)",
    )
    args = parser.parse_args()

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
    proc = subprocess.Popen(
        [args.binary],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )

    output_fh = open(args.output, "w", encoding="utf-8") if args.output != "-" else sys.stdout

    try:
        # ── Startup: measure time from spawn to initialize response ──────────
        proc.stdin.write(encode_message(req_initialize(root_uri)))
        proc.stdin.flush()
        _init_resp = read_message(proc)
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
        _drain_until_indexed(proc, args.index_wait)

        rss = rss_kb(proc.pid)
        output_fh.write(json.dumps({
            "method": "rss",
            "rss_kb": rss,
        }) + "\n")
        output_fh.flush()

        # ── Request latency ───────────────────────────────────────────────────
        for i in range(args.requests):
            rid = next_id()
            msg = build_request(rid)
            t0 = time.perf_counter()
            proc.stdin.write(encode_message(msg))
            proc.stdin.flush()
            _resp = read_message(proc)
            t1 = time.perf_counter()
            output_fh.write(json.dumps({
                "method": f"textDocument/{lsp_method}",
                "request_index": i,
                "latency_ms": round((t1 - t0) * 1000.0, 3),
            }) + "\n")
            output_fh.flush()

    finally:
        proc.stdin.close()
        proc.wait(timeout=5)
        if args.output != "-":
            output_fh.close()


if __name__ == "__main__":
    main()
