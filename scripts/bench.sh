#!/usr/bin/env bash
# bench.sh — criterion baseline management
#
# Usage:
#   scripts/bench.sh save [baseline]    save current results as a named baseline (default: main)
#   scripts/bench.sh compare [baseline] run benches and compare against a saved baseline
#   scripts/bench.sh run                run benches without comparison
#
# Criterion stores baselines under target/criterion/<group>/<bench>/base/
# HTML report is at target/criterion/report/index.html after any run.

set -euo pipefail

BASELINE="${2:-main}"
CMD="${1:-run}"

BENCHES=(parse index requests semantic)

case "$CMD" in
  save)
    echo "Running benchmarks and saving baseline '$BASELINE' ..."
    for b in "${BENCHES[@]}"; do
      cargo bench --bench "$b" -- --save-baseline "$BASELINE"
    done
    echo "Baseline '$BASELINE' saved."
    ;;
  compare)
    echo "Running benchmarks and comparing against baseline '$BASELINE' ..."
    for b in "${BENCHES[@]}"; do
      cargo bench --bench "$b" -- --baseline "$BASELINE"
    done
    ;;
  run)
    for b in "${BENCHES[@]}"; do
      cargo bench --bench "$b"
    done
    ;;
  *)
    echo "Usage: $0 {save|compare|run} [baseline_name]" >&2
    exit 1
    ;;
esac

echo "HTML report: target/criterion/report/index.html"
