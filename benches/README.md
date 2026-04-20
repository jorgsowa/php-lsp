# Benchmarks

Four complementary tools for measuring and comparing LSP performance.

---

## Setup (once)

```bash
# Clone Laravel framework as a realistic PHP workspace fixture (~2,500 files).
./scripts/setup_laravel_fixture.sh
```

---

## 1. Criterion — wall-clock benchmarks (local, with comparison)

Best for: catching speed regressions during development.

```bash
# Save current performance as the "main" baseline:
./scripts/bench.sh save main

# After making a change, compare:
./scripts/bench.sh compare main
```

Criterion prints per-bench deltas with confidence intervals and colours
regressions red. HTML report: `target/criterion/report/index.html`.

The `index/workspace_scan/laravel_framework` group exercises indexing the full
Laravel codebase. Note: the store's default 1,000-file LRU cap means ~1,500
files are evicted during this bench — realistic for what users experience, but
not a clean "time to index N files" measurement.

---

## 2. iai-callgrind — instruction-count benchmarks (CI, deterministic)

Best for: regression gates in CI where wall-clock is too noisy.

**Requires valgrind + the runner binary (Linux only):**
```bash
cargo install iai-callgrind-runner   # once
cargo bench --bench iai_critical
```

On macOS, valgrind is not supported on recent OS versions — these benches
compile but will fail at runtime. Run them in CI (Linux) only.

Three benchmarks: `parse_medium`, `index_get_all_docs`, `hover_cross_file`.

---

## 3. E2E — latency + peak RSS (integration)

Best for: measuring request latency and memory growth against a real binary.

```bash
cargo build --release
./benches/integration/bench_lsp.sh --method hover --requests 200
```

Reports: startup time, post-index RSS, **peak RSS during indexing** (sampled
every 500 ms), and latency distribution (mean / p50 / p95 / p99).

To compare before/after a change:
```bash
./benches/integration/bench_lsp.sh > before.txt
# make your change, rebuild
./benches/integration/bench_lsp.sh > after.txt
diff before.txt after.txt
```

---

## 4. dhat — heap allocation profiling

Best for: understanding *where* memory is allocated (not just total RSS).

Use `mem_index` — a standalone binary that indexes a directory and exits
cleanly, so dhat's `Drop` runs and the profile is written.

Two modes:

- **default** — FileIndex only (fast, measures `DocumentStore`)
- **`--full`** — full `scan_workspace` pipeline: parse → `DefinitionCollector` →
  `FileIndex` → `codebase.finalize()`. This matches what the real LSP does and
  is the right mode for catching memory regressions.

```bash
# DocumentStore only (FileIndex path):
cargo run --release --bin mem_index -- benches/fixtures/laravel/src

# Full pipeline — matches real LSP workspace scan:
cargo run --release --bin mem_index -- --full benches/fixtures/laravel/src

# Full pipeline with heap profile:
cargo run --release --features dhat-heap --bin mem_index -- --full benches/fixtures/laravel/src
```

Example output (`--full`, 1 609 files):
```
RSS before:              10 752 KB (10.5 MB)
RSS after index:         59 472 KB (58.1 MB)
RSS after finalize:      59 616 KB (58.2 MB)
Delta (peak - before):   48 864 KB (47.7 MB)
  DocumentStore share:   48 720 KB (47.6 MB)
  Codebase share:           144 KB  (0.1 MB)
```

The large DocumentStore delta (~47 MB) comes from the system allocator holding
freed bumpalo arena pages after the double-parse per file in `scan_workspace`
(parse #1 for `DefinitionCollector`, parse #2 inside `docs.index()`). The
Codebase itself is tiny.

This writes `dhat-heap.json` in the current directory. Open it at:
https://nnethercote.github.io/dh_view/dh_view.html

To compare before/after a change:
```bash
cargo run --release --features dhat-heap --bin mem_index -- --full benches/fixtures/laravel/src
cp dhat-heap.json dhat-heap-before.json
# make your change, rebuild
cargo run --release --features dhat-heap --bin mem_index -- --full benches/fixtures/laravel/src
cp dhat-heap.json dhat-heap-after.json
# open both in browser tabs at https://nnethercote.github.io/dh_view/dh_view.html
```

Note: do **not** use `--features dhat-heap` with the `php-lsp` binary — the
tokio runtime keeps background tasks alive after LSP `exit`, so the process
never exits cleanly and `dhat-heap.json` is never written.

---

## 5. Tracing spans — per-operation timing

Best for: identifying which operation (finalize, analyze_stmts, etc.) is slow.

```bash
RUST_LOG=php_lsp=debug python3 benches/integration/lsp_client.py \
  --binary target/release/php-lsp \
  --fixture benches/fixtures/medium_class.php \
  --requests 10 \
  --output /dev/null \
  --trace-file trace.jsonl
```

Each closed span emits a JSON line with `time.busy` (CPU time) and `time.idle`
(waiting). Extract finalize durations:

```bash
jq 'select(.span.name=="codebase_finalize") | {"file": .span.file, "busy": .fields["time.busy"]}' trace.jsonl
```

Instrumented spans: `codebase_finalize`, `collect_definitions`, `analyze_stmts`
(in `semantic_diagnostics.rs`), `parse_from_disk` (in `document_store.rs`),
`workspace_scan` (in `backend.rs`).
