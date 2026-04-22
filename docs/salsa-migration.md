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
| B4d-4 | Delete `OnceLock<MethodReturnsMap>` from `ParsedDoc` | ⏸ deferred — see Phase E3 |
| C | Migrate `mir_codebase` into salsa queries | ✅ shipped |
| D | `file_refs`/`symbol_refs` lazy reference index | ✅ shipped |
| E1 | Snapshot-clone reads off the host mutex | ✅ shipped |
| E2 | LSP request cancellation → `RequestCancelled` | ⏸ folded into E1 — `snapshot_query` retries on `salsa::Cancelled` and falls back to the mutex; nothing escapes to the LSP layer |
| E3 | Thread salsa db through `type_map`; delete `OnceLock<MethodReturnsMap>` | ⏳ pending — 35 call sites to update |
| E4 | Move `DocumentStore.map` bookkeeping to `Backend`; delete the struct if empty | ⏳ pending (optional cleanup) |
| F | `#[salsa::tracked(lru = N)]`; delete `indexed_order` | ⏳ pending (blocked by inputs-are-immortal problem) |

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
- E3: thread salsa-db access through `type_map` and delete `OnceLock`. (~1 PR, large — touches 35 call sites.)
- E4: move `DocumentStore.map` bookkeeping to `Backend`; delete the struct if anything remains. (cleanup, optional.)

Don't treat Phase E as a single PR. It's a phase with four independent sub-PRs.

### Phase F — salsa LRU + delete indexed_order

**2026-04-22 note**: Phase F alone does not solve memory growth. Salsa's per-query LRU only evicts memoized *outputs*; salsa *inputs* (`SourceFile` handles stored in `DocumentStore.source_files`) are immortal for the life of the database. A workspace that churns through many files accumulates inputs forever. Phase F needs to be paired with explicit input removal (salsa 0.26 supports deleting inputs, but the pattern and correctness implications need design work). Not a quick win.


**Goal**: replace the hand-written `indexed_order: Mutex<VecDeque<Url>>` eviction with salsa's per-query LRU.

```rust
#[salsa::tracked(no_eq, lru = 512)]
pub fn parsed_doc(db: &dyn Database, file: SourceFile) -> ParsedArc { … }
```

Removes `indexed_order`, `max_indexed`, `set_max_indexed`, `push_to_lru`, and the
associated eviction tests. Requires measurement on a large fixture to pick `N`.

**Dependency**: the current LRU tests (`eviction_removes_oldest_indexed_file`,
`eviction_skips_open_files_and_evicts_next_indexed`) assert a contract that
moves to salsa. They must be rewritten or deleted as part of Phase F.

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
