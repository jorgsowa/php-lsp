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

set -uo pipefail

BASELINE="${2:-main}"
CMD="${1:-run}"

BENCHES=(parse index requests semantic)

# `compare` uses criterion's `--baseline`, which panics (and returns
# non-zero) for any individual bench that lacks the named baseline —
# e.g. a bench added after the baseline was saved. We intentionally do
# NOT `set -e` the loop, so a per-bench panic doesn't skip the remaining
# suites. Track failures and exit non-zero at the end so CI still fails
# loudly, but every bench has been attempted by then.
failed=0

case "$CMD" in
  save)
    echo "Running benchmarks and saving baseline '$BASELINE' ..."
    for b in "${BENCHES[@]}"; do
      if ! cargo bench --bench "$b" -- --save-baseline "$BASELINE"; then
        echo "::warning::bench '$b' failed during save" >&2
        failed=1
      fi
    done
    echo "Baseline '$BASELINE' saved."
    ;;
  compare)
    echo "Running benchmarks and comparing against baseline '$BASELINE' ..."
    for b in "${BENCHES[@]}"; do
      if ! cargo bench --bench "$b" -- --baseline "$BASELINE"; then
        echo "::warning::bench '$b' failed during compare (likely missing baseline for a sub-bench; re-run 'bench.sh save $BASELINE' to refresh)" >&2
        failed=1
      fi
    done
    ;;
  run)
    for b in "${BENCHES[@]}"; do
      if ! cargo bench --bench "$b"; then
        echo "::warning::bench '$b' failed" >&2
        failed=1
      fi
    done
    ;;
  *)
    echo "Usage: $0 {save|compare|run} [baseline_name]" >&2
    exit 1
    ;;
esac

if [ "$failed" -ne 0 ]; then
  echo "One or more benchmark suites failed; see warnings above." >&2
  exit 1
fi

echo "HTML report: target/criterion/report/index.html"
