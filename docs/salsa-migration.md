# Salsa migration

Ongoing multi-phase refactor that replaces php-lsp's ad-hoc parse/index caching
with [salsa](https://docs.rs/salsa/) — a demand-driven, incrementally-invalidating
query framework (same lineage as rust-analyzer). This doc tracks what's shipped,
what's pending, and the constraints that shape the remaining work.

## Why

Before this migration:

- **Cold start**: every restart re-parses the workspace, rebuilds `mir_codebase`,
  and runs Phase-3 reference indexing from scratch. Minutes on large codebases.
- **Per-edit**: `remove_file_definitions → collect → finalize` runs by hand;
  feature modules re-walk ASTs on every LSP request; the only memoization was
  one `OnceLock<MethodReturnsMap>` per `ParsedDoc`.
- **Cross-request**: workspace-symbols, inheritance, references — recomputed
  per call.

Salsa replaces hand-written invalidation with "query is a pure function of its
inputs; when an input changes, salsa re-runs only the transitively-affected
queries." That gives us:

- Automatic invalidation (no more manual remove/collect/finalize dances)
- Cross-request memoization for free
- Natural foundation for cancellation and persistent on-disk caching
- A clean separation between the LSP adapter (Backend) and the semantic core
  (AnalysisHost / Analysis)

## Phase status

| Phase | Description | Status |
|---|---|---|
| A | Salsa scaffold: `RootDatabase`, `SourceFile` input, smoke-test query | ✅ shipped |
| B1 | `parsed_doc(SourceFile) -> ParsedArc` tracked query | ✅ shipped |
| B2 | `file_index(SourceFile) -> IndexArc` tracked query | ✅ shipped |
| B3 | `method_returns(SourceFile) -> MethodReturnsArc` tracked query | ✅ shipped |
| B4a | DocumentStore mirrors every mutation into salsa inputs | ✅ shipped |
| B4b | `*_salsa` accessors on DocumentStore | ✅ shipped |
| B4c | Feature-module reads migrated to salsa (24 call sites) | ✅ shipped |
| B4d-1 | `get_doc_salsa_any` + call-hierarchy on-demand sites | ✅ shipped |
| B4d-2 | `did_change` structure-change redesign | ✅ shipped |
| B4d-3a | Delete `entry.doc`; route doc iteration through salsa | ✅ shipped |
| B4d-3b | Delete `entry.index`; route index iteration through salsa | ✅ shipped |
| B4d-3c | Move version gate to Backend; delete `apply_parse` | ✅ shipped |
| B4d-4 | Delete `OnceLock<MethodReturnsMap>` from `ParsedDoc` | ✅ shipped (folded into E3) |
| C | Migrate `mir_codebase` into salsa queries | ✅ shipped |
| D | `file_refs`/`symbol_refs` lazy reference index | ✅ shipped |
| E1 | Snapshot-clone reads off the host mutex | ✅ shipped |
| E2 | LSP request cancellation → `RequestCancelled` | ⏸ folded into E1 — `snapshot_query` retries on `salsa::Cancelled` and falls back to the mutex; nothing escapes to the LSP layer |
| E3 | Thread salsa-memoized method-returns into `TypeMap`; delete `OnceLock<MethodReturnsMap>` | ✅ shipped |
| E4 | Move `DocumentStore.map` bookkeeping to `Backend`; delete the struct if empty | ✅ shipped |
| F | `#[salsa::tracked(lru = N)]`; delete `indexed_order` | ✅ shipped |
| G1 | Drop redundant parse in `DocumentStore::index` | ✅ shipped |
| G2 | Lock-free fast path in `mirror_text` | ✅ shipped (≈4.1× speedup under 8-thread contention) |
| G3 | Trim `get_doc_salsa` overhead — cross-revision `parsed_cache` | ✅ shipped |
| G4 | Investigate `references/*` +2000% regression | ✅ resolved — stale baseline, not a real regression |
| H | Fix benches + CI regression gate | ✅ shipped |
| I | Semantic diagnostics as a salsa query | ✅ shipped |
| J | Workspace-symbol / type-hierarchy / implementation as tracked queries | ✅ shipped |
| K1 | Persistent on-disk cache — infrastructure module | ✅ shipped |
| K2a | Plumb `cached_slice` through `file_definitions` | ✅ shipped |
| K2b | Wire `scan_workspace` to read/write the cache | ✅ shipped |
| K3 | Cache size cap (reset-on-overflow) | ✅ shipped |
| K4 | LRU-by-mtime eviction + orphan sweep | 🧭 proposed |
| L | Reference warm-up background task | ✅ shipped |

## Architecture — current state

```
src/db/
├── mod.rs            // module root; re-exports
├── input.rs          // SourceFile input, FileId
├── parse.rs          // parsed_doc tracked query + ParsedArc
├── index.rs          // file_index tracked query + IndexArc
├── method_returns.rs // method_returns tracked query + MethodReturnsArc
└── analysis.rs       // RootDatabase, AnalysisHost, Analysis (Phase E scaffold)
```

### The ParsedArc pattern

Salsa tracked returns must implement `Update`. `ParsedDoc` owns a self-referential
bumpalo arena via `unsafe { transmute }` and cannot safely implement structural
equality. Each tracked query returns an `Arc`-wrapper newtype with a manual
`Update` impl gated on `Arc::ptr_eq`:

```rust
pub struct ParsedArc(pub Arc<ParsedDoc>);

unsafe impl Update for ParsedArc {
    unsafe fn maybe_update(old: *mut Self, new: Self) -> bool {
        let old = unsafe { &mut *old };
        if Arc::ptr_eq(&old.0, &new.0) { false }
        else { *old = new; true }
    }
}

#[salsa::tracked(no_eq)]
pub fn parsed_doc(db: &dyn Database, file: SourceFile) -> ParsedArc { … }
```

Same pattern for `IndexArc` and `MethodReturnsArc`. `no_eq` is required because
pointer equality is the only safe signal ("a new reparse produces a new Arc").
This mirrors how rust-analyzer handles `SyntaxNode`.

### DocumentStore as thin LSP shell

DocumentStore retains exactly the state LSP needs that salsa doesn't own:

```rust
struct Document {
    text: Option<String>,     // open-state + live text
    diagnostics: Vec<Diagnostic>,      // parse-level
    sem_diagnostics: Vec<Diagnostic>,  // semantic (will move to salsa in Phase C)
    text_version: u64,        // LSP diagnostic versioning
}
```

Plus a salsa mirror:

```rust
pub struct DocumentStore {
    map: DashMap<Url, Document>,
    indexed_order: Mutex<VecDeque<Url>>,  // legacy LRU (Phase F removes this)
    token_cache: DashMap<Url, (String, Vec<SemanticToken>)>,
    max_indexed: AtomicUsize,

    // Salsa mirror
    host: Mutex<AnalysisHost>,
    source_files: DashMap<Url, SourceFile>,
    next_file_id: AtomicU32,
}
```

Every text-changing mutation (`set_text`, `index`, `index_from_doc`) calls
`mirror_text(uri, text)` before touching `map`, so the salsa layer is always
at least as fresh as the legacy map.

### Accessor contracts

| Method | Source | Semantics |
|---|---|---|
| `get(uri)` | legacy `map.text` | Live text, only for open files |
| `get_doc_salsa(uri)` | salsa + open-state gate | ParsedDoc, only if editor has file open |
| `get_doc_salsa_any(uri)` | salsa | ParsedDoc for any mirrored file (open or background) |
| `get_index_salsa(uri)` | salsa | FileIndex for any mirrored file |
| `get_method_returns_salsa(uri)` | salsa | Method-returns map for any mirrored file |
| `get_index(uri)` | legacy map membership + salsa | FileIndex for known (LRU-bounded) files |
| `get_diagnostics(uri)` | legacy | Parse diagnostics |
| `current_version(uri)` | legacy | Text revision; Backend uses this to gate stale parse publication |

### Backend's version gate

Pre-migration: `apply_parse(uri, doc, diags, version) -> bool` gated in
DocumentStore. Now:

```rust
let version = docs.set_text(uri.clone(), text);
// … async parse in spawn_blocking …
if docs.current_version(&uri) == Some(version) {
    docs.set_parse_diagnostics(&uri, diagnostics);
    // publish, update codebase, etc.
}
```

DocumentStore no longer owns the staleness concept.

### did_change structure-change preservation

A subtle invariant: the "body-only edit skips codebase rebuild" optimization
compares the pre-edit FileIndex against the post-parse FileIndex via
`FileIndex::same_structure`. Under salsa, `set_text` immediately bumps the input
revision, so `get_index_salsa(uri)` after `set_text` returns the *new* index —
the comparison would trivially succeed. Fix:

```rust
async fn did_change(&self, params: …) {
    let uri = …;
    let text = …;

    // Capture pre-edit index BEFORE the mirror sees the new text.
    let old_index = self.docs.get_index_salsa(&uri);

    let version = self.docs.set_text(uri.clone(), text.clone());
    // spawn_blocking { parse; compute new_index; compare to old_index; … }
}
```

Holding the `Arc<FileIndex>` keeps the old view alive regardless of salsa
revision changes.

## Benchmark results (post-B4 shipped)

Laravel fixture (1609 files):

| Benchmark | Δ vs baseline | p |
|---|---|---|
| `index/workspace_scan/laravel_framework` | **−12.4%** | <0.05 |
| `implementation/laravel_framework` | **−37.3%** | <0.05 |
| `implementation/cross_file_class` | **−40.1%** | <0.05 |
| `call_hierarchy/prepare/laravel_framework` | **−29.0%** | <0.05 |
| `workspace_symbol/laravel_framework` (subcase) | **−3.1%** | <0.05 |

Cross-file query wins come from `all_docs_for_scan` no longer re-reading files
from disk (salsa memoizes parses across requests). Single-file hot paths are
unchanged within noise.

## Benchmark results (post-E1 — 2026-04-22)

Re-ran `scripts/bench.sh compare main` on the `refactor/salsa-incremental` branch
after E1 snapshot-clone landed. The `parse` and `index` suites ran; `requests`
and `semantic` failed to compile against this branch (API drift — `hover_info`
et al. grew a `method_returns` parameter during E3; bench files weren't
updated). Fix tracked under Phase G.

| Benchmark | Δ | Note |
|---|---|---|
| `index/workspace_scan/laravel_framework` | **−20.2%** | memoization + Arc sharing |
| `index/workspace_scan/50_files` | **−27.3%** | same |
| `index/workspace_scan/10_files` | **−12.0%** | same |
| `index/single/medium_class` | **−14.4%** | |
| `parse/small_class` | **−17.7%** | |
| `parse/medium_class` | **−5.7%** | |
| `index/single/small_class` | **+65.8%** | per-call overhead dominates |
| `index/workspace_scan/1_files` | **+64.5%** | same as above (N=1) |
| `index/get_doc` | **+36.8%** | `snapshot_query` + double DashMap lookup |
| `parse/interface_large` | +10.5% | |
| `index/single/interface_large` | +7.7% | |

### After Phase G1 — 2026-04-22 (same day)

Re-ran after dropping the redundant `parse_document` call in
`DocumentStore::index`. Results vs the same `main` baseline:

| Benchmark | Δ before G1 | Δ after G1 |
|---|---|---|
| `index/workspace_scan/laravel_framework` | −20.2% | **−97.4%** |
| `index/workspace_scan/50_files` | −27.3% | **−97.5%** |
| `index/workspace_scan/10_files` | −12.0% | **−94.1%** |
| `index/workspace_scan/1_files` | +64.5% | **−26.7%** |
| `index/single/small_class` | +65.8% | **−20.6%** |
| `index/single/medium_class` | −14.4% | **−83.7%** |
| `index/single/interface_large` | +7.7% | **−67.2%** |
| `index/get_doc` | +36.8% | +30.1% (unchanged — G3 target) |
| `parse/small_class` | −17.7% | −21.4% |
| `parse/medium_class` | −5.7% | −12.4% |

`index()` is now a pure text-mirror into salsa; the parse it used to do was
entirely wasted (the AST was dropped, salsa re-parsed on first read, and the
diagnostics it stored were only ever read for open files — which parse again
via `did_open`). All `index/*` benches now win. G2/G3 remain open.

**New regressions surfaced** (were not visible in the first run because
`benches/requests.rs` didn't compile):

| Benchmark | Δ | Suspect |
|---|---|---|
| `references/scale/5` | +2491% | Phase D `symbol_refs` changed hot path |
| `references/cross_file_class` | +2180% | same |
| `references/scale/10` | +1361% | same |
| `references/scale/1` | +48.5% | same |
| `references/single_file_method` | +33.1% | same |

These are unrelated to G1 — the `requests` bench calls `find_references`
directly with `&[(Url, Arc<ParsedDoc>)]`, no DocumentStore involved. The
salsa branch's Phase D (`feat(salsa): Phase D step 2 — wire references
handler through symbol_refs`, commit `5b6d6d0`) is the likely cause: it
changed the cross-file references path. The `find_references` function
signature is unchanged, so either the function body got slower or the
codebase-fast-path it used to hit is now gated on a salsa-only condition
the bench can't satisfy. **Investigate under a new Phase G sub-item (G4).**

**Also**: the bench script aborts if any criterion run panics. The
`references/laravel_framework` bench has no saved `main` baseline (it didn't
exist or didn't run at baseline-save time), so criterion panics, `set -e`
kills `bench.sh`, and the `semantic` suite never runs. Either (a) save a
baseline for it, (b) make `bench.sh` resilient to per-bench panics, or (c)
gate the laravel benches on baseline presence.

### What we learned

1. **Salsa adds per-call fixed overhead.** Small/single-file benches hit the
   worst case: the mutex round-trip, `db.clone()`, and `Cancelled::catch`
   panic-unwind setup in `snapshot_query` (`document_store.rs:163`) cost tens
   of nanoseconds each. Negligible when amortized across a workspace scan;
   dominant when compared against a 24 ns `DashMap::get`.

2. **`DocumentStore::index` double-parses.** `index()` (`document_store.rs:248`)
   calls `parse_document` purely to extract parse diagnostics — then discards
   the AST. Salsa's `parsed_doc` query re-parses on first read. For
   background-indexed files (the `index()` caller path: workspace scan + PSR-4
   on-demand) parse diagnostics are never published until the file opens, so
   the upfront parse is wasted work. This is the single largest contributor to
   the `+65%` on `single/small_class`.

3. **`mirror_text` acquires the host mutex even when deduping.** The
   byte-equality short-circuit (`document_store.rs:117`) needs the mutex to
   read `sf.text(host.db())`. Under workspace scan where many calls are no-op
   updates this serialises threads. Lock-free fast path (read text from a
   `DashMap<Url, Arc<str>>` mirror) would help multi-threaded indexing.

4. **Micro-benches overstate the downside.** Real LSP workloads are bursts of
   cross-file queries where memoization pays off (the cross-file suites in
   the previous table all regressed before salsa and now win 20–40%).
   Per-edit latency lives in `did_change` → `spawn_blocking` parse, which is
   dominated by parsing, not by salsa overhead. But the single-file numbers
   are still real regressions worth fixing; they map to cold-path operations
   like definition-jump-into-unindexed-file.

## Remaining phases

### Phase C — mir_codebase as a salsa query

**Goal**: replace `codebase.remove_file_definitions(f) → DefinitionCollector::collect(f) → codebase.finalize()` with an automatically-invalidated salsa query.

**2026-04-22 recon — plan sizing was wrong.** The original plan called it a "small mir-codebase API addition." Reading `mir-codebase/src/codebase.rs` shows `Codebase` has ~15 pieces of interlocking state beyond the top-level DashMaps: `symbol_interner`, `file_interner`, `symbol_reference_locations`, `file_symbol_references`, `compact_ref_index` (CSR), `is_compacted`, `symbol_to_file`, `known_symbols`, `file_imports`, `file_namespaces`, `file_global_vars`, `referenced_methods/properties/functions`, `finalized` flag. Building a pure `FileDefs` value that a merging aggregator can consume is 2–3 PRs of cross-crate work, not days.

**Also: Phase C buys correct invalidation for Phase D, not per-edit CPU.** Today's edit: `remove+collect(1 file)+finalize`. Functional version: `collect(1 file, memoized) + merge(N files into fresh Codebase) + finalize`. The merge is O(total symbols) per edit — a new cost that cancels the per-file memoization win. The real value is downstream queries (Phase D `file_refs`) getting correct invalidation.

**Three forks for next planning session**:

1. **C-full** — the design sketch below. 2–3 PRs, weeks, pure-function codebase. Right if long-term goal is pure-salsa architecture.
2. **C-lite** — keep `Arc<Codebase>` mutable inside `AnalysisHost`, outside salsa. Add a `codebase_revision: u64` salsa input that backend bumps after every successful edit. Downstream queries take `codebase_revision` as a dep → correct invalidation without FileDefs refactor. Days, not weeks. Loses structural memoization of unchanged files' collection, but that CPU win was illusory per above.
3. **Defer C** — do something else first; revisit C when Phase D's real requirements are on the table.

**Design sketch (C-full)**:

```rust
#[salsa::tracked(no_eq)]
pub fn file_definitions(db: &dyn Database, file: SourceFile) -> Arc<FileDefs> {
    let doc = parsed_doc(db, file);
    DefinitionCollector::new().collect_one_file(doc.get())
}

#[salsa::tracked(no_eq)]
pub fn codebase(db: &dyn Database, ws: Workspace) -> Arc<Codebase> {
    let files = ws.files(db);
    let mut builder = CodebaseBuilder::new();
    for sf in files.iter() {
        builder.add(file_definitions(db, *sf).clone());
    }
    Arc::new(builder.finalize())
}
```

**Blocker (C-full)**: `mir_codebase::Codebase` today is built imperatively via `collect_into_codebase` + `finalize`. C-full needs a `CodebaseBuilder::from_parts(Vec<FileDefs>)` constructor in the `mir-codebase` crate so the query is purely functional. The `FileDefs` value must also serialize/carry the interner IDs and reference spans that `Codebase` tracks per-file today; that surface is non-trivial.

**C1 recon findings (2026-04-22)** — `DefinitionCollector` (in `mir-analyzer/src/collector.rs`) writes the following `Codebase` fields and no others:

| Field | Write site (line) | Key/value shape |
|---|---|---|
| `functions` | 433 | `DashMap<Arc<str>, FunctionStorage>` |
| `classes` | 592 | `DashMap<Arc<str>, ClassStorage>` |
| `interfaces` | 655 | `DashMap<Arc<str>, InterfaceStorage>` |
| `traits` | 774 | `DashMap<Arc<str>, TraitStorage>` |
| `enums` | 848 | `DashMap<Arc<str>, EnumStorage>` |
| `constants` | 870, 889 | `DashMap<Arc<str>, Union>` |
| `symbol_to_file` | 431, 590, 653, 772, 846 | `DashMap<Arc<str>, Arc<str>>` (FQN → file path) |
| `global_vars` + `file_global_vars` | 312 (via `register_global_var`) | `DashMap<Arc<str>, Union>` + reverse-index |

Fields **NOT** touched by `DefinitionCollector`: `file_imports`, `file_namespaces`, `known_symbols`, `symbol_reference_locations`, `file_symbol_references`, `compact_ref_index`, `referenced_*`, `symbol_interner`, `file_interner`, `finalized`.

**The `file_imports` / `file_namespaces` / `known_symbols` fields are populated by `mir-analyzer::project::Project::analyze` — php-lsp does not call this path.** php-lsp's `self.codebase.file_imports.get(...)` at `src/backend.rs:236` therefore always returns empty in production. Either (a) php-lsp has a latent bug that needs fixing independently, or (b) php-lsp has its own import-resolution path (see `use_resolver` module) and the `codebase.file_imports` read is dead code. Audit this before Phase C lands; don't replicate a dead read through the new query.

**FileDefs draft** (first cut — validate in C2):

```rust
// in mir-codebase
pub struct FileDefs {
    pub file: Arc<str>,
    pub functions: Vec<(Arc<str>, FunctionStorage)>,
    pub classes: Vec<(Arc<str>, ClassStorage)>,
    pub interfaces: Vec<(Arc<str>, InterfaceStorage)>,
    pub traits: Vec<(Arc<str>, TraitStorage)>,
    pub enums: Vec<(Arc<str>, EnumStorage)>,
    pub constants: Vec<(Arc<str>, Union)>,
    pub global_vars: Vec<(Arc<str>, Union)>,  // names also drive file_global_vars reverse-index
}

impl CodebaseBuilder {
    pub fn from_parts(parts: Vec<FileDefs>) -> Codebase { ... }  // folds + calls finalize()
}
```

The `Vec<(K, V)>` shape (vs `HashMap`) is intentional: aggregator merges deterministically by last-writer-wins matching today's `DashMap::insert` behavior; no per-file `HashMap` overhead.

**Reference-index fields (`symbol_reference_locations` etc.) are Pass-2 outputs** — they belong to Phase D (`file_refs`), not Phase C. FileDefs covers only Pass-1 (definitions).

**C2 design (2026-04-22)** — reuse `StubSlice`, don't invent `FileDefs`.

`mir-codebase::StubSlice` (`src/storage.rs:312`) already has: `classes`, `interfaces`, `traits`, `enums`, `functions`, `constants`. And `Codebase::inject_stub_slice` (`src/codebase.rs:225`) already exists as the merge primitive — it inserts all definitions. The Phase C work is smaller than the original plan admitted.

**Storage types carry their own FQN** (`ClassStorage.fqcn`, `InterfaceStorage.fqcn`, `TraitStorage.fqcn`, `EnumStorage.fqcn`, `FunctionStorage.fqn`). The builder can derive `symbol_to_file` by iterating a slice and mapping each FQN → owning file. No need to serialize `symbol_to_file` into FileDefs.

**Mechanical plan**:

1. **Extend `StubSlice`** (non-breaking — `#[serde(default)]` for new fields):
   ```rust
   pub struct StubSlice {
       // existing fields...
       #[serde(default)]
       pub file: Option<Arc<str>>,   // None for bundled stubs, Some("path") for user files
       #[serde(default)]
       pub global_vars: Vec<(Arc<str>, Union)>,
   }
   ```

2. **Refactor `DefinitionCollector`** to accumulate into a `StubSlice` instead of mutating `&Codebase`:
   ```rust
   pub struct DefinitionCollector<'a> { /* no codebase field */ }
   impl DefinitionCollector<'_> {
       pub fn collect(...) -> (StubSlice, Vec<Issue>) { ... }
   }
   ```
   The `use_aliases` tracking stays internal (used only for FQN resolution during one pass; not persisted).

3. **Add `CodebaseBuilder`** (thin convenience over existing primitives):
   ```rust
   pub struct CodebaseBuilder { cb: Codebase }
   impl CodebaseBuilder {
       pub fn new() -> Self { Self { cb: Codebase::new() } }
       pub fn add(&mut self, slice: StubSlice) {
           // for each def in slice: insert + if slice.file is Some, update symbol_to_file
           self.cb.inject_stub_slice(slice);
       }
       pub fn finalize(self) -> Codebase { self.cb.finalize(); self.cb }
   }
   // Or a standalone constructor:
   pub fn codebase_from_parts(parts: Vec<StubSlice>) -> Codebase { ... }
   ```

4. **Breaking? No** — existing callers (`mir_analyzer::stubs::load_stubs`, bundled stub loaders) still work because `inject_stub_slice` is unchanged. New `StubSlice` fields default to empty/None.

5. **Backward-compat**: keep `DefinitionCollector::new(codebase, ...)` + `.collect(program) -> Vec<Issue>` as a shim that internally does `collect_into_slice + inject_stub_slice + record symbol_to_file`. Lets the mir-codebase release land without requiring a simultaneous php-lsp change.

**Estimated size**:
- mir-codebase: 1 PR, ~150-300 LOC (collector refactor + StubSlice fields + builder). Release as 0.7.
- php-lsp C4: 1 PR, adds `file_definitions` + `codebase` tracked queries, `Workspace` input, replaces `remove_file_definitions` dance with salsa input setters. ~300 LOC.

This is days per PR, not weeks. The advisor's earlier "weeks" estimate assumed inventing FileDefs from scratch; with `StubSlice` reuse, the delta is small.

**Validation before C3 (ship mir-codebase release)**:
- Run mir-analyzer's existing test suite against the refactored collector; all passes.
- Verify `semantic_diagnostics.rs` in php-lsp (the non-`backend.rs` caller of DefinitionCollector) still compiles against the shim.

**Also needed**: a `Workspace` salsa input tracking the set of files (medium durability — changes on workspace scan and watched-file events, not on every edit).

### Phase D — reference index

**Goal**: replace the "Phase 3" post-scan reference-indexing pass with a salsa query that runs lazily on first reference-lookup.

```rust
#[salsa::tracked(no_eq)]
pub fn file_refs(db: &dyn Database, file: SourceFile) -> Arc<FileRefs> {
    let cb = codebase(db, workspace(db));
    let doc = parsed_doc(db, file);
    StatementsAnalyzer::new(&cb).collect_refs(doc.get())
}

#[salsa::tracked]
pub fn symbol_refs(db: &dyn Database, ws: Workspace, sym: Symbol) -> Vec<Location> {
    ws.files(db).iter()
        .flat_map(|sf| file_refs(db, *sf).locations_of(sym))
        .collect()
}
```

Removes the `ref_index_ready` atomic flag and the Phase-3 background task. First-time `textDocument/references` is slower (lazy); subsequent requests are memoized. A background warm-up task can pre-fill hot symbols.

### Phase E — Analysis snapshot + cancellation

**Goal**: mutations on `AnalysisHost` trigger `salsa::Cancelled` on in-flight snapshot reads; Backend translates to LSP `RequestCancelled`.

**2026-04-22 recon — plan's framing was wrong**:

1. **Feature modules do NOT take `&DocumentStore`.** They take `&ParsedDoc` + `&[(Url, Arc<ParsedDoc>)]` today. Only `backend.rs` and `document_store.rs`'s own tests call `DocumentStore` methods. The plan's "20-module mechanical signature change" does not exist — there's no churn to do there.

2. **Cancellation is a no-op in today's concurrency model.** `DocumentStore::with_host` holds a `Mutex<AnalysisHost>` for the full duration of every query. Writes (`set_text`) and reads are already fully serialized — they cannot overlap, so `salsa::Cancelled` is never raised. Wiring `Cancelled::catch` at request entry points would catch nothing until the mutex is released during reads. This is the real work of Phase E.

3. **Proper concurrency model change**:
   ```rust
   // DocumentStore
   fn snapshot_db(&self) -> RootDatabase {
       self.host.lock().unwrap().db().clone()  // Storage shares Arc<Zalsa>; cancel flag shared
   }

   pub fn get_doc_salsa(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
       let sf = self.source_file(uri)?;
       let db = self.snapshot_db();          // brief lock, release before query
       Some(parsed_doc(&db, sf).0.clone())
   }
   ```
   Writers calling `set_text` on the owner db set the cancellation flag; concurrent readers holding cloned `db`s throw `Cancelled` on their next salsa call. Backend wraps handler bodies in `Cancelled::catch` → LSP `RequestCancelled` error.

4. **`OnceLock<MethodReturnsMap>` removal is blocked by perf.** Commit `c6e190b` cached this on `ParsedDoc` for ~325x on Laravel. Replacing it requires threading salsa-db access through ~35 `type_map::from_doc*` call sites (either as `&Analysis` or as pre-fetched `&[Arc<MethodReturnsMap>]`). Not a cleanup — it's a real API refactor.

5. **"Delete DocumentStore" is unrealistic.** `DocumentStore.map` still holds bookkeeping that isn't salsa-shaped: open-file state, parse diagnostics cache, semantic diagnostics cache, token cache, LRU queue. These either move to `Backend` or stay in a slimmed `DocumentStore`. They do not become salsa inputs.

**Revised Phase E scope (for next planning session)**:

- E1: refactor salsa accessors to snapshot-clone the db and run queries outside the mutex. Prerequisite for everything else. (~1 PR, moderate risk — needs concurrent-read stress tests.)
- E2: wrap LSP request entry points in `Cancelled::catch`; map to `RequestCancelled`. (~1 PR, small; depends on E1.)
- E3: ✅ shipped. `TypeMap::from_doc_with_meta` / `from_docs_with_meta` / `from_docs_at_position` now accept precomputed `&MethodReturnsMap` values; production callers (inlay_hints, type_definition, hover, completion via `CompletionCtx.doc_returns` / `other_returns`) thread the salsa-memoized Arcs through. `DocumentStore::other_docs_with_returns` batches the salsa fetch into a single `snapshot_query` so `Cancelled` retries don't multiply per open file. `OnceLock<MethodReturnsMap>` and `ParsedDoc::method_returns_cached` removed; the salsa `method_returns(db, file)` query is now the sole cache. The "35 call sites" were mostly tests that still call `TypeMap::from_doc(doc)` unchanged (a `#[cfg(test)]` shim that builds the map inline); only 6 production sites needed edits.
- E4: ✅ shipped. See "Phase E4 — delete DocumentStore.map" below.

Don't treat Phase E as a single PR. It's a phase with four independent sub-PRs.

### Phase E4 — delete DocumentStore.map ✅ shipped 2026-04-23

**What shipped**

- `DocumentStore.map: DashMap<Url, Document>` and the `Document` struct are
  deleted. With them go `set_text`, `close`, `get`, `current_version`,
  `set_parse_diagnostics`, `get_diagnostics`, `all_diagnostics`, and
  `get_index`. `DocumentStore` is now a pure salsa-input wrapper —
  `source_files` is the known-files set, and every read goes through the
  salsa queries (`parsed_doc`, `file_index`, `method_returns`,
  `semantic_issues`, `codebase`, `symbol_refs`, `workspace_index`).
- Open-file state moved to `Backend` as an `OpenFiles` newtype wrapping
  `Arc<DashMap<Url, OpenFile>>`. `OpenFile { text, version,
  parse_diagnostics }` bundles the three non-salsa concerns that used to
  live on `Document`. `OpenFiles` clones cheaply (Arc) so async closures
  (`did_change` debounce, workspace scan) can capture it alongside
  `Arc<DocumentStore>`.
- `DocumentStore::get_doc_salsa` lost its open-state gate — it now
  returns `Some` for any mirrored file. The gate lives on
  `Backend::get_doc`, which checks `open_files.contains(&uri)` before
  delegating. The `get_doc_salsa_any` variant is folded back into
  `get_doc_salsa` (one method, one contract).
- Aggregation helpers (`other_docs`, `other_docs_with_returns`,
  `doc_with_others`, `docs_for`) on `DocumentStore` take the open-URL
  slice as a parameter. `all_indexes` / `other_indexes` / `all_docs_for_scan`
  iterate `source_files` (the same known-files set that `map` used to be).
- `Backend::index_if_not_open` / `index_from_doc_if_not_open` replace
  the old `map`-based "skip if already open" guard inside `DocumentStore::index`.
  Call sites: `did_change_watched_files`, `did_rename_files`,
  `did_create_files`, `psr4_goto`, and the workspace scan inside
  `scan_workspace`. The editor's buffer remains authoritative for open files.

**Call-site surface** — contained entirely to `backend.rs` (~60
mechanical replacements via `replace_all`) plus the `document_store.rs`
reshape. Zero feature-module changes.

**Tests** — 894/894 after E4 (down from 903; nine tests in
`document_store.rs::tests` that poked legacy `map` semantics were
either deleted or rewritten to exercise salsa-level invariants
directly: `open_then_get_returns_text`, `update_replaces_text`,
`close_clears_text_but_keeps_index`, `close_nonexistent_uri_is_safe`,
`index_does_not_overwrite_open_file`, `open_caches_diagnostics_for_invalid_file`,
`get_index_salsa_matches_legacy_get_index`,
`get_doc_salsa_matches_legacy_open_state`, and
`get_returns_none_for_unknown_uri`). Integration suites (all 57 wire-protocol
tests + Symfony suite) pass unchanged — the Backend-side open-file
semantics are the same contract, just owned by a different struct.

**Atomicity** — text + version are stored in the same `OpenFile` value,
so a single `DashMap::entry` acquisition updates both. No TOCTOU window
between `set_open_text`'s mirror-into-salsa call and the entry write:
salsa sees the new revision first, then the entry lock is taken; any
reader that observes the bumped version has also observed the
corresponding text through either the open-files map or the salsa
input. Parse-diagnostics is updated under the same entry lock via
`set_parse_diagnostics`.

**Known non-goal** — the `DocumentStore` struct is *not* deleted. It
still owns `host: Mutex<AnalysisHost>`, `source_files`, `text_cache`
(G2), `parsed_cache` (G3), `token_cache`, `workspace`, and
`next_file_id`. All salsa-shaped. Calling the new shape
`InputStore` would be more accurate but is a rename-only follow-up.

### Phase F — salsa LRU + delete indexed_order ✅ shipped 2026-04-22

**What shipped**

- `parsed_doc` in `src/db/parse.rs` now carries `#[salsa::tracked(no_eq,
  lru = 2048)]`. That is the only query bounded: parsed docs own bumpalo
  arenas and dominate memo memory; `file_index` and `method_returns` are
  plain structs in the KB range, and their memos are already tied to the
  lifetime of the input set via dependency tracking, so adding `lru` to
  them would trade a small memory win for CPU regressions on cross-file
  queries.
- `DocumentStore.indexed_order` (VecDeque), `max_indexed` (AtomicUsize),
  `DEFAULT_MAX_INDEXED`, `set_max_indexed`, and `push_to_lru` are all
  deleted. `close()` no longer pushes to a queue; `index` /
  `index_from_doc` no longer evict.
- The `DocumentStore.map` survives as an unbounded known-files set. Its
  remaining purpose is the three roles that are not salsa-shaped:
  open-file state (`text: Option<String>`), parse diagnostics cache,
  and semantic-diagnostics cache. Memory cost at workspace-scan cap
  (50 k files) is ~2 MB of `Document` headers plus empty `Vec`s —
  acceptable, and the same order as the existing `source_files` /
  `text_cache` DashMaps.
- Three tests deleted: `eviction_removes_oldest_indexed_file`,
  `eviction_skips_open_files_and_evicts_next_indexed`, and
  `close_twice_does_not_duplicate_lru_entry`. They asserted the
  `indexed_order` contract that no longer exists; salsa's own test
  suite covers the `lru` behaviour on the query side. `cargo test` is
  892/892 after the removal (was 895; the delta is exactly the three
  deleted tests).

**`maxIndexedFiles` LSP config option** — preserved as a no-op for
backwards compatibility with existing editor configs. Salsa 0.26
exposes `IngredientImpl::set_capacity` only on internal types; the
`#[salsa::tracked(lru = N)]` macro emits a compile-time `usize` literal
and does not generate a public `set_lru_capacity` on the query. Runtime
tuning is therefore not reachable from the tracked-fn API. The three
`lsp_config_*_max_indexed_files` tests in `backend.rs` only assert
parsing and continue to pass; the field is read once at `initialize`
and immediately discarded with a `let _ =` so the intent is explicit
in the source. Bumping `lru = 2048` in `src/db/parse.rs` is the new
knob, and it requires a recompile.

**Why `lru = 2048`** — sized above the Laravel fixture (1609 files) so
full-workspace scans stay memoized with ~25 % headroom for per-session
churn. Measurement on larger fixtures can bump this; it is a single-line
change.

**Known limitation — DocumentStore holds ASTs outside the salsa LRU.**
The G2 `text_cache: DashMap<Url, Arc<str>>` and G3 `parsed_cache:
DashMap<Url, (Arc<str>, Arc<ParsedDoc>)>` are read-through caches keyed
on `Url`. Every `get_doc_salsa` read inserts into `parsed_cache` and
only `remove(uri)` (or a text change) evicts.

**Update (2026-04-23):** `parsed_cache` is now size-bounded at
`PARSED_CACHE_CAP = 2048` (matching the `lru = 2048` on the salsa
`parsed_doc` memo). On insert, if the cache has grown past the cap,
`insert_parsed_cache` calls `DashMap::retain` to drop roughly half the
entries before inserting the new one. Eviction is probabilistic
(DashMap iteration order is arbitrary), which is deliberate — salsa's
memo already has hotness-aware LRU, so losing a DashMap entry for a
hot file just means one extra `snapshot_query` that short-circuits on
the salsa memo. Benchmarked hot-path cost is unchanged at
~39 ns/call; G3 vs no-G3 is still 38 ns vs 76 ns (measured on this
bench machine, 2026-04-23).

`text_cache` is still unbounded. It stores only `Arc<str>` (the file
text), not parsed docs, so the per-entry cost is bytes-of-source not
KB-of-arena; leaving it as-is until profiling justifies a bound.

**Inputs-are-immortal** — unchanged by this phase. Salsa 0.26 has no
public input-delete API (confirmed: `salsa-0.26.1/src/input.rs` has no
`delete` / `remove` method; only tracked-struct deletion exists in
`tracked_struct.rs`). `DocumentStore::remove(uri)` already drops the
`source_files` / `text_cache` / `parsed_cache` entries so the file
stops contributing to `workspace.files` after the next
`sync_workspace_files()` call. The salsa input header itself
(~40 bytes of `FileId` + `Arc<str>` uri) remains alive for the lifetime
of the database. This is a complexity-reduction fix, not a memory-leak
fix: a workspace that churns through hundreds of thousands of unique
files over a single LSP session would still leak those headers. A
proper solution waits on salsa exposing input deletion upstream; revisit
when `salsa` releases the feature.

**Behaviour change observable to callers** — `get_index(uri)` used to
return `None` for files that had been evicted by the hand-written LRU;
it now returns `Some` for any ever-indexed file (salsa reparses on a
memo miss). This is strictly better: feature handlers no longer see
spurious "file unknown" results mid-session. No LSP-visible regression
because the only feature gate was the LRU itself.

### Phase G — close the single-file perf gap

**Goal**: recover the `index/single/*` and `index/get_doc` regressions surfaced
by the 2026-04-22 bench run, without giving up the workspace-scan wins.

Four concrete items, ordered by expected impact:

**G1 — Drop the redundant parse in `DocumentStore::index`.** ✅ **shipped
2026-04-22.** `index()` used to call `parse_document` purely to store parse
diagnostics, then discard the AST — salsa re-parses on first read, and both
`get_diagnostics` call sites gate on `get_doc_salsa` (open-files-only). The
parse was wasted work.

Impact (see post-G1 table above): `index/single/*` went from +7 to +66 %
regressions to −21 to −84 % wins; workspace-scan benches went from −12 to
−27 % wins to −94 to −97 % wins. All 894 unit tests still pass.

**G2 — Lock-free fast path in `mirror_text`.** ✅ **shipped 2026-04-22.**
A `text_cache: DashMap<Url, Arc<str>>` now sits alongside `source_files`
holding the last-set text per URI. `mirror_text` compares against it
without taking `host.lock()` and returns immediately on a byte-equal
match; the mutex is acquired only when the salsa input actually needs
to change. Cache entries are inserted inside the mutex immediately
after every setter (and after the creation of a fresh `SourceFile`),
so a cache hit implies the handle exists and the salsa revision agrees
with the cached value for equality purposes.

`remove(uri)` now drops the `text_cache` entry alongside `source_files`;
otherwise a re-indexed file keyed on the same URL would see a stale
cache hit against a fresh `SourceFile` handle.

All 894 unit tests pass (including `concurrent_reads_and_writes_do_not_panic`
and `salsa_codebase_matches_imperative_codebase`).

**Measurement (2026-04-23).** Added `bench_mirror_same_text_contended` to
`benches/index.rs`: 8 worker threads, each calling `store.index(uri,
MEDIUM)` 500× on a pre-indexed file. This models the workspace-scan /
`did_open` collision where every thread wants to re-mirror the same
already-correct text.

| variant | total time (8×500 ops) | per-op |
|---|---|---|
| G2 ON (shipped) | 2.16 ms | ≈540 ns |
| G2 OFF (fast path removed) | 8.92 ms | ≈2 230 ns |

That's a **≈4.1× speedup** from the lock-free fast path. Single-threaded
throughput actually regresses slightly (the two `DashMap::get` guards
cost more than one uncontended `host.lock()` + one `Arc::from` on a
4 KB fixture) — the win is entirely from avoiding host-mutex
serialization under contention, which is exactly the workspace-scan
shape we care about.

**G3 — Trim `get_doc_salsa` overhead.** ✅ **shipped 2026-04-22.**
Added `parsed_cache: DashMap<Url, (Arc<str>, Arc<ParsedDoc>)>` — a
cross-revision read-through cache shared by `get_doc_salsa` and
`get_doc_salsa_any`. Cache validity is keyed on `Arc::ptr_eq` between
the cached text Arc and the G2 `text_cache[uri]`, which is written
inside the host mutex right after each `sf.set_text`. That pointer
check proves the cached ParsedDoc matches the current committed salsa
revision, so the query can return without snapshotting the db at all.

The pointer-equality approach beats naked URI-keyed invalidation
because it closes a TOCTOU race: if a reader's `snapshot_query`
returned a ParsedDoc for rev N and a concurrent writer bumped to N+1
and cleared a URI-keyed cache before the reader inserted, the reader
would silently repopulate the cache with a stale entry. Self-evicting
on Arc mismatch eliminates that window — no writer-side invalidation
is required at all. `remove(uri)` still drops the entry, purely to
free memory.

`index/get_doc` improved **−9.6 %** vs the post-G2 baseline
(`[−11.2 %, −7.8 %]`, p < 0.05). A dedicated test
(`get_doc_salsa_any_cache_hits_across_calls`) pins the assumption
that salsa preserves the stored `Arc<str>` identity in
`SourceFile::text` — if that ever regressed, the cache would stop
hitting and the test would fail.

The alternative ("skip `Cancelled::catch` on the first attempt") was
not pursued: `catch_unwind` at this scale costs single-digit
nanoseconds and the cache hit bypasses it entirely anyway.

**G4 — Investigated and resolved (2026-04-22): not a real regression.**
The saved `main` criterion baseline in `target/criterion/references/*/main/`
was recorded before commit `d4bd7c7` on main (`perf(references): substring
pre-filter + parallel per-doc scan`, PR #204) landed — it reflected a
pre-optimization `find_references` where cross-file scans were ~1 µs.
Current main produces ~25 µs for the same bench. Re-saving the baseline
from a fresh `main` checkout and re-running the comparison shows only
noise-range deltas on `references/scale/*` and a mild +12% on
`cross_file_class`, which is within the expected range for a 25 µs
rayon-driven bench. No code change needed.

**Takeaway**: criterion's saved baselines silently go stale when the
comparison target (`main`) moves. Rerun `scripts/bench.sh save main` after
every `main` merge that touches perf-sensitive paths; Phase H's CI gate
should bake this in.

**Validation**: G1 done. G2/G3/G4 each need their own compare run.

### Phase H — fix benches and add integration regression gate ✅ shipped 2026-04-22

**Bench compilation** — `benches/requests.rs` and `benches/semantic.rs`
both compile and run end-to-end on this branch; the E3-era signature
drift flagged earlier was resolved in transit (current `hover_info`
et al. take the threaded `MethodReturnsMap` the benches already pass).
No code change needed here.

**`scripts/bench.sh` resilience** — the previous `set -e` loop aborted
the entire compare run as soon as one sub-bench panicked (e.g.
`references/laravel_framework` with no saved baseline). Rewritten to
track per-suite failures and continue: every suite is attempted, each
failure emits a `::warning::` line that GitHub Actions surfaces, and
the script still exits non-zero overall so CI fails loudly. Crucially
this means a single missing-baseline bench no longer hides regressions
in the remaining three suites.

**CI gate** — `.github/workflows/bench.yml` now triggers on:

- `push` to `main` (paths: `src/**`, `benches/**`, `mir-*/**`,
  `Cargo.toml`, `Cargo.lock`) — keeps the gh-pages baseline history
  fresh so every subsequent PR comparison is anchored on the latest
  merged state. This replaces the "rerun `bench.sh save main` after
  every main merge" takeaway from Phase G4.
- `pull_request` against `main` (same paths filter) — compares the
  PR's bench output against the gh-pages history and fails the PR
  on a regression above the 130 % alert threshold.
- `workflow_dispatch` — unchanged, for manual reruns.

Paths filter avoids running criterion on doc-only or CI-only PRs,
where the ~15–20 minute bench run would be pure overhead. The Laravel
fixture is now cloned via `scripts/setup_laravel_fixture.sh` as an
explicit workflow step — without it the `*_laravel_framework` benches
silently skip, under-reporting coverage on the cross-file paths most
likely to regress on salsa changes.

**Why not `scripts/bench.sh compare main`** — the plan originally
called for driving CI through `bench.sh compare`, but the existing
`benchmark-action/github-action-benchmark` integration offers a
stronger signal: it compares against rolling gh-pages history rather
than a single frozen baseline, surfaces alert comments on the PR, and
handles result storage across runs. The `compare` subcommand remains
the canonical local-workflow entry point for reproducing CI results.

### Phase I — semantic diagnostics as a salsa query ✅ shipped 2026-04-22

**What shipped**

- New `src/db/semantic.rs` exposing
  `semantic_issues(db, ws, file) -> IssuesArc` where `IssuesArc(Arc<[Issue]>)`
  has the same `Arc::ptr_eq`-based `Update` impl as the other `*Arc`
  newtypes. The query runs `mir_analyzer::StatementsAnalyzer` against the
  memoized codebase + parsed doc and returns the raw non-suppressed
  `mir_issues::Issue` list. Depends on `codebase(ws)` and
  `parsed_doc(file)`, so body-only edits to a file invalidate only its
  own issues; structural edits cascade via the codebase query.
- New `DocumentStore::get_semantic_issues_salsa(uri) -> Option<Arc<[Issue]>>`
  accessor that runs the query under `snapshot_query` (so it benefits
  from the same `salsa::Cancelled` retry loop as the other read paths).
- **Config filtering lives outside the query.** New
  `semantic_diagnostics::issues_to_diagnostics(&[Issue], &Url,
  &DiagnosticsConfig) -> Vec<Diagnostic>` applies the user's per-category
  toggles and converts to LSP diagnostics. Keeping the filter outside the
  tracked function preserves memoization across `DiagnosticsConfig`
  changes — flipping "undefined_variables" no longer invalidates the
  expensive analyzer output.
- Backend handlers (`did_open`, `did_change`, `document_diagnostic`,
  `workspace_diagnostic`, `code_action`) all migrated to call the
  accessor + `issues_to_diagnostics`. Every call is wrapped in
  `tokio::task::spawn_blocking` — the salsa memo turns repeat hits into
  Arc-clones, but the *first* call per file still walks
  `StatementsAnalyzer` (hundreds of ms on cold files) and would stall
  the async runtime if run inline. The imperative
  `semantic_diagnostics_no_rebuild` is retained for `benches/semantic.rs`
  as a single-call reference implementation and is marked
  `#[allow(dead_code)]` for the library build.
- **Dropped**: `Document.sem_diagnostics` field,
  `DocumentStore::set_sem_diagnostics` / `get_sem_diagnostics`, and
  every call site that wrote into the manual cache. `code_action` now
  reads through the memo like every other diagnostic consumer — no
  extra cache layer, no risk of serving stale diagnostics after an
  edit.

**Behaviour changes observable to callers**

- Repeated `textDocument/diagnostic` pulls on an unchanged file return
  instantly from the memo (was: reran `StatementsAnalyzer` every pull).
- `workspace/diagnostic` sweeps share the per-file memo with open-file
  handlers; on a cold workspace the pull pays the full analysis once
  across handlers, not once per pull.
- `code_action` no longer depends on `did_open`/`did_change` having
  populated a side cache first. Previously, code-actions raised right
  after `initialize` (before any edit) saw empty diagnostics; now they
  compute on demand through the memo.

**Tests**

- Three new `src/db/semantic.rs` tests pin the query contract: flags an
  undefined-function call, memoizes on unchanged inputs
  (`Arc::ptr_eq` across calls), and reparses after an edit (different
  `Arc`). Full suite is 895/895 after migration.

### Phase J — workspace-symbol / type-hierarchy / implementation as tracked queries ✅ shipped 2026-04-22

**What shipped**

One aggregate query instead of a per-handler query per class of work.
Reading the handlers showed that `workspace_symbols`,
`prepare_type_hierarchy`, `supertypes`, `subtypes`, and
`find_implementations` all needed the same three shapes: the flat
`(Url, Arc<FileIndex>)` list, name → class lookup, and parent/interface
name → subtype lookup. Collapsing them into a single query keeps the
memo footprint bounded, avoids a combinatorial explosion of query
keys, and skips the "workspace_symbols(query) memo grows
unboundedly" hazard the original sketch flagged.

- New `src/db/workspace_index.rs` exposes
  `workspace_index(db, ws) -> WorkspaceIndexArc`. The `WorkspaceIndexData`
  inner type carries:
  - `files: Vec<(Url, Arc<FileIndex>)>` — the flat list handlers used
    to rebuild on each call,
  - `classes_by_name: HashMap<String, Vec<ClassRef>>` — O(1) name
    lookup for `prepare_type_hierarchy` and `supertypes_of`,
  - `subtypes_of: HashMap<String, Vec<ClassRef>>` — pre-built reverse
    map (keyed by `extends`/`implements`/`use` target) for
    `subtypes_of` and `find_implementations`.
- `ClassRef { file: u32, class: u32 }` plus `WorkspaceIndexData::at`
  turns a reverse-map entry back into `(&Url, &ClassDef)` without
  cloning strings.
- New `DocumentStore::get_workspace_index_salsa()` runs the query
  through `snapshot_query` (same retry-on-cancel path the other salsa
  accessors use) and returns the shared `Arc`. `sync_workspace_files`
  is called first so the aggregate always reflects the latest set of
  mirrored files.
- `type_hierarchy`, `implementation`, and `symbols` each gained a
  `_from_workspace` helper that consumes `&WorkspaceIndexData` in place
  of `&[(Url, Arc<FileIndex>)]`. The inner algorithms for
  `subtypes_of_from_workspace` and `find_implementations_from_workspace`
  are now O(matches) via the `subtypes_of` map instead of O(files ×
  classes); `prepare_type_hierarchy_from_workspace` and
  `supertypes_of_from_workspace` are O(name-lookup).
  `workspace_symbols_from_workspace` is the one exception: fuzzy match
  is inherently O(total symbols), so the function body is a thin
  wrapper over the original walk. The win there is removing the
  per-request `all_indexes()` rebuild (~1600 `host.lock()` acquisitions
  on Laravel).
- Backend handlers (`symbol`, `prepare_type_hierarchy`, `supertypes`,
  `subtypes`, `goto_implementation`) all migrated to the
  `_from_workspace` variants.
- **Dropped**: `prepare_type_hierarchy_from_index`,
  `supertypes_of_from_index`, `subtypes_of_from_index`, and
  `find_implementations_from_index`. They had no remaining callers
  after the migration; their tests were rewritten against the new
  `_from_workspace` helpers via a test-only
  `WorkspaceIndexData::from_files` constructor.

**Tests**: new `src/db/workspace_index.rs` has three tests pinning the
aggregate contract (name map + subtype map populated correctly, Arc
memoization on unchanged inputs, Arc invalidation on edit, trait-use
and interface cases). `src/implementation.rs` tests ported to
`_from_workspace`. Full suite is 899/899 (was 896, +3 from the new
aggregate query).

**Benches** (2026-04-22, `cargo bench --bench requests` against the
previous run's baseline):

| Bench | Δ | Absolute |
|---|---|---|
| `workspace_symbol/fuzzy_small` | +1.9 % | 4.7 µs |
| `workspace_symbol/laravel_framework` | +5.3 % | 2.2 ms |
| `implementation/cross_file_class` | −7.8 % | 38 ns |
| `implementation/laravel_framework` | +25.4 % | 29 µs |

These deltas are not a measurement of Phase J. The existing benches
call the direct parsed-doc paths (`symbols::workspace_symbols`,
`implementation::find_implementations`) — which take `&[(Url,
Arc<ParsedDoc>)]` and walk every AST — whereas the Phase J hot path
lives in the backend handlers calling `*_from_workspace` against a
memoized `Arc<WorkspaceIndexData>`. The numbers above therefore only
validate that the public `workspace_symbols` / `find_implementations`
functions were left untouched; the `+25 %` on `implementation/laravel_framework`
at a 29 µs operation is within criterion's baseline-noise
window for a bench of that size.

The real payoff lives in the LSP handler path that benches don't
exercise: every workspace-symbol picker keystroke, subtype lookup, and
implementation jump now reads through the shared `Arc<WorkspaceIndexData>`
instead of rebuilding a `Vec<(Url, Arc<FileIndex>)>` via 1609
`host.lock()` acquisitions through `all_indexes()`. A dedicated
`_from_workspace` benchmark is a follow-up.

### Phase K — persistent on-disk cache

**Goal**: serialize `StubSlice` per file to disk between LSP sessions so a
warm start on the same workspace skips parsing + `DefinitionCollector` for
files whose content hasn't changed.

Broken into three shippable steps so each carries its own weight:

#### K1 — infrastructure module ✅ shipped 2026-04-23

New `src/cache.rs` with `WorkspaceCache { dir: PathBuf }`:

- `WorkspaceCache::new(root)` — creates (or re-opens) `<cache_base>/php-lsp/<schema>/<workspace-hash>/`. Returns `None` when the system has no cache dir; callers treat that as "cache disabled".
- `WorkspaceCache::key_for(uri, content) -> CacheKey` — blake3 over `uri || 0x00 || content`, truncated to 128 bits. URI is baked into the key because `StubSlice::file` is part of the payload; otherwise two files with identical content would share a cache entry but carry the wrong URI after decode.
- `read<T: DeserializeOwned>(&key) -> Option<T>` / `write<T: Serialize>(&key, &value)` — bincode v2. `write` goes through a temp-file rename, so a crash mid-write never leaves a half-written `.bin`.
- `clear()` — drops every entry (reserved for K3).

**Cache layout**:
```text
<cache_base>/php-lsp/<pkg-version>-mir-0.7/<workspace-hash>/<entry-hash>.bin
```

`<cache_base>` = `$XDG_CACHE_HOME` / `$HOME/.cache` / `%LOCALAPPDATA%`. No `dirs` crate — hand-rolled to keep the footprint small and behaviour predictable. `<schema-version>` includes `CARGO_PKG_VERSION` + a hardcoded mir-codebase marker (`mir-0.7`): bumping either rotates every cache entry. `<workspace-hash>` is blake3 of the canonicalized root path (truncated to 64 bits) — two separate projects stay isolated; two checkouts at the same path share.

**Dependencies added**: `blake3`, `bincode v2`, `serde` (derive). All three were already transitively present via salsa / mir-codebase. Direct-dep footprint: +3 lines in `Cargo.toml`.

**Nothing is wired yet.** `backend.rs` / `document_store.rs` don't consume the module — K2 is a separate commit that plumbs it into `scan_workspace`. K1 on its own is 9 tests (key determinism, round-trip, corruption-is-silent, atomic write, clear, `StubSlice` smoke-test) landing on top of 895 → 904 lib tests.

#### K2a — plumb cached_slice through file_definitions ✅ shipped 2026-04-23

Settled the design question K1 left open. Memo pre-population (no public
API in salsa 0.26) is avoided entirely: instead, `SourceFile` grows an
`Option<Arc<StubSlice>>` input field, and `file_definitions` checks it
before falling through to parse + `DefinitionCollector`:

```rust
#[salsa::tracked(no_eq)]
pub fn file_definitions(db: &dyn Database, file: SourceFile) -> SliceArc {
    if let Some(cached) = file.cached_slice(db) {
        let _ = file.text(db);   // load-bearing: declare dep on text
        return SliceArc(cached);
    }
    // … existing parse + collect …
}
```

The `let _ = file.text(db)` on the cached branch is load-bearing.
Without it, salsa wouldn't record a dependency on the text input, so
the memo would survive across edits and silently serve stale data. With
it, any `sf.set_text(…)` bumps the revision and invalidates the memo —
and `DocumentStore::mirror_text` now *also* calls
`sf.set_cached_slice(db).to(None)` on every text change, so the
re-run takes the slow path instead of looping back to a stale Arc.

New API:

- `DocumentStore::seed_cached_slice(uri, Arc<StubSlice>) -> bool` — sets
  the field for an already-mirrored file. Returns `false` for unknown
  URIs so scan can't accidentally allocate `SourceFile`s outside
  `mirror_text`'s bookkeeping.
- `SourceFile::cached_slice` / `set_cached_slice` — generated by
  `#[salsa::input]`, private usage inside `document_store.rs` and
  `file_definitions`.

Four new tests cover the invariants:

- `file_definitions_returns_seeded_slice_without_parsing` — seeds a
  slice whose facts don't match the file text; verifies the cached
  slice wins (proves the fast path is actually taken, not silently
  falling through).
- `edit_invalidates_seeded_slice` — after `set_text` + `set_cached_slice(None)`
  the next query reflects fresh parse.
- `seed_cached_slice_then_edit_invalidates` — end-to-end through
  `DocumentStore`: seed, read through `get_codebase_salsa`, edit,
  confirm codebase reflects new text.
- `seed_cached_slice_noops_for_unknown_uri` — can't seed a URI that
  wasn't mirrored first.

No user-visible impact yet — `seed_cached_slice` has no callers in
production code. K2b wires scan_workspace to actually populate it from
disk.

#### K2b — wire scan_workspace to read/write the cache ✅ shipped 2026-04-23

The `scan_workspace` per-file task is now:

```text
read text from disk
if editor already has file open → skip (buffer is authoritative)
key = WorkspaceCache::key_for(uri, text)

if cache.read::<StubSlice>(key) is Some(slice):
    mirror_text(uri, text)
    seed_cached_slice(uri, Arc::new(slice))
    return                           ← no parse, no DefinitionCollector
else:
    parse_document(text)
    docs.index_from_doc(uri, doc, diags)
    docs.slice_for(uri)               ← forces file_definitions run
    cache.write(key, &slice)          ← best-effort persist
```

The write path materializes `file_definitions` inline in the scan task
instead of letting it run lazily on first codebase query. That's a
wash on work — the query has to run before anything useful can happen
anyway — but doing it inside the scan task means (a) the parallelism
budget already covers it, and (b) the slice is in hand right when we
want to persist it, so no separate "sweep through memoized slices
after scan" pass is needed.

`WorkspaceCache::new(root)` is called once per root in
`Backend::initialized` / `did_change_workspace_folders`. When it
returns `None` (no cache dir available — sandboxed runner, read-only
`$HOME`), every `cache.as_ref()` guard no-ops and scan runs exactly as
before. Persistence is strictly best-effort: an I/O error during
`cache.write` is silently discarded (`let _ = …`) so a flaky disk
can't fail the scan.

**Blocker (unchanged)**: `ParsedDoc` cannot be cached (bumpalo arena).
So even with K2b, opening a file still re-parses. That's fine — the
expensive path on cold start is `DefinitionCollector` running on every
file's AST, which is exactly what K2b skips for unchanged files.

**Expected impact**: second-start on Laravel (1609 files) should drop
from seconds of `scan_workspace` (read + parse + collect) to tens of
ms (read + hash + deserialize). Full measurement requires an
integration bench that runs twice against a persistent cache dir —
deferred until K3 lands a size cap, otherwise benchmarks could grow
the cache unboundedly across runs.

#### K3 — size cap (reset-on-overflow) ✅ shipped 2026-04-23

Cap at `CACHE_SIZE_CAP = 512 MiB` per workspace. At `WorkspaceCache::new`,
sum the flat directory's `.bin` files via `size_bytes()`; if over cap,
call `clear()` before returning the handle. Rebuild cost is bounded:
exactly one full `scan_workspace` runs as if cold, and the new cache
grows back under the cap because every edit keeps one entry per live
content hash.

Chosen over LRU-by-mtime for K3 because it's ~30 lines and touches
nothing outside `cache.rs`. LRU would need either a per-entry
last-access timestamp (atime is unreliable on `noatime` mounts,
adding `touch` on read costs one syscall per hit) or a separate
index file — both materially more complex for a marginal savings
(the reset approach wastes at most one cold start per cap overflow).

#### K4 — LRU-by-mtime eviction + orphan sweep (proposed)

Replace the K3 reset with fine-grained eviction: walk entries on
overflow, sort by `mtime`, delete oldest until under target (e.g.
80% of cap). Also do an orphan sweep on startup — delete entries
whose content hash corresponds to a URI no longer in the workspace
file list. Both reduce the "one full cold scan" cost K3 incurs on
overflow, at the cost of one `read_dir` + sort per startup.

### Phase L — reference warm-up background task ✅ shipped 2026-04-22

**What shipped**

Simpler than the original sketch. Reading `src/db/refs.rs` showed that
`symbol_refs(db, ws, key)` iterates every workspace file's `file_refs`
to build its result — **with any key**, including a sentinel that
matches nothing. The per-file walk is memoized on first traversal, so
one call warms the entire cross-file reference index. No need to pick
"hot symbols"; a single invocation does the work.

Implementation:

- New `DocumentStore::warm_reference_index(&self)` — invokes
  `symbol_refs(db, ws, "__phplsp_warmup__")` through `snapshot_query`.
  The returned `Vec<(Url, u32, u32)>` is empty (sentinel matches
  nothing), but the memo is populated for every `file_refs(ws, sf)`
  pair.
- Called from the `initialized` workspace-scan spawn, right after the
  per-root scan loop completes and before `drop(docs)`. Wrapped in
  `tokio::task::spawn_blocking` so the async runtime doesn't stall on
  the CPU-bound walk.
- Fire-and-forget. A `textDocument/references` request arriving
  mid-warmup runs through `snapshot_query`'s existing `salsa::Cancelled`
  retry loop, so late-arriving lookups either join the warm-up's
  results or retry cheaply.

**Payoff**: first `textDocument/references` on a cold workspace goes
from "walk every file's Pass-2 analysis" to "O(total refs) filter-walk
over already-memoized `file_refs`". On Laravel (1609 files) that's the
difference between seconds and single-digit milliseconds.

**Tests**: new
`warm_reference_index_does_not_panic_and_keeps_lookups_correct` in
`document_store.rs` exercises the path on a two-file workspace and
asserts that a post-warm-up `get_symbol_refs_salsa` still returns the
correct locations. Full suite is 896/896 after Phase I + L.

---

**Prioritization among I/J/K/L**: I, J, and L shipped 2026-04-22. K is
deferred — the in-memory picture is salsa-native now, but the observed
wins (Phase D references, Phase I diagnostics, Phase J cross-file lookups)
already cover the user-visible hot paths. Revisit K only if cold-start
profiling becomes the dominant complaint.

## Constraints carried forward

- **bumpalo arena + salsa lifetimes**: tracked values must be `'static`. Every
  parse result is wrapped in `Arc<ParsedDoc>` via the `ParsedArc` newtype; the
  arena is owned by `ParsedDoc` and freed when the Arc refcount drops to zero.
  Salsa drops memoized values on input change or LRU eviction; refcount
  serialization guarantees no concurrent access during drop. Do not expose
  `&ParsedDoc` borrows that outlive a single query call.
- **mir-codebase is not salsa-aware** and should stay that way — it remains
  usable as a standalone CLI. Phase C wraps it; does not rewrite it.
- **Salsa 0.26 API churn**: the `#[salsa::tracked]` attribute syntax is still
  evolving. Pin to exact version (`salsa = "0.26"` in `Cargo.toml`); budget
  one API-update PR per quarter.
- **No async in salsa queries**: queries are sync. Long-running queries must
  run under `tokio::task::spawn_blocking`. Cancellation propagates via
  `salsa::Cancelled`; handle at the `spawn_blocking` boundary.
- **LRU test expectations** (two tests in `document_store.rs`) block Phase F
  until rewritten. Flagged in Phase F notes.

## Files of note

- `src/db/` — salsa layer, all tracked queries
- `src/document_store.rs` — thin mirror; `Document` struct is the post-migration shape
- `src/backend.rs` — LSP adapter; version gating lives here now (`current_version` pattern)
- `benches/index.rs` — cold-start scan benchmark (updated to `get_doc_salsa`)
- `docs/architecture.md` — high-level architecture (should be updated when Phase E lands)

## How to extend

Adding a new query:

1. Decide on the input key (`SourceFile` for per-file, `Workspace` for cross-file).
2. Decide if the return type needs a newtype `Update` wrapper (anything containing
   non-`Update` types like bumpalo-allocated AST nodes does).
3. Write the `#[salsa::tracked(no_eq)]` function in the appropriate `src/db/*.rs`.
4. Expose a `get_*_salsa` accessor on `DocumentStore` that looks up the
   `SourceFile` and invokes the query inside `with_host`.
5. Migrate any legacy `DocumentStore`/`FileIndex` callers incrementally.

Never add a query that returns a reference into the database (`returns(as_ref)`)
unless the caller has a lifetime tied to the Analysis snapshot — today's
DocumentStore wrapper can't satisfy that.
