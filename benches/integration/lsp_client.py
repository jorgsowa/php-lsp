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
        --output results.jsonl
"""

import argparse
import json
import os
import subprocess
import sys
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
            "capabilities": {},
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


# ── Main ───────────────────────────────────────────────────────────────────────

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


def main() -> None:
    parser = argparse.ArgumentParser(description="Minimal LSP benchmark client")
    parser.add_argument("--binary", required=True, help="Path to php-lsp binary")
    parser.add_argument("--fixture", required=True, help="Path to PHP fixture file")
    parser.add_argument("--requests", type=int, default=100, help="Number of hover requests")
    parser.add_argument("--output", default="-", help="Output JSONL file (- for stdout)")
    parser.add_argument("--index-wait", type=float, default=2.0,
                        help="Seconds to wait for workspace index before sampling RSS (default: 2)")
    args = parser.parse_args()

    fixture_path = os.path.abspath(args.fixture)
    fixture_uri = "file://" + fixture_path
    root_uri = "file://" + os.path.dirname(fixture_path)

    with open(fixture_path, encoding="utf-8") as fh:
        fixture_text = fh.read()

    # Pin the hover position to a known method name in medium_class.php so the
    # server exercises symbol resolution rather than returning an empty response.
    # medium_class.php line 110 (LSP line 109), char 19 → `getTitle`.
    # Falls back to mid-file if the fixture is shorter than expected.
    lines = fixture_text.splitlines()
    hover_line = min(109, len(lines) - 1)
    hover_char = 19

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

        # Open the document.
        proc.stdin.write(encode_message(req_did_open(fixture_uri, fixture_text)))
        proc.stdin.flush()

        # ── RSS: wait for workspace index, then sample from outside the process ──
        time.sleep(args.index_wait)
        rss = rss_kb(proc.pid)
        output_fh.write(json.dumps({
            "method": "rss",
            "rss_kb": rss,
        }) + "\n")
        output_fh.flush()

        # ── Request latency ───────────────────────────────────────────────────
        for i in range(args.requests):
            rid = next_id()
            msg = req_hover(rid, fixture_uri, hover_line, hover_char)
            t0 = time.perf_counter()
            proc.stdin.write(encode_message(msg))
            proc.stdin.flush()
            _resp = read_message(proc)
            t1 = time.perf_counter()
            output_fh.write(json.dumps({
                "method": "textDocument/hover",
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
