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
echo "==> Latency statistics (ms):"
python3 - "$RESULTS_FILE" <<'EOF'
import json, sys, statistics

records = []
with open(sys.argv[1]) as f:
    for line in f:
        line = line.strip()
        if line:
            try:
                obj = json.loads(line)
                if "latency_ms" in obj:
                    records.append(obj["latency_ms"])
            except json.JSONDecodeError:
                pass

if not records:
    print("  No latency records found.")
    sys.exit(0)

records_sorted = sorted(records)
n = len(records_sorted)
p50 = records_sorted[int(n * 0.50)]
p95 = records_sorted[int(n * 0.95)]
p99 = records_sorted[min(int(n * 0.99), n - 1)]
mean = statistics.mean(records)

print(f"  count : {n}")
print(f"  mean  : {mean:.2f} ms")
print(f"  p50   : {p50:.2f} ms")
print(f"  p95   : {p95:.2f} ms")
print(f"  p99   : {p99:.2f} ms")
print(f"  min   : {min(records):.2f} ms")
print(f"  max   : {max(records):.2f} ms")
EOF

# ── RSS check ─────────────────────────────────────────────────────────────────
# Spawn once more just to sample RSS at idle (after initialize).
echo "==> Sampling server RSS (KB)..."
"$BINARY" &
SERVER_PID=$!
sleep 0.5
RSS_KB=$(ps -o rss= -p "$SERVER_PID" 2>/dev/null | tr -d ' ' || echo "N/A")
kill "$SERVER_PID" 2>/dev/null || true
echo "    RSS at idle: ${RSS_KB} KB"

rm -f "$RESULTS_FILE"
echo "==> Done."
