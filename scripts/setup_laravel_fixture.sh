#!/usr/bin/env bash
set -euo pipefail

FIXTURE_DIR="$(dirname "$0")/../benches/fixtures/laravel"

if [ -d "$FIXTURE_DIR/.git" ]; then
  echo "Laravel fixture already present at $FIXTURE_DIR"
  exit 0
fi

echo "Cloning laravel/framework into $FIXTURE_DIR ..."
git clone --depth=1 https://github.com/laravel/framework "$FIXTURE_DIR"
echo "Done. $(find "$FIXTURE_DIR/src" -name '*.php' | wc -l | tr -d ' ') PHP files available."
