#!/usr/bin/env bash
# bench_lsp.sh — build php-lsp, send 100 hover requests, report latency + RSS.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BINARY="$ROOT_DIR/target/release/php-lsp"
CLIENT="$SCRIPT_DIR/lsp_client.py"
FIXTURE="$ROOT_DIR/benches/fixtures/medium_class.php"
RESULTS_FILE="$(mktemp /tmp/bench_lsp_results.XXXXXX.jsonl)"

# ── Build ──────────────────────────────────────────────────────────────────────
echo "==> Building php-lsp (release)..."
BUILD_START=$(date +%s%3N)
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml" 2>&1
BUILD_END=$(date +%s%3N)
BUILD_MS=$(( BUILD_END - BUILD_START ))
echo "    Build time: ${BUILD_MS} ms"

# ── Startup latency ────────────────────────────────────────────────────────────
echo "==> Measuring startup + initialize latency..."
START_MS=$(date +%s%3N)
python3 "$CLIENT" \
    --binary "$BINARY" \
    --fixture "$FIXTURE" \
    --requests 100 \
    --output "$RESULTS_FILE"
END_MS=$(date +%s%3N)
TOTAL_MS=$(( END_MS - START_MS ))
echo "    Total wall time for 100 requests: ${TOTAL_MS} ms"

# ── Parse results ──────────────────────────────────────────────────────────────
python3 - "$RESULTS_FILE" <<'EOF'
import json, sys, statistics

startup_ms = None
rss_kb = None
latencies = []

with open(sys.argv[1]) as f:
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

print("==> Hover latency statistics (ms):")
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
