#!/usr/bin/env bash
# bench_lsp.sh — build php-lsp, send N requests, report latency + RSS.
#
# Usage:
#   ./bench_lsp.sh [--method hover|definition|references] [--requests N]
#
# Defaults: method=hover, requests=100
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BINARY="$ROOT_DIR/target/release/php-lsp"
CLIENT="$SCRIPT_DIR/lsp_client.py"
FIXTURE="$ROOT_DIR/benches/fixtures/medium_class.php"
RESULTS_FILE="$(mktemp /tmp/bench_lsp_results.XXXXXX.jsonl)"

# ── Argument parsing ───────────────────────────────────────────────────────────
LSP_METHOD="hover"
NUM_REQUESTS=100

while [[ $# -gt 0 ]]; do
    case "$1" in
        --method)
            LSP_METHOD="$2"; shift 2 ;;
        --requests)
            NUM_REQUESTS="$2"; shift 2 ;;
        *)
            echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

# ── Build ──────────────────────────────────────────────────────────────────────
echo "==> Building php-lsp (release)..."
BUILD_START=$(date +%s%3N)
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml" 2>&1
BUILD_END=$(date +%s%3N)
BUILD_MS=$(( BUILD_END - BUILD_START ))
echo "    Build time: ${BUILD_MS} ms"

# ── Run client ────────────────────────────────────────────────────────────────
echo "==> Benchmarking textDocument/${LSP_METHOD} (${NUM_REQUESTS} requests)..."
START_MS=$(date +%s%3N)
python3 "$CLIENT" \
    --binary "$BINARY" \
    --fixture "$FIXTURE" \
    --requests "$NUM_REQUESTS" \
    --lsp-method "$LSP_METHOD" \
    --output "$RESULTS_FILE"
END_MS=$(date +%s%3N)
TOTAL_MS=$(( END_MS - START_MS ))
echo "    Total wall time: ${TOTAL_MS} ms"

# ── Parse results ──────────────────────────────────────────────────────────────
python3 - "$RESULTS_FILE" "$LSP_METHOD" <<'EOF'
import json, sys, statistics

results_path = sys.argv[1]
lsp_method   = sys.argv[2]

startup_ms = None
rss_kb = None
latencies = []

with open(results_path) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = obj.get("method", "")
        if method == "startup":
            startup_ms = obj["latency_ms"]
        elif method == "rss":
            rss_kb = obj.get("rss_kb")
        elif "latency_ms" in obj:
            latencies.append(obj["latency_ms"])

print("==> Startup time (spawn → initialize response):")
if startup_ms is not None:
    print(f"    {startup_ms:.1f} ms")
else:
    print("    N/A")

print("==> RSS after workspace index:")
if rss_kb is not None:
    print(f"    {rss_kb} KB ({rss_kb / 1024:.1f} MB)")
else:
    print("    N/A")

print(f"==> textDocument/{lsp_method} latency statistics (ms):")
if not latencies:
    print("    No latency records found.")
    sys.exit(0)

s = sorted(latencies)
n = len(s)
print(f"    count : {n}")
print(f"    mean  : {statistics.mean(latencies):.2f}")
print(f"    p50   : {s[int(n * 0.50)]:.2f}")
print(f"    p95   : {s[int(n * 0.95)]:.2f}")
print(f"    p99   : {s[min(int(n * 0.99), n - 1)]:.2f}")
print(f"    min   : {s[0]:.2f}")
print(f"    max   : {s[-1]:.2f}")
EOF

rm -f "$RESULTS_FILE"
echo "==> Done."
