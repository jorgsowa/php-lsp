# Changelog

All notable changes to php-lsp are documented here.

## [Unreleased]

## [0.25.1] — 2026-08-22

### Fixed

- **Stale completion analysis is now canceled**: completion requests no longer let outdated analysis work continue after newer edits arrive, preventing stale background work from competing with current requests.
- **`didOpen` no longer deadlocks during the initial workspace scan**: opening a document while startup indexing is still in flight now completes reliably instead of getting stuck behind the initial scan.

## [0.25.0] — 2026-08-21

### Added

- **Use imports included in find references**: imported symbols (e.g., from `use` statements) are now included when resolving references across the workspace, matching PHP's scope resolution behavior.

### Fixed

- **Companion-file index-lag race on references**: references and related lookups now wait for mir's own per-file resolution with a dedicated retry path after detecting an empty first result, ensuring declarations in recently opened companion files are found reliably instead of falling back to AST heuristics or being lost.
- **PHPDoc navigation false positives**: resolved incorrect resolution targeting PHPDoc tokens (tag names, `@template` parameters, `@param`/`@var`/`@property*`/`@method` variable syntax) that could previously resolve to unrelated workspace symbols.
- **Nested local variable reference scoping**: fixed incorrect scoping of references in nested scopes (closures, anonymous functions, loops) where outer-scope variables were incorrectly included in or excluded from result sets.
- **Non-ASCII declaration range boundaries**: the fallback declaration-range width construction in `handle_references` previously used UTF-8 byte counts instead of LSP-compliant string spans, causing definition ranges for multi-byte/non-ASCII identifiers to extend past the actual identifier boundary. Now uses correct token-length calculations aligned with MIR's UTF-16 column data.
- **Unicode and CRLF reference range conversion**: fixed offset calculations for reference span widths in files containing non-BMP Unicode characters or CRLF line endings, ensuring ranges never drift from their intended positions.
- **MIR column-to-offset panic**: resolved a panic when MIR's reported source column landed mid-character (e.g., on multi-byte UTF-8 sequences) during `mir_reference_line_column_to_offset` — now clamps to valid character boundaries safely.

### Performance

- **Responsive local variable references**: offloaded blocking navigation lookups (including local variable resolution) to prevent blocking the LSP request loop under concurrent edits, keeping completions and references responsive while index builds are in-flight.
- **Optimized references partial-result final pass**: reduced unnecessary work in the final pass of large-scale reference queries by short-circuiting already-resolved files.

### Dependencies

- **mir bumped to 0.72**: adopted the updated MIR dependency with API changes including column offset reporting aligned with UTF-16 LSP conventions (see `Fix` section above).

## [0.24.1] — 2026-08-11

### Maintenance

- **Re-cut the release**: the tag-triggered Release workflow run for v0.24.0 got stuck queued with no jobs ever starting. No functional changes; retagging to get a clean pipeline run through build, crates.io publish, and the GitHub release.

## [0.24.0] — 2026-08-06

### Added

- **Laravel Vite asset resolution**: `vite('path')` and `Vite::asset('path')` resolve against `public/build/manifest.json` for go-to-definition, hover, completion, and document links.
- **Laravel Mix asset resolution**: `mix('path')` resolves against `public/mix-manifest.json` the same way.
- **Laravel validation rule-name completion**: built-in rule names (`required`, `email`, `max`, ...) complete inside `->validate([...])`, `Validator::make([...], [...])`, and a `rules()` method's return array, in both pipe-delimited and array form.
- **Laravel middleware groups**: `$middleware->group('name', [...])` registrations in `bootstrap/app.php` are now indexed, so `Route::middleware('name')` resolves to the group registration.
- **Laravel translations**: `lang/` is now scanned recursively (previously only direct children of each locale directory), and a `group.item` miss with an existing `<group>.php` array file now offers a quick-fix that inserts the missing key.
- **Laravel routes**: `routes/` is scanned recursively, and `Route::resource()`/`apiResource()` calls synthesize their implicit CRUD route names, respecting an enclosing `as` prefix.
- **Laravel config**: `config/` is scanned recursively, deriving dotted key prefixes from nested paths (e.g. `config/services/stripe.php` → `services.stripe`).
- **`debugStats`**: reports per-cache entry counts (token/text/parsed/analysis/owned-program/decl-fingerprint/vendor-index) plus workspace file count, for diagnosing memory growth without an external profiler.

### Fixed

- **Promoted constructor property symbols** now report correct output.
- **Type hierarchy subtype ordering is now deterministic.**
- **Cross-file promoted property definition lookup** now resolves correctly.
- **Global interface names are preserved in fallback hover** instead of being dropped.
- **FQCNs are preserved through type-hierarchy flows.**
- **Diagnostics are now composed and ordered consistently across every publish path**: the cross-file republish path fed mir's raw parse errors straight to the client, bypassing the same-issue exclusion the primary path applies, and merged external diagnostics before semantic/Laravel diagnostics instead of after; the batched dependent-file publish path also omitted Laravel's unguarded-model diagnostic entirely.
- **A cursor on a semi-reserved keyword** (`final`, `list`, `class`, `public`, ...) used as a method/property/constant name no longer falls through to an ungated workspace lookup that could resolve to an unrelated same-named symbol, across go-to-definition, go-to-declaration, hover, and document-highlight/linked-editing-range.
- **A cursor on a PHPDoc-only token** (a tag name, a `@template` parameter, or the `$var` half of `@param`/`@var`/`@property*`/`@method`) is now gated the same way, instead of resolving to an unrelated same-named workspace symbol.
- **The PHP keyword list is now sourced from `php-lexer`/`php-ast`** instead of a hand-maintained list that was missing `__halt_compiler`.
- **References/goto-definition/hover on a builtin global constant** (`PHP_EOL`, `PHP_VERSION`, ...) now excludes vendor from the candidate scope, matching the existing class/function/method builtin-scoping fix.
- **A leaked on-disk cache file per edited revision**: the cache is now keyed on URI hash alone (with the content hash validated from the entry), so editing a file overwrites its existing slot instead of leaving the prior revision to rot until the size cap forces a full wipe.

### Performance

- **Startup no longer stalls behind synchronous cache-directory pruning**: on a long-lived dev cache, the old schema-version `remove_dir_all` could cover millions of files and delay `indexReady` by 20+ minutes. Now runs on a background thread.
- **A warm-started file with an unresolved reference posting is now reanalyzed in the background** instead of on the first query that touches it: first-touch query cost on a real Laravel fixture drops from ~4-9ms to ~76µs.
- **Reference and reachability queries now answer from mir's persistent per-file mention index** instead of a per-query Aho-Corasick scan, with full query results memoized per revision; a file scanned by a code action now answers a later reference query for free, and vice versa. Drops the direct `aho-corasick` dependency.

### Changed

- **Symbol lookups migrated off the legacy per-file `FileIndex` onto mir's own indexes**: type hierarchy, hover, signature help, completion docs, background navigation, and importer resolution now resolve through mir postings/mention index. Several now-redundant `FileIndex` fields and accessors were retired accordingly.

### Maintenance

- **Release workflow**: added an `aarch64-unknown-linux-musl` release target (cross-compiled via `cross-rs`), plus a CI concurrency group and a docs fix for the release-target download table.

### Dependencies

- **mir updated to 0.70.1** (from 0.67.0): retires `classes_by_name`/`subtypes_of`/`decls_by_name` in favor of the mention index, adds `is_builtin_constant`, and rolls up the 0.68–0.70 line of fixes and performance work this release depends on.

## [0.23.0] — 2026-08-03

### Added

- **Laravel Blade template support**: a Blade lexer covers `{{ }}`/`{!! !!}` expressions, `{{-- --}}` comments, `@{{ }}` literal escapes, view/Livewire-referencing directives (`@include`, `@includeIf`, `@extends`, `@each`, `@component`, `@livewire`), and `<x-component>`/`<livewire:name>` tags. Helper calls (`route()`/`view()`/`config()`/`asset()`/`env()`/`trans()`/`->middleware(...)`) inside expressions get definition/hover/completion by reparsing each expression as a standalone PHP snippet. New `ComponentIndex`/`LivewireIndex` cover class-based component/Livewire fallback when no matching Blade view exists.
- **Laravel hover, document links, asset and middleware support**: hover and document-link coverage for the existing `env`/`config`/`view`/translation/`route` string-key domains, plus two new domains — `asset()` completion/hover/definition/links (`public/` file index) and `->middleware('alias')`/`Route::middleware([...])` completion/hover/definition/links (aliases sourced from `bootstrap/app.php` or the legacy `Kernel.php`).
- **`stubDirs` config option**: load user-supplied PHP stub directories (e.g. for extensions or frameworks the bundled stubs don't cover) as an additional, highest-precedence symbol source, alongside `.php-lsp.json` support.
- **Collector-phase diagnostics now reach the editor** (e.g. `BackedEnumCaseTypeMismatch`): `get_semantic_issues_salsa` previously merged only the body-analysis and class-issues sources, silently dropping any diagnostic raised during mir's collector phase.

### Fixed

- **A `@var` docblock immediately followed by `?>` is no longer lost across a split PHP/HTML block** (#235): a Yii2-style view template with `/** @var Post $model */` right before the closing `?>` tag, followed by inline HTML and a new `<?php` block, previously produced a false-positive `UndefinedVariable` and lost completion for `$model` in the second block.
- **Spreading a string-keyed array as a call's sole argument no longer produces a false type-mismatch** (`f(...$args)` binding by parameter name, valid PHP 8.1+ named-argument syntax): the spread expansion only recognized sequential integer keys and otherwise fell back to a nonsensical merged-union check against the first parameter.

### Dependencies

- **mir updated to 0.67.0** (from 0.66.1): wires up `AnalysisSession::collector_issues`, the `@var` docblock split-block fix, and the named-arg string-keyed spread fix above.

## [0.22.2] — 2026-08-02

### Fixed

- **A panicking off-request-loop closure no longer fails silently**: blocking work is now routed through one shared boundary (`Backend::blocking`/`blocking_gated`) that logs a panic instead of discarding it via `unwrap_or_default()`.

### Performance

- **`will_rename_files` and `will_delete_files` no longer block the request loop while parsing every file that imports the renamed/deleted class**: each batches its per-file parse-and-diff work into a single `spawn_blocking` call, matching the reference-edit loop beside it.
- **`prepare_call_hierarchy`'s workspace-wide trait-alias fallback scan no longer blocks the request loop**, matching the offloading already done for `incoming_calls`/`outgoing_calls`.
- **`selection_range`, `goto_implementation`, and `rename` no longer block the request loop for their inline whole-document walks**: the per-position chain walk, the method-decl disambiguation check, and the property/promoted-param and variable-scope walkers now run off the async runtime worker.
- **`completion_resolve` and `inlay_hint_resolve` no longer block the request loop on a cold workspace-index rebuild**: both called the raw index directly instead of the offloading `workspace_index_async` wrapper used elsewhere.
- **`on_type_formatting` no longer blocks the request loop on its whole-document brace/line scans**: the highest-frequency handler in the file, firing on nearly every keystroke, is now offloaded — completing the off-request-loop sweep started in 0.22.1.

## [0.22.1] — 2026-07-31

### Fixed

- **`new X()` no longer fails call-hierarchy self-declaration lookup for classes, interfaces, traits, and enums**: `new` targets are indexed under the type's own name, but the declaration lookup only matched functions/methods/class-members, so every `new` expression fell through to a workspace-wide trait-alias scan and resolved to nothing. `self`/`static`/`parent` are also now excluded from the call collector, since they're late-binding and never literal declaration entries.
- **`parent::CONST` now resolves to the parent class's constant** instead of failing outright: the owner-match logic already handled `self`/`static` but still compared `parent::` against the literal string `"parent"`, which can never match a real class name.

### Performance

- **`workspace/didChangeWatchedFiles`, `textDocument/didSave`, and `workspace/willRenameFiles` no longer block the request loop on file parsing/analysis**: each now batches its CPU-bound work — re-indexing changed files, recomputing diagnostics, parsing importers and reference sites for a rename — into a single `spawn_blocking` call, matching the pattern already used elsewhere.
- **`resolve_parent_construct_class` looks up the class by index** instead of linearly scanning every class in every workspace file.

### Dependencies

- **mir updated to 0.66.1** (from 0.65.0); **php-rs-parser and php-ast updated to 0.19** (from 0.18): php-ast 0.19 changes `Arg::value` to `Option<Expr>` to represent PHP 8.6 partial-application placeholders (`?`, `...`).

## [0.22.0] — 2026-07-31

### Added

- **`codeActionKinds` are now advertised and honored**: the server declares the concrete kinds it returns (`quickfix`, `refactor`, `refactor.extract`, `refactor.inline`, `source.organizeImports`) and filters results against a client's `context.only`, matching descendant kinds (`refactor` also covers `refactor.extract`) — previously every request returned every possible action regardless of what was asked for.

### Changed

- **Returning sessions seed mir's workspace symbol index from the disk cache**
  (mir 0.64): symbol lookups in a cache-warm session answer from an O(1) map
  and can never fall back to the O(all-files) index walk — `debugStats` gains
  `workspace_symbol_index_ready` and `workspace_index_walks` so tests pin the
  walk count at zero. Watcher-driven external edits reconcile the seeded
  index before any query reads it. Costs ~110 ms extra warm-boot indexing and
  ~23 MB RSS at 1.6K files; first-ever boot is unchanged.

### Fixed

- **Hover on a method declared only via a class-level `@method` docblock tag now shows its signature** instead of nothing: the AST-based member scan behind mir's method hover only walked concrete methods, missing virtual methods that completion and signature help already understood.
- **Type hierarchy's dynamic registration is no longer sent to clients that never declared `textDocument.typeHierarchy.dynamicRegistration`**: lsp-types 0.94 has no static `typeHierarchyProvider` field, so support can only be advertised via `client/registerCapability`, but that call was previously sent unconditionally.
- **Intermittent stack-overflow abort under contended parallel analysis**: salsa 0.28's dependency-graph lock transfer recurses per transferred dependent and could overflow the 16 MB analysis-thread stacks on large workspaces. All analysis threads (rayon workers, tokio blocking pool, warm-sweep thread) now get 64 MB stacks — reserved, not committed, so resident memory is unchanged.
- **Reading a `private`/`protected` property from outside its declaring class is now flagged** (`InaccessibleProperty`, mir 0.65), matching the existing visibility checks for class constants and methods.

### Dependencies

- **mir updated to 0.65.0** (from 0.64.0): a batch of analyzer correctness fixes, including the new `InaccessibleProperty` check above, promoted-property docblock refinement handling, nullable-hint preservation through `@param`, and exhaustive-match recognition of backed-enum `->value`.

## [0.21.0] — 2026-07-26

### Added

- **Analysis cache now flushes on a periodic background interval** (`analysisCacheFlushIntervalMs`, default 20s) instead of only after a fully-settled warm sweep or clean shutdown, bounding data loss on an unclean exit to roughly one interval regardless of session length.

### Changed

- **The whole-workspace text prefilter for references/rename is gone**: mir 0.61 gates cold candidate files on a symbol-name text mention internally (with PHP's case-insensitive matching semantics), so all reference-shaped handlers — references, rename, constructor references, code lens, call hierarchy — now hand mir the workspace scope through one consolidated `reference_candidate_files` path. Removes one or two full workspace text scans per query and closes a correctness hole where case-divergent mentions (`new COLOR()`) were dropped from the cold candidate set by the case-sensitive scan.
- **`willRenameFiles`/`willDeleteFiles` `use`-line rewrites now look up importers in the workspace index** instead of text-scanning the workspace and parsing every file that mentions the class's short name — renaming or deleting a file whose class has a common short name no longer parses unrelated files while the editor waits on the rename dialog.

### Performance

- **Workspace scan's directory walk is now parallel** (`src/index/workspace_scan.rs`): one `read_dir` per directory still runs serially, but the fan-out across subdirectories runs on the rayon pool instead of a single thread walking the whole tree — 1.3–1.5x faster on real Laravel/Symfony corpora, on top of the already-parallel parse+index phase.
- **`document_store`'s `file_index` memo shrinks on file removal**: previously unbounded (unlike the LRU-capped `parsed_doc`/`symbol_map`), so a permanently abandoned file — the common case for a rename — pinned its pre-deletion `FileIndex` for the rest of the process's life. Now cleared and recomputed against the emptied text right after removal.

### Dependencies

- **mir updated to 0.62.0** (from 0.60.0): adds a class-mention index that memoizes the reference-query gate's textual predicate per file, so repeat single-needle queries (classes, `__construct`) answer from recorded mention sets instead of rescanning every candidate's raw text (~5 MB of index instead of ~100 MB of text at Laravel scale). Also includes the 0.61.0 work: `indexed_references_to` skips analysis of never-committed files whose text cannot name the queried symbol, a parallelized workspace-symbol-index seed, `freeze_workspace_index` applied to the two remaining parallel body-analysis passes, and a single-pass multi-needle scan for the reference-scoping freshness gate. Cold `indexed_references_to` on mir's Laravel benchmark: 1.5s → 0.63s.

## [0.20.0] — 2026-07-19

### Fixed

- **Hover and match-arm completion now resolve members declared in unopened files (including `vendor/`)** via an O(1) workspace-index lookup, matching completion's existing fast path — previously only currently-open editor buffers were searched, so hover on an unopened class's method/property/constant silently fell back to a lower-fidelity path.

### Changed

- **The pre-mir `TypeMap` type-inference engine has been deleted entirely**: mir now resolves every `$var->`/`::` receiver that goto-definition needs (the last remaining caller), so the whole fallback — alias expansion, docblock element-type propagation, and its ~26 unit tests — is gone. One narrow edge case moves from silently-correct-by-luck to a known, documented gap: goto-definition through a free-function-scoped `@psalm-type` alias can pick the wrong same-named method when more than one class declares it, since mir's alias expansion is intentionally class-scoped only.

### Dependencies

- **mir updated to 0.60.0** (from 0.59.2): adds property-receiver narrowing for the `is_string`/`is_array`/`gettype`/`get_debug_type`/`get_class` family, `array_key_exists`/`in_array` narrowing, `class_implements`/`class_parents`/`get_parent_class` narrowing, `filter_var`/`is_countable`/`is_iterable` inference, and opaque-callback `array_map`/`array_reduce` return-type inference.

## [0.19.0] — 2026-07-18

### Added

- **Laravel string-key index**: go-to-definition, completion, and find-references for `env()`, `config()`, `view()`, `__()`/`trans()`, and named routes (`route()`, including `Route::group(['as' => ...])` prefix accumulation and the fluent `Route::name(...)->group(...)` equivalent). Gated behind a one-time Laravel-project detection (`artisan`/`composer.json`) so non-Laravel workspaces pay no per-request cost. `Route::resource()`/`apiResource()` implicit CRUD route names are a known, documented gap.
- **`signatureHelp` now resolves method-call receiver classes via mir** (`$var->method()`), matching hover/completion/goto-definition — previously the only feature with no mir integration, falling back to the pre-mir `TypeMap` walk unconditionally.

### Fixed

- **Hover on a first-class callable** (`strlen(...)`, `$obj->method(...)`) now shows mir's fully-typed closure signature instead of falling back to a bare `Closure`.

### Changed

- **Removed three now-redundant `TypeMap` fallback tiers** (`completion/member.rs`, `hover/hover_impl.rs`, `hover/named_args.rs`) — mir resolves a receiver's type directly at the `->`/`?->`/`::` operator gap as of the dependency bump below, which was the one case these existed for. Internal cleanup; no user-visible behavior change.

### Dependencies

- **mir updated to 0.59.2** (from 0.58.0): adds `symbol_at` resolution at the `->`/`?->`/`::` operator gap, bare `@var` `@psalm-type`/`@phpstan-type` alias expansion, and `array_map`/`array_reduce` return-type inference through an opaque `callable` parameter via caller unioning.

## [0.18.1] — 2026-07-17

### Fixed

- **A blank line inside an indented heredoc/nowdoc in a CRLF-checked-out file could drop a reference posting**: the parser's indentation check saw the line's trailing `\r` as non-whitespace content and emitted a spurious "Invalid body indentation level" error, which silently dropped later reference postings from the file. Fixed by bumping the parser (php-rs-parser 0.18.3) and mir (0.58.0).

## [0.18.0] — 2026-07-16

### Added

- **Reference postings now persist across server restarts**: the background warm sweep flushes analyzed files' reference postings to the on-disk cache, so a second server launch replays the reference index from disk and answers find-references index-warm with no analysis sweep — previously only subtype edges survived a restart this way.

### Changed

- **Call-hierarchy incoming calls and code-lens counts now answer from mir's posting-list indexes** instead of AST word-walkers: `incoming_calls` resolves the item's FQN and reads `meth:`/`methname:`/`fn:` postings directly, and code-lens reference/implementation/trait-usage counts read the same posting lists and subtype-edge index, with hierarchy clauses (`extends`/`implements`/`use Trait`) now counting as references too.
- **Rename and file-rename now answer from the same indexes**: call/access sites, `use`-import lines, and declaration tokens are resolved from mir's postings instead of the AST walker.
- **The background warm sweep no longer caps at 16,384 files** — postings survive analysis-memo eviction, so the cap only protected sweep duration, not query latency; the whole workspace is swept instead, with memory still bounded by mir's analysis LRU.

### Fixed

- **A warm sweep could report itself complete while silently skipping a chunk of files** whose analysis was cancelled by a transient concurrent write, occasionally leaving the reference index momentarily incomplete under contention. The sweep now retries a cancelled chunk until it settles before counting itself done.

### Dependencies

- **mir updated to 0.57.0** (from 0.55.0).
- **php-rs-parser and php-ast updated to 0.18.2** (from 0.18.1).

## [0.17.0] — 2026-07-15

### Changed

- **Find-references and go-to-implementation now answer from mir's delta-maintained inverted indexes** (reference posting lists and resolved subtype edges) instead of the opt-out imperative reference index and hand-rolled AST-based subtype/constructor walkers. Warm requests answer in 0.02–0.07 ms regardless of workspace size. Disk-cached postings and subtype edges now replay on workspace scan, before the analysis warm sweep runs, so a returning session starts index-warm.

### Fixed

- **Aliased `extends`/`implements` could be missed by go-to-implementation**: e.g. `use App\Base as X; class C extends X {}` is now resolved through the alias instead of only matching the bare name.
- **Promoted constructor properties weren't recognized as reference declarations**: `public function __construct(private User $user)` now participates in find-references like any other property declaration.
- **Class-constant references could resolve to the wrong owner**: the owning class is now resolved to its fully-qualified name instead of the bare short name before lookup.

### Dependencies

- **mir updated to 0.55.0**.

## [0.16.0] — 2026-07-14

### Added

- **Background analysis warm-up** (`warmAnalysis`, default `true`): after workspace indexing completes, the server analyzes every indexed file in the background — yielding to interactive requests — and re-warms after edits settle and after external file changes (e.g. `git checkout`). The first find-references or rename on a symbol answers from warm caches (sub-millisecond) instead of paying a cold per-file analysis at request time (~20 ms per candidate file, i.e. multi-second stalls on common symbols). Open files and the files declaring the classes they reference warm first, so the working set is ready within the sweep's first seconds. Set `warmAnalysis: false` to trade slower references for a smaller resident footprint.

## [0.15.1] — 2026-07-12

### Fixed

- **Parse-error diagnostics now carry a proper code and clean message**: previously had no `code` at all and could leak a raw byte-offset `Debug` string (e.g. `Span { start: 16, end: 17 }`) into the message; now uses a `SyntaxError` code and renders the position as line:col.
- **"Extract variable" no longer offered on non-expressions**: the eligibility check was purely textual and could offer the action on a bare class name, function name, or an entire class body, producing invalid PHP when applied. It now requires the selection to cover a real AST expression.
- **Renaming a class's file only rewrote importers, not the declaration**: `willRenameFiles` now also renames the `class`/`interface`/`trait`/`enum` declaration in the moved file itself, instead of leaving it out of sync with dependents' updated `use` imports.
- **`textDocument/rename` on a class missed type-hint occurrences**: renames now also update `function greet(User $user)`-style type hints, `extends`/`implements`, and static-call class tokens, in addition to `use` imports and `new`/`instanceof` sites.
- **Find-references resolved `parent::__construct()` to the child class**: now resolves to the actual parent class named in the `extends` clause.
- **Hover, go-to-implementation, and type-hierarchy cold-index fallbacks could match the wrong class**: same-short-name classes sharing a local `use ... as Alias` (e.g. Laravel's many `Factory` classes aliased to `FactoryContract`) are now disambiguated by FQN instead of by bare short name.
- **Type hierarchy could list a supertype twice**: fixed a missing dedup when two classes with the same short name shared a common ancestor.
- **Semantic tokens for comments were one character too long on CRLF files**: the scan for `//`, `#`, and `/* */` comments included the trailing `\r` in the token length, most visible on Windows checkouts.
- **`workspaceSymbol/resolve` never resolved real results from live client traffic**: results were always treated as an already-resolved zero-width range; it now falls through to compute the real name range.
- **Document-symbol and call-hierarchy selection ranges could escape their own symbol's range, or land on the wrong name**: fixed a fallback that pointed at byte offset 0, and PHP 8 attributes on a member no longer cause its name search to match text inside the attribute.
- **Go-to-type-definition ranges from the index were always zero-width**: results now highlight the actual class-name span instead of collapsing to line-start.
- **Signature help could resolve a bare call to a trait/interface/enum member**: e.g. `log($0)` matching a same-named trait method instead of the `log()` builtin; matches on those now require a receiver (`->`/`::`).
- **Enum `->value` completion always showed `string|int`**: now shows the enum's real declared backing type.
- **"Extract constant" could duplicate an existing constant declaration**: no longer offered when the selection is already the RHS of a `const` declaration.
- **Find-references from an enum case's own declaration found nothing**: case declarations are now classified and searched like class constants, so all real usages are found.
- **"Convert to closure" dropped variables captured from the outer scope**: arrow-function-to-closure conversion now synthesizes a `use (...)` clause for them, instead of producing a runtime "undefined variable" error.
- **`textDocument/typeDefinition` could resolve to an unrelated same-named class from an open file**: exact FQN matches from the background index now outrank an open document's ambiguous short-name fallback.

### Dependencies

- **mir updated to 0.53.1**: fixes a hover regression on static-property access and a chained-call completion fallback; seeds `$this` into free-standing closures.

## [0.15.0] — 2026-07-02

### Features

- **"Extract interface from class" refactor action**: Generates an interface from a class's public non-constructor methods and inserts it above the class declaration, appending `implements Name` to the class header. Handles existing `implements` lists, static methods, union/nullable return types, and unbraced namespaces.
- **"Add missing @throws tags" refactor action**: Offers to add `@throws` tags to a docblock for `throw new ClassName()` sites not yet documented, for both a single missing tag and multiple.
- **"Update PHPDoc to match signature" refactor action**: Regenerates an out-of-sync `@param`/`@return` block from the actual signature, preserving descriptions, `@throws`, and `@see` tags.
- **"Convert local variable to instance property" refactor action**.
- **"Convert switch to match" refactor action**: Offered when every non-empty case body is a single `return`; requires a `default` arm to preserve the exhaustiveness contract. Fall-through cases are grouped into comma-separated arms and dead `break;` is stripped.
- **"Convert to closure" refactor action**: Inverse of "Convert to arrow function" — converts `fn($p): T => expr` to `function($p): T { return expr; }`, handling `static`, by-ref returns, and nested arrow functions.
- **"Convert to arrow function" refactor action**: Converts an eligible closure (single-statement return, no by-ref `use`, no `yield`) to a PHP 7.4 arrow function.
- **"Change visibility" refactor action**: Offers "Make public / protected / private" on a method, property, or class-constant declaration line.
- **`@method` parameter lists in signature help**: `@method` tag signatures from docblocks are now used as a fallback source of parameter info when no real method body is found.
- **Docblock descriptions in signature help**: The free-text description preceding a function/method's docblock tags is now shown in `SignatureInformation.documentation`.
- **Workspace scan progress percentage**: `$/progress` now reports `WorkDoneProgressReport` with a percentage after each 500-file chunk, so editors can show a live progress bar during workspace scan.
- **`cachePath` initialization option**: Pins the on-disk cache to an explicit directory without touching `XDG_CACHE_HOME`.

### Performance

- **Dependent-diagnostics sweep is cancellable**: When a file that others depend on is edited, the workspace-wide re-analysis of its dependents now runs under a cancel token that the next edit flips. A newer keystroke preempts an in-flight sweep at the next file boundary instead of letting it run to completion, so typing over a widely-depended-on base class, interface, or trait stays responsive; superseded sweeps also drop their partial results so they can't land out of order.
- **Document highlight stays responsive under write pressure**: `textDocument/documentHighlight` now reads through a stale-tolerant accessor that serves the last-good cached parse instead of joining the cancellation retry loop when a concurrent edit stream keeps invalidating the snapshot.
- **Cache eviction gains LRU shedding**: `parsed_cache`, `analysis_cache`, `owned_program_cache`, and `type_map_cache` share recency tracking with bounded caps; the least-recently-used half is shed on overflow instead of an arbitrary front slice, so actively edited files survive a references sweep over a large candidate set.
- **Find-references prunes by owner reachability earlier**: The owner-class reachability check now runs before per-candidate analysis instead of as a post-filter, skipping expensive analysis on files that only text-match the method name. Cold references are 1.4×–3.2× faster across 100–3 000 files.
- **`code_lens` and `workspace/diagnostic` cancel on concurrent write**: Both handlers now poll a write-revision counter and abandon a stale scan instead of computing results that would be discarded.
- **Completion, hover, inlay-hint, and workspace-symbol CPU work offloaded**: These handlers now run on `spawn_blocking` instead of the async executor, so a slow request no longer stalls unrelated requests on the same worker thread. References also cancel on a concurrent write.

### Fixed

- **Completion suggests unimported classes from unopened files**: Class-name completion now searches a workspace-index-backed table in addition to open documents, so a class defined in a file never opened in the editor (a sibling package, or `vendor/` when `indexVendor` is enabled) appears as a candidate and triggers auto-import (#240).
- **Cache correctness on size-preserving edits**: The on-disk cache key switched from `mtime + size` to `blake3(uri || content)`, so a size-preserving edit within the same mtime second is no longer missed.
- **Silent panics in call hierarchy and code lens now logged**: `incoming_calls`, `outgoing_calls`, and `code_lens` now emit `tracing::warn!` with the target URI when their `spawn_blocking` task panics, instead of silently returning an empty result.
- **`vendor_index_cache` no longer grows unbounded**: Moved into the shared `CacheRegistry` so it is evicted alongside the other per-file caches instead of only ever being inserted into.

### Dependencies

- **mir updated to 0.50.2**, salsa to 0.27.2. 0.50.2 fixes a concurrent-read salsa query-stack abort (`index_generation` read the shared db handle) that could SIGABRT under simultaneous indexing and reference queries.

## [0.14.0] — 2026-06-25

### Features

- **find-implementations and type-hierarchy use mir's resolved subtype graph**: `textDocument/implementation`, `typeHierarchy/subtypes`, and `typeHierarchy/supertypes` now resolve subtypes through mir's name-resolution graph instead of the raw-name workspace index. This fixes under-reporting for classes that extend a parent via an aliased import (`use App\Base as X; class C extends X {}`) or a fully-qualified name (`class C extends \App\Base {}`).

### Performance

- **Word-boundary candidate pre-filter, scanned in parallel**: find-references applies a memmem word-boundary gate over the candidate file set in parallel threads before any semantic analysis, cutting per-request cost for common method names.
- **Parse-free method reference path with visibility-derived scope**: the method-reference path reads lightweight `FileIndex` entries (~2 KB/file) instead of full parsed documents, and the candidate set is scope-narrowed from visibility before text scanning. Protected methods limit the search to the declaring file plus its resolved subtype hierarchy.

### Fixed

- **Find-references session lookup retries on salsa cancellation**: a salsa cancellation during the session reference lookup no longer surfaces as an empty result; the lookup now retries like the other snapshot queries.

### Dependencies

- **mir updated to 0.49.0.**

## [0.13.1] — 2026-06-24

### Fixed

- **Intermittent `workspace/diagnostic` failure while indexing**: a concurrent write from the background workspace scan could cancel an in-flight salsa snapshot query during diagnostics, surfacing as a `task N panicked` error. Analysis now retries on cancellation instead of failing the request.
- **Find-references intermittently returning no results while indexing**: a concurrent scan write could cancel the session reference lookup's salsa snapshot, and the resulting panic was swallowed into an empty response. The lookup now retries on cancellation like the other snapshot queries.
- **Handler panics no longer abort the session**: a panic in one request handler is isolated, and the workspace index re-syncs on `didClose`.
- **CRLF and reversed selection ranges**: text-range handling now copes with CRLF line endings and ranges whose start is after their end.

### Performance

- **Find-references no longer degrades over a long session**: the references read path runs as a memoized, scoped query over a salsa snapshot instead of mutating shared state on every request.
- Reference collection and go-to-definition index lookups moved off the async executor, keeping the request loop responsive.
- Whole-document analysis reuses a cached owned AST per file revision, avoiding repeated deep clones.

### Changed

- **Single analysis database**: php-lsp and the mir analyzer now share one salsa database, removing the dual-database bridge and its duplicate invalidation.

### Dependencies

- **mir updated to 0.48.0.**

## [0.13.0] — 2026-06-21

### Features

- **Configurable parse debounce delay**: `initializationOptions` now accepts `debounceMs` (integer, default `100`). Set lower for fast machines or Neovim users; set higher for slow machines or large files to reduce parse thrashing.
- **`@template-covariant` / `@template-contravariant` in hover**: Hovering a class or function with a covariant or contravariant template parameter now shows the correct tag (`@template-covariant T`, `@template-contravariant T of Base`) instead of always rendering `@template`.

## [0.12.2] — 2026-06-21

### Dependencies

- **mir updated to 0.46.0**: Fixes ~8 false positives including trait property
  access, method override covariance, reference assignments, `preg_replace()`
  return types, generic collection empty arrays, `@internal` scoping, and
  `extract()`/variable-variable assignments. Also picks up php-rs-parser 0.18.1.

## [0.12.1] — 2026-06-21

### Bug Fixes

- **Variable goto-def lands on `$name`, not the type annotation**: Go-to-definition on `$var` in a function or method body now jumps to the `$` sigil of the parameter declaration rather than the start of the type annotation span (`Baz $x` previously resolved to `B`).
- **Find-references on `$variable` was always empty**: The general reference walker matched only `ExprKind::Identifier`, not `ExprKind::Variable`, so variable references never appeared. References for `$var` now use a scope-aware walker that collects all occurrences within the enclosing function/method.
- **Foreach `$key` leaked into sibling functions**: Variable references were not scoped to the enclosing function, so `$key` in a `foreach` loop would also return same-named variables in unrelated functions. References are now confined to the immediately enclosing function or method body.
- **`parent::__construct()` returned all constructors in the file**: Cursor on a call site like `parent::__construct()` was falling through to the unscoped method-reference path and returning every `__construct` declaration. It now resolves to the enclosing class's own instantiation sites only.
- **`parent::__construct()` call-site leaked across namespaces**: In namespaced classes, the call-site fallback now computes the fully-qualified class name (e.g. `Alpha\Widget`) so that same-short-name classes from different namespaces are excluded from the results.

## [0.12.0] — 2026-06-19

### Features

- **Trait alias go-to-definition**: Go-to-definition on a call to a trait-aliased method (`use Trait { orig as alias }`) now redirects to the original method in the aliasing trait. `FileIndex` records these aliases in `ClassDef.trait_method_aliases` so the resolution works without a full `ParsedDoc`.
- **PSR-0 autoloading**: `Psr4Map` now reads `psr-0` entries from `composer.json` and `vendor/composer/installed.json` and tries them as a fallback when PSR-4 resolution returns nothing. Go-to-definition for underscore-separated class names (e.g. `Acme_Client`) in PSR-0 vendor packages now resolves correctly.
- **`is_readonly` and `is_backed_enum` in FileIndex**: `ClassDef` gains `is_readonly` (readonly class modifier) and `is_backed_enum` (enum with a scalar backing type); `PropertyDef` gains `is_readonly`. All three are extracted during `FileIndex::extract` so cross-file features can use them without a full parse.

### Bug Fixes

- **Readonly class hover from index**: Hovering a readonly class defined in a background file now renders `readonly class Foo` instead of `class Foo`. `class_hover_from_index` was checking `is_abstract` but not the new `is_readonly` flag.
- **Anonymous class implementations navigable**: `new class implements I {}` now appears in go-to-implementation results. `collect_anon_class_in_expr` previously missed the `ExprKind::New` wrapper that anonymous-class expressions are parsed into.
- **TypeMap fallback improvements** (used when mir analysis is unavailable at an offset):
  - Generic params stripped from docblock class names (`@var Collection<User>` → `Collection`).
  - `list<T>`, `array<T>`, and `iterable<T>` element types propagated to foreach loop variables.
  - `@psalm-type` / `@phpstan-type` aliases in file-level docblocks collected and expanded so `@param Result $r` resolves when `Result` is a local alias.
  - `$fn = strlen(...)` (first-class callable) now registers `$fn` as `Closure` in the fallback path.
  - `@psalm-type` aliases inside braced-namespace blocks are now collected.
- **Variable hover completeness**: Variable hover now uses mir for scalar and callable types (int, string, callable, …) that were previously silently dropped when TypeMap had nothing.

### Dependencies

- mir updated to 0.45.0; `array_inference` fallback removed (no longer needed).

## [0.11.0] — 2026-06-17

### Features

- **PSR-4 vendor navigation**: Go-to-definition and call-hierarchy now resolve methods through the class hierarchy in `vendor/` files not covered by the workspace scan. A new `psr4_method_goto` helper lazily walks supertypes, caching each `FileIndex` in a `vendor_index_cache` so repeated navigations are cheap.
- **Incremental analysis cache**: Per-file eviction replaces the previous full `analysis_cache.clear()` on every keystroke. Body-only edits evict only the changed file; declaration changes bump a `decl_version` counter that lazily invalidates sibling files on next access, avoiding unnecessary re-analysis across the workspace.
- **Static member completion (`::`)**: Completion now detects `::` context and offers static members (methods, constants, enum cases) for the resolved class.
- **Property-declaration hover and method implement**: Hovering a property declaration surfaces its type, and go-to-implementation resolves methods across files.
- **Static-method hover via workspace index**: When the primary mir path returns nothing (e.g. `Str::camel()` where `Str.php` is not open), the class token before `::` is resolved through use-aliases and the signature is looked up in `FileIndex`.
- **`use function` quick-fix**: An `UndefinedFunction` diagnostic for a function defined in a namespaced workspace file now offers a quick-fix that inserts `use function Namespace\fn;`.
- **Enhanced startup messages and debug mode**: Clearer server startup logging plus an opt-in debug mode.

### Bug Fixes

- **Use-import alias resolution**: Aliased imports (`use ... as ...`) are now resolved consistently across static `::` completion, supertypes/find-implementations, and the `subtypes_of` index.
- **Cross-file member completion after method chains**: Member completions now resolve after method-chain receivers (`$obj->a()->b()->…`).
- **Autoload helper false positives**: `autoload.files` helper functions are pre-ingested so they no longer raise false `UndefinedFunction` diagnostics.
- **Property rename from declaration site**, superglobal guard, and cross-file implement fixes.
- **`did_save` cache correctness**: The on-disk `FileIndex` cache is written on `did_save`, using the stat key (matching the workspace-scan reader) and reading stat/content from disk rather than from the editor buffer.
- **Extract-constant edge cases**: String/comment content is skipped during brace matching; naming and multibyte extraction are handled correctly.
- **References from class-constant access sites** are now resolved.
- **UTF-16 declaration rename**: Fallback declaration rename uses UTF-16 code-unit counts.
- **Getter/setter quick-fix title** counts properties, not methods.
- **PSR-4 composer.json discovery**: Resolution walks up parent directories to find `composer.json` when the workspace root is nested.
- **Workspace scan robustness**: Phase 2a reads are fully drained so a single unreadable file can no longer truncate the workspace scan.
- **Removed unsafe quick-fixes**: The nullable-arrow (`?->`) and add-null-args quick-fixes are withdrawn until a safer implementation lands, as they could produce syntactically invalid PHP in edge cases.

### Dependencies

- Upgraded the mir analysis suite from 0.41.0 to **0.44.0** and the PHP parser suite to 0.18.0. mir 0.42 unified column indexing (body-analysis diagnostics are now 0-indexed like collector-stored ones), removing the column bifurcation workarounds; 0.43–0.44 refine analyzer accuracy (sealed-array narrowing via `array_key_exists`, `iterable<K,V>` key propagation, and fewer false positives on dynamic const/property access and non-class-name string literals).

## [0.10.0] — 2026-06-12

### Features

- **Salsa GC — tracked `SourceFile`**: `SourceFile` is now a `#[salsa::tracked]` struct produced by `workspace_files()`, so salsa GC frees its memo heap (`parsed_doc`, `file_index`, `symbol_map`) when a file is removed from the workspace. A separate immortal `FileText` input per URI survives delete/reopen cycles so no new salsa inputs accumulate on churn. Delete/reopen cycles no longer grow memory.
- **Completion inside strings/comments suppressed**: A state-machine scanner (`cursor_in_string_or_comment`) prevents completion from triggering inside string literals, `//`/`#` line comments, and `/* */` block comments. PHP 8 `#[…]` attribute syntax is correctly excluded from comment detection.
- **`missingTypes` and `mixedUsage` diagnostic toggles**: mir v0.36.0's `MissingReturnType`/`MissingParamType`/`MissingPropertyType` and `Mixed*` lints are now surfaced as opt-in categories (`diagnostics.missingTypes`, `diagnostics.mixedUsage`), both off by default to avoid noise on existing codebases.
- **`__get` magic property hover**: Hovering `->propName` when the property has no explicit declaration now surfaces the class and type derived from mir's resolved `__get` return type, showing `(property) ClassName::$prop: T` instead of falling through to no hover.
- **`insteadof` goto-definition**: Go-to-definition on a trait-use alias or conflict-resolution (`insteadof`) now resolves through mir's symbol dispatch to the precise method span in the winning trait, correctly respecting conflict resolution where the plain AST walk would pick the wrong trait.

### Performance

- **Blocking pool for mir and workspace-index rebuilds**: `cached_analysis` (mir Pass 1+2) and `get_workspace_index_salsa` (cold FileIndex walk) now run via `spawn_blocking` so they no longer stall the tokio executor under concurrent requests.
- **Incremental text sync**: `TextDocumentSyncKind::INCREMENTAL` — clients send only changed ranges instead of full document text on every keystroke, significantly reducing serialisation overhead for large files.
- **Allocation-free fuzzy matching**: `fuzzy_camel_match` previously allocated two lowercase `String`s and two `Vec<char>` per candidate. `FuzzyQuery` now holds the pattern pre-processed once; matching against candidates is heap-free — cuts hundreds of thousands of allocations per workspace-symbol picker keystroke at Laravel scale.
- **`O(1)` definition cross-file fallback**: `find_declaration_in_indexes` now looks up candidates through a `decls_by_name` reverse map on the salsa-memoised `WorkspaceIndexData` instead of scanning every `FileIndex` linearly.
- **`TypeMap` cached per document revision**: `TypeMap::from_doc_with_meta` was called up to twice per completion request; it is now memoised in `DocumentStore::cached_type_map`, keyed by source-`Arc` pointer equality, and shared across all completion paths for the same document revision.
- **Per-request class-list collected once**: The sub-namespace and cross-file completion branches previously each collected class lists from all open docs; both now share a single `OnceCell`-backed collection per request.
- **`O(log N)` source-file lookup**: Four `DocumentStore` accessors replaced a linear `.find(|sf| sf.uri == uri)` scan with a binary search over `Workspace::files` (kept sorted by URI by `sync_workspace_files`).
- **Semantic-tokens range pruning**: `semantic_tokens_range` now skips top-level statements and class/trait members whose spans don't overlap the requested range instead of walking the full AST and post-filtering.
- **Call-hierarchy index-backed**: `prepare_call_hierarchy` and `outgoing_calls` resolve candidate files through `decls_by_name` and fetch only those documents; full-workspace scans route through `spawn_blocking`.
- **Definition BFS queue**: Class-hierarchy BFS in `goto_definition` switched from `Vec::remove(0)` (O(n²)) to `VecDeque::pop_front` (O(1)).

### Bug Fixes

- **`goto_implementation` with FQN cursor**: Hovering a use-import line returns a fully-qualified name; the workspace index keys `subtypes_of` by short name, so the lookup was empty. The FQN is now shortened before the index lookup.
- **Stale diagnostics after `phpVersion` change**: `analysis_cache` keyed entries by source-content pointer only; changing `phpVersion` left cached `FileAnalysis` results from the old version alive for unchanged files. The cache is now cleared on `phpVersion` change.
- **`indexReady` delayed by salsa warmup**: The `$/php-lsp/indexReady` notification was blocked behind a full per-file salsa warmup, causing workspace features to be permanently unavailable on large codebases within normal timeouts. The notification is now sent before warmup begins.
- **`signatureHelp` method resolution**: `find_params_in_index` only searched `idx.functions`; method calls (`$obj->method()`) never returned a signature. The receiver type is now resolved and the correct class's methods are searched.
- **`definition` aliased namespace prefix**: `#[ORM\Column]` with `use Doctrine\ORM\Mapping as ORM` silently failed — `psr4_goto` received `"ORM\Column"` which has no PSR-4 mapping. The first segment is now expanded through the file's class import map before PSR-4 resolution.
- **Workspace symbol substring match**: `fuzzy_camel_match` only matched prefixes and abbreviations; querying `"Controller"` never matched `"BlogController"`. A substring fallback is now applied when prefix/abbreviation matching fails.
- **`workspace/willCreateFiles` capability**: The will-create handler was fully implemented but the capability was never registered in `workspace.fileOperations`, so spec-compliant clients never sent the notification. All six `fileOperations` capabilities are now advertised.

### Bug Fixes

- **Reference scan cancellation**: `textDocument/references` now yields to the tokio scheduler before starting the synchronous AST scan, so a queued `$/cancelRequest` can interrupt the handler before any CPU-bound work begins.

### Dependencies

- Upgraded `mir-{analyzer,codebase,issues,types}` from 0.35.1 to 0.36.0: adds `DuplicateInterface`, `DuplicateTrait`, `DuplicateEnum`, and `DuplicateFunction` issue kinds (the local duplicate-declaration AST walk is removed), plus `MissingReturnType`/`MissingParamType`/`MissingPropertyType` and `Mixed*` lints.

## [0.9.0] — 2026-06-08

### Features

- **`@mixin` traversal in file index**: `DocMethodEntry` now carries `return_type`, and workspace indexing follows `@mixin` tags so hover and completion resolve methods from mixed-in types across files.

### Bug Fixes

- **`documentSymbol` selectionRange containment**: Class, interface, trait, and enum members whose name text also appeared earlier in the file (e.g. a property default containing the method name as a string) returned a `selectionRange` pointing at the earlier occurrence, violating the LSP `selectionRange ⊆ range` invariant that VSCode enforces. All member kinds now use span-constrained search.
- **Call hierarchy `selectionRange`**: `prepare/callHierarchy` and `callHierarchy/incomingCalls` had the same name-range bug for function, class-method, trait-method, and enum-method items — the selected line in the call hierarchy tree could point to a string literal rather than the actual method name.
- **`textDocument/implementation` class location**: Implementing class names were located via an unconstrained content search; a same-name occurrence in a string literal before the class declaration caused the implementation link to target the wrong line.
- **`@method` docblock navigation**: go-to-definition and go-to-declaration now resolve `@method` tags in class docblocks.

## [0.8.0] — 2026-06-06

### Features

- **mir-primary hover**: Member hover now resolves types through mir's type engine instead of the internal TypeMap, giving more accurate signatures for inherited and vendor methods.
- **Variable type resolution via mir**: Variable types (including `$this`, enum case variables, and factory chains) are now resolved through mir symbols, eliminating a class of false `mixed` results in hover and completion.

### Performance

- **Dependencies**: Upgraded `mir-{analyzer,codebase,issues,types}` from 0.32.0 to 0.33.0, bringing 15–55% speedups across the board.
- **Symbol map memoisation**: Per-file symbol table is now computed once via a salsa query and reused across hover, completion, and references — eliminating redundant rebuilds on repeated requests.
- **Eager mir stub warm-up**: mir stubs are loaded once at initialisation and cached to disk, removing the cold-start penalty on the first hover or completion request.
- **Parallel PSR-4 and PHP version probe**: Both operations during `initialize` now run concurrently, cutting startup latency on large projects.
- **Incremental workspace sync**: Body-only edits no longer trigger a workspace index rebuild; the sync step is skipped entirely when the file set is unchanged.
- **`O(1)` class-doc lookup**: Completion's class docblock lookup is now constant-time via the workspace index instead of a linear scan.
- **References substring gate**: The references handler short-circuits on a fast string containment check before walking the full AST, cutting work on large workspaces.
- **Async workspace scan**: Phase 2 directory walk replaced with `spawn_blocking`, freeing the tokio runtime during heavy I/O.
- **Cache key**: On-disk cache now uses `mtime + size` instead of `blake3(content)` for the entry hash, avoiding a full file read on every cache lookup.

### Refactoring

- Removed `MethodReturnsMap`, `FunctionReturnsMap`, and the TypeMap fallback paths for completion, hover, named-arg resolution, and type definitions — mir is now the sole source of truth for all type inference.
- Consolidated per-file diagnostic merging into a single `merge_file_diagnostics` helper.
- Centralised cursor resolution and FQN short-name extraction across all feature modules.

### Bug Fixes

- **Completion**: `insertText` is now set correctly for instance property items.
- **Definition**: Class and function name lookups are now scoped to the enclosing statement span, preventing false matches on identifiers that appear elsewhere in the file.

## [0.7.0] — 2026-05-30

### Features

- **PHP 8.5 `CloneWith` expression**: Type inference and AST traversal now handle the `CloneWith` expression introduced in PHP 8.5.

### Performance

- **ArcSwap migrations**: `Backend` PSR-4 map and several `RwLock` fields migrated to `ArcSwap`, reducing lock contention under concurrent requests.
- **Parallel workspace Phase 2**: `scan_workspace` Phase 2 now runs with rayon, cutting indexing time on large projects.

### Bug Fixes

- **Completion**: Class constants are no longer offered in arrow (instance) member completions.
- **PHP version clamping**: Unsupported PHP versions passed via configuration are now clamped to the latest known version instead of triggering binary detection.

## [0.5.0] — 2026-05-15

### Features

- **Cross-file signature help**: `textDocument/signatureHelp` now falls back to the workspace `FileIndex` when a function is not found in the current file or built-in signatures. Supports both bare names and fully-qualified names (leading `\` stripped).

### Performance

- **Incremental cross-file diagnostics**: Uses `mir::analyze_dependents_of` to republish diagnostics only for files that genuinely depend on a changed file, replacing the previous full open-file scan.
- **Pre-warmed workspace index**: `workspace_index` is computed before `indexReady`, so the first hover request no longer triggers a cold parse.

### Bug Fixes

- **`phpVersion` fallback**: An explicitly-provided but invalid `phpVersion` now falls back to the latest stubs instead of falling through to PHP binary detection, which could pick up an unexpected system PHP version.

## [0.3.0] — 2026-04-26

### Maintenance

- **Dependencies**: Upgraded `mir-{analyzer,codebase,issues,types}` from 0.9.0 to 0.10.0, and `php-rs-parser`/`php-ast` from 0.9.2 to 0.9.4.

The mir 0.10.0 upgrade brings three new diagnostics that will now surface in the editor:
- `NullArgument` — literal `null` passed to a non-nullable parameter (warning).
- `UnusedFunction` — free functions never called when dead-code analysis is enabled.
- `InvalidPropertyAssignment` — value of an incompatible type assigned to a typed property.
- `InvalidDocblock` — malformed type annotation in a docblock.

## [0.2.0] — 2026-04-26

### Features

- **Incremental computation**: Salsa query layer replaces imperative codebase updates. Repeated reads (hover, completion, references) now hit memoized results rather than recomputing. LRU eviction keeps memory bounded.
- **Persistent cache**: On-disk cache stores file definitions across server restarts, eliminating the full workspace re-scan on startup for large projects.
- **`maxIndexedFiles`**: Configurable cap (default 1 000) on the number of files indexed during workspace scan. Prevents unbounded memory growth on very large monorepos.

### Performance

- Switched allocator to `mimalloc` for reduced fragmentation and faster small allocations.
- Parallelized `file_refs` warm-up for faster initial `textDocument/references` responses.
- Eliminated double-parse in workspace scan; skips codebase rebuild on body-only edits.
- O(log n) line lookup via precomputed `line_starts`; dropped O(n) UTF-16 position scan.
- Cached `TypeMap` in hover and completion; eliminated `Arc<str>` → `String` conversions in references.
- Reused bump arenas across parses via a global pool; avoided codebase rebuild in code actions.

### Bug fixes

- **References**: Trait method declarations, constructor refs, promoted-property refs, and nullsafe refs are now found correctly.
- **Completion**: Leading backslash in namespace-prefix completions is now normalised.
- **Hover**: Classes not yet opened in the editor are now resolved correctly; arrow function completion includes properties; variable scoping fixed.
- **Document symbols**: Interface method declarations now appear.
- **Goto definition**: `$this->traitMethod()` now resolves to the trait declaration.
- **Diagnostics**: `did_open` now triggers deprecated-call diagnostics; duplicate deprecated-call filter removed.
- **Require paths**: Used `Url::join` for relative require paths to fix resolution on Windows.
- **Configuration**: `send_refresh_requests` is now fired after `did_change_configuration`.

### Maintenance

- **Dependencies**: Upgraded `mir-*` crates from 0.7.x to 0.9.0 and `php-rs-parser`/`php-ast` to 0.9.2. PHP version is now wired through to the analysis engine.
- **Docblock parser**: Removed the in-tree duplicate; delegated to `mir`'s `DocblockParser`.

## [0.1.54] — 2026-04-12

### Performance

- **Call hierarchy**: Eliminated O(n²) scans with HashMaps for incoming/outgoing call lookups.
- **References**: Used `HashSet` for O(1) declaration-span filtering; collect declaration spans once before `retain()`.
- **Semantic diagnostics**: Build `all_sources` once per file instead of once per call expression.
- **Workspace scan**: Parallelised file parsing across CPU cores.
- **Workspace diagnostics**: Finalise codebase once instead of once per file.

### Features

- **Benchmarks**: Added a performance benchmark suite (`cargo bench`).

### Bug fixes

- **References**: Interface method declarations are now included in `collect_declaration_spans`; method declarations are now classified as `Method` (not `Function`).
- **Semantic tokens**: Stopped emitting keyword tokens.
- **Hover**: `@var` type and description are now shown for class properties.
- **Diagnostics**: Duplicate-declaration diagnostic range now spans the full symbol name.
- **PHP version detection**: Improved accuracy; wired through to diagnostics.
- **Selection range / semantic tokens**: Fixed several integration test failures uncovering edge-case bugs.

### Maintenance

- **Docblock**: Delegated to `mir`'s `DocblockParser`; removed the in-tree duplicate parser.

## [0.1.53] — 2026-04-12

### Bug fixes

- **Semantic tokens**: Token `length` for named type hints, attribute names, string literals, and variables now uses UTF-16 code units as required by the LSP spec, not raw byte span width. Previously any source containing non-ASCII characters (e.g. `"café"`, `Héros`) would produce incorrect highlight widths.
- **Positions**: `offset_to_position` no longer counts `\r` as a column on CRLF files. The stray column was inflating the end position of every token on a Windows line-ending line, corrupting ranges for hover, go-to-definition, references, rename, and all other LSP features on CRLF files.

### Maintenance

- **Dependencies**: Upgraded `php-rs-parser` and `php-ast` from 0.5.0 to 0.6.2, and `mir-*` from 0.3.0 to 0.4.1. The 0.6.x parser fixes a span bug where `parse_name()` incorrectly included trailing whitespace in name spans.

## [0.1.52] — 2026-04-12

### Features

- **CLI**: Server now prints a startup message to stderr on launch.

### Bug fixes

- **Safety**: Replaced `unwrap()` calls in production code paths with `expect()` to improve error messages on panic.

### Maintenance

- **Refactor**: Split `completion.rs` and `backend.rs` into focused submodules.
- **Tests**: Added coverage for all public traversal functions in the `walk` module.
- **Dependencies**: Updated all dependencies to latest versions.

### Documentation

- Added VS Code extension setup guide.
- Added PhpStorm native plugin reference.

## [0.1.51] — 2026-04-11

### Features

- **Extract method code action**: Added "Extract method" code action — promotes a selected block of statements into a new private method with parameters inferred from used variables.
- **Promote constructor parameters**: Added code action to promote constructor parameters to class properties.
- **Inlay hints**: Variadic param hints, arrow function return type hints, and foreach loop variable type hints.
- **Named argument snippets**: Completion now inserts named argument snippets for PHP 8 call sites.
- **Organize imports**: `use function` and `use const` statements are now handled by the organize imports action.
- **Hover**: Enum case backing values, class constant types, constants in interface/trait hover, and type inference for catch-block and static variables.
- **Signature help**: `@param` descriptions from docblocks are now shown in parameter documentation.
- **Symbols**: Interface constants listed as children; deprecated symbols carry the `deprecated` flag.
- **Code lens**: Implementation count lens for traits; `#[Test]` attribute detected for PHPUnit test methods alongside `@test`.
- **Semantic tokens**: `VARIABLE` and `TYPE` tokens emitted during statement walking for richer highlighting.
- **Completion**: Improved relative `include`/`require` path completions.
- **Implement action**: Interface resolved through `use` imports; stub bodies improved.
- **CLI**: `--version` flag added.

### Bug fixes

- **Type hierarchy**: Traits now use `CLASS` kind and subtype detection correctly identifies trait users.

### Performance

- **Semantic diagnostics**: Incremental analysis — `remove_file_definitions` + `finalize` on the persistent codebase replaces creating a fresh codebase per call, removing 60 lines of copy machinery.
- **Semantic diagnostics**: Stubs loaded once into a `static OnceLock`; backend's persistent codebase reused across calls.

### Maintenance

- **Release workflow**: Expanded to all 6 targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-pc-windows-msvc`.
- **Dependencies**: Updated `mir-analyzer` to 0.2.0 and `php-ast` to 0.4.0.
- **Refactor**: Deduplicated AST traversal with `RefVisitor` trait, replacing four near-identical `*_refs_in_stmt` functions.
- Removed dead code and unused parameters across multiple modules.

### Documentation

- Improved PHPStorm / LSP4IJ setup guide, README, and configuration reference.

## [0.1.50] — 2026-04-05

### Features

- **Extract constant code action**: Added "Extract constant" code action — promotes an inline scalar expression to a class or file-level constant.

### Bug fixes

- **'Add use import' code action**: Gated the action on a typed `IssueKind` code so it no longer appears for unrelated diagnostics.

### Documentation

- Rewrote README; added LICENSE, CONTRIBUTING, and editor setup guides (`docs/editors.md`).
- Added Neovim 0.11 `lsp/` config with 0.10 fallback.
- Fixed Claude Code LSP config — added required `extensionToLanguage` field.

### Maintenance

- **Release workflow**: Added `.github/workflows/release.yml` — triggers on `v*` tag pushes, runs tests on Ubuntu/macOS/Windows, builds cross-platform binaries (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`) with `.sha256` checksums, uploads artifacts to a GitHub Release, and publishes to crates.io.
- **CI hardening**: Pinned all GitHub Actions to commit SHAs (`actions/checkout` v6.0.2, `actions/cache` v5.0.4, `softprops/action-gh-release` v2.6.1, `dtolnay/rust-toolchain` v1); scoped `contents: write` permission to the build job only.

## [0.1.49] — 2026-04-04

### Features

- **Hover for property docblocks**: `$obj->prop` and `$this->prop` now show the property's docblock in hover.
- **Completion item detail**: Completion items now carry a full signature in `detail` and documentation from docblocks.
- **Auto-import on attribute completion**: Selecting an attribute class completion inserts the `use` statement automatically.

### Bug fixes

- Fixed four independent bugs (refs jorgsowa/php-lsp#2 #3 #4 #5).
- `refs_in_stmt` now pushes the name span instead of the whole-statement span, fixing incorrect reference ranges.

## [0.1.48] — 2026-04-04

### Features

- **Semantic tokens**: Added `string`, `number`, `comment`, and `keyword` semantic token types.
- **Semantic find-references**: Filters results by symbol kind to eliminate false positives.

### Maintenance

- Migrated from `mir-php` to `mir-analyzer` for semantic diagnostics.
- Added snapshot tests with `expect-test` for hover and completion.
- Added unit tests for `backend.rs` pure helper functions and a `cursor()` position marker helper for test fixtures.
- Applied `cargo fmt` across all files; resolved clippy warnings (`needless_late_init`, `unnecessary_map_or`, `collapsible_if`).

## [0.1.47] — 2026-04-03

### Bug fixes

- **Workspace diagnostics**: Semantic diagnostics are now included in `workspace/diagnostic` pull responses.

## [0.1.46] — 2026-04-01

### Bug fixes

- Fixed UTF-16 byte offset calculations, CRLF line ending handling, and span equality comparisons.

## [0.1.45] — 2026-03-31

### Features

- **Organize imports**: New code action to sort and deduplicate `use` statements.
- **Inline variable**: New code action to inline a variable assignment at its usage sites.
- **Magic constants**: `__DIR__`, `__FILE__`, `__CLASS__`, etc. now complete and resolve correctly.
- **Closure `use` completions**: Variables captured in `use (...)` clauses are now suggested.
- **Attribute argument completions**: Named arguments on attributes are now completed.
- **Symbol kind filter**: Workspace symbol search now accepts a kind filter.
- **Psalm/PHPStan tags**: `@psalm-param`, `@phpstan-return`, etc. are parsed and surfaced in hover/completion.

## [0.1.44] — 2026-03-31

### Features

- **Scope-aware highlights**: Document highlights now respect variable scope boundaries.
- **Moniker FQN via `use`**: Monikers resolve the fully-qualified name through `use` imports.

## [0.1.43] — 2026-03-31

### Features

- **Format-on-save**: The server now handles `textDocument/willSaveWaitUntil` to format before save.
- **PHP file stub on create**: A minimal `<?php` stub is inserted when a new PHP file is created.
- Fixed several LSP capability registration gaps.

## [0.1.42] — 2026-03-31

### Features

- **Rename property**: Cross-file rename now covers property declarations and all `$obj->prop` / `$this->prop` usages.
- **Go-to-definition for `$variable`**: Navigates to the assignment site of a local variable.

## [0.1.41] — 2026-03-31

### Features

- **Rename variable/param in scope**: Renames a local variable or parameter within its enclosing scope without affecting other scopes.
- **Extract method**: New code action to extract a selected block into a new method.

## [0.1.40] — 2026-03-31

### Features

- **Add return type declaration**: New code action to insert a return type based on inferred type.
- **Constructor-promoted properties**: Promoted properties (`__construct(private Type $prop)`) are now resolved in type inference and completion.

## [0.1.39] — 2026-03-31

### Bug fixes

- LRU eviction now skips currently open files to prevent evicting active documents.
- Fixed half-open range boundary off-by-one in eviction logic.

## [0.1.38] — 2026-03-31

### Bug fixes

- Fixed multiple UTF-16 position calculation bugs affecting go-to-definition, hover, and completion on multibyte characters.
- Filled LSP capability gaps surfaced by capability negotiation tests.

## [0.1.37] — 2026-03-30

### Maintenance

- **Repository restructure**: Removed the Cargo workspace wrapper — `php-lsp` is now a standalone crate. `mir-php` continues to be resolved from crates.io as before.

## [0.1.36] — 2026-03-30

### Features

- **Extension stubs — builtin functions**: Added ~100 builtin PHP functions to the arity table (`array_fill_keys`, `array_is_list`, `hash/*`, `openssl_*`, `password_*`, `mb_*`, `gz*`, `filter_*`, `random_bytes/int`, `curl_multi_*`, and many more); all entries are binary-search sorted for O(log n) lookup.
- **Extension stubs — builtin classes**: Added ~30 builtin classes (`DOMDocument`, `DOMElement`, `DOMNode`, `DOMNodeList`, `DOMAttr`, `DOMText`, `DOMXPath`, `SimpleXMLElement`, `SimpleXMLIterator`, `XMLReader`, `XMLWriter`, `ZipArchive`, `Fiber`, `FiberError`, `mysqli`, `mysqli_result`, `mysqli_stmt`, `SplFileInfo`, `SplFileObject`, `DirectoryIterator`, `FilesystemIterator`, `GlobIterator`, `RecursiveDirectoryIterator`, `ReflectionClass`, `ReflectionMethod`, `ReflectionProperty`, `ReflectionFunction`, `ReflectionParameter`, `HashContext`, `JsonException`, `WeakMap`, `IntlChar`).
- **ClassMembers stubs**: Full method/property completions for all new classes above, plus `WeakMap` and the `Reflection*` family.
- **DNF type support**: `(A&B)|C` disjunctive normal form types now parse correctly in `mir-php` — parenthesised intersection groups are split on `|` at depth 0, then each group is stripped of parens and split on `&`.
- **Include/require path completions**: Typing an `include`/`require` string literal now offers filesystem completions relative to the current document's directory, showing `.php`/`.inc`/`.phtml` files and subdirectories (directories listed first).
- **Diagnostic configuration flags**: `initializationOptions.diagnostics` accepts per-category toggles — `enabled`, `undefinedVariables`, `undefinedFunctions`, `undefinedClasses`, `arityErrors`, `typeErrors`, `deprecatedCalls`, `duplicateDeclarations`. Settings are read live via `workspace/didChangeConfiguration`.

## [0.1.35] — 2026-03-30

### Maintenance

- **CI**: Removed `path = "../mir-php"` from the dependency — CI now resolves `mir-php` from crates.io; local workspace resolver continues to use the sibling crate unchanged.
- **Clippy**: Resolved all 149 warnings surfaced when `mir-php` became available to the linter. Key fixes: collapsible `if` statements, redundant `..Default::default()`, `starts_with` + manual slice replaced with `strip_prefix`, `MetaEntries` type alias for complex `HashMap` type, dead `get_element_type` method removed (test updated to use `tm.get("$result[]")`).
- **Formatting**: Applied `cargo fmt` across 31 files.

## [0.1.34] — 2026-03-30

### Bug fixes

- **`call_hierarchy.rs` — `prepare_call_hierarchy` could not find trait/enum methods**: `find_declaration_item` only handled `Function`, `Class`, and `Namespace` nodes. Trait and enum methods were never returned by `prepare_call_hierarchy`, breaking call hierarchy for those symbols entirely. Added `StmtKind::Trait` and `StmtKind::Enum` arms to match the fix already applied to `enclosing_in_stmt`.

### Tests

- Added 13 tests covering all bug fixes from v0.1.29–v0.1.33 that previously had no test:
  - `definition` — enum definition, enum case, and enum method go-to-definition
  - `call_hierarchy` — `prepare_call_hierarchy` for enum method, outgoing calls from enum method body, outgoing calls from for-loop init/update
  - `code_lens` — ref-count lens for enum declaration, trait declaration, and enum method
  - `declaration` — go-to-declaration for enum method
  - `semantic_diagnostics` — deprecated warning for enum method call
  - `semantic_tokens` — for-loop init/update expressions are tokenized
  - `type_map` — type inference inside trait method body, type inference inside enum method body

## [0.1.33] — 2026-03-30

### Bug fixes

- **`signature_help.rs` — no signature help for trait/enum methods**: `find_signature` only scanned `Function` and `Class` nodes. Trait and enum method signatures are now found.
- **`call_hierarchy.rs` — call hierarchy broken inside trait/enum methods**: `enclosing_in_stmt` returned `None` for `StmtKind::Trait` and `StmtKind::Enum`, so "Prepare Call Hierarchy" on a call inside those method bodies found nothing. Both are now handled.
- **`type_map.rs` — type inference dead inside trait/enum method bodies**: `collect_types_stmts` walked `Class` method bodies but ignored `Trait` and `Enum`. Param types and variable assignments inside trait/enum methods now contribute to the type map, enabling hover and completion there.
- **`inlay_hints.rs` — no param hints for trait/enum method calls**: `collect_defs_stmts` only registered `Function` and `Class` method signatures. Trait and enum method signatures are now registered so call sites get `param:` hints.

## [0.1.32] — 2026-03-30

### Bug fixes

- **`type_map.rs` — `$this->` completion broken inside enum methods**: `enclosing_class_in_stmts` only matched `StmtKind::Class`; now also matches `StmtKind::Enum` so the enum name is returned as the enclosing type.
- **`code_lens.rs` — no reference-count lenses for enums or traits**: `collect_lenses` had no cases for `StmtKind::Enum` or `StmtKind::Trait`. Both now emit a ref-count lens on their name and on each of their methods.
- **`implementation.rs` — PHP 8.1 enum implementations not found**: `collect_implementations` only checked `StmtKind::Class`. Now also checks `StmtKind::Enum`, so enums implementing an interface appear in go-to-implementation results.
- **`semantic_diagnostics.rs` — deprecated warnings missing for trait/enum method bodies**: `collect_deprecated_calls` walked Class and Function bodies but not Trait or Enum method bodies. Now all four are walked.
- **`semantic_diagnostics.rs` — deprecated warnings missing for nested calls**: `check_expr_for_deprecated` checked only the outermost function/method call per statement. Now recurses into call arguments and the callee object, so `wrapper(oldFn())` correctly warns about `oldFn`.
- **`semantic_diagnostics.rs` — `find_method_span_in_stmts` missed trait/enum methods**: Deprecation look-up only scanned Class members; methods declared in traits or enums were never found. Now scans all three.
- **`declaration.rs` — go-to-declaration missed enum methods**: `find_any_declaration` had no `StmtKind::Enum` arm, so jumping to the declaration of an enum method returned nothing.

## [0.1.31] — 2026-03-30

### Bug fixes

- **`inlay_hints.rs` — `for` init/update not walked**: Parameter hints inside `for (init; cond; update)` were missing. Same fix applied as was done to `walk.rs` in v0.1.30.
- **`call_hierarchy.rs` — `for` init/update not walked**: Outgoing calls inside for-loop init/update expressions were not detected. Also added Trait and Enum method body scanning to `collect_calls_for` so outgoing calls from those are now visible.
- **`semantic_tokens.rs` — `for` init/update not walked**: Expressions in for-loop init/update were not syntax-highlighted.
- **`selection_range.rs` — `StmtKind::Enum` not handled**: Selection range inside an enum method body was silently dropped (no enum parent in the chain). Added handling matching the Trait pattern.
- **`definition.rs` — Enum member scanning missing**: Go-to-definition for enum cases and enum methods only found the enum declaration itself, not individual members. Now scans `e.members` for both `EnumMemberKind::Case` and `EnumMemberKind::Method`.
- **`symbols.rs` — Interface constants missing from document outline**: `StmtKind::Interface` emitted `children: None` regardless of whether the interface had constants. Interface constants are now emitted as `SymbolKind::CONSTANT` children.

## [0.1.30] — 2026-03-30

### Bug fixes

- **`walk.rs` — enum method bodies not walked**: `StmtKind::Enum` fell into the catch-all `_ => {}` in `refs_in_stmt`, so references inside enum methods were invisible to find-references and rename. Now walks method bodies and backed enum case values.
- **`walk.rs` — class/trait property default expressions not walked**: `ClassMemberKind::Property` was unhandled in both `StmtKind::Class` and `StmtKind::Trait`. Class constants used as property defaults (e.g. `public $x = Status::ACTIVE`) were missed by rename.
- **`walk.rs` — `for` loop init/update not walked**: `StmtKind::For` only visited the condition. Now also visits `f.init` and `f.update` so function calls in those positions are found by references/rename.
- **`inlay_hints.rs` — no parameter hints for `new ClassName(...)` calls**: `ExprKind::New` was unhandled in `hints_in_expr`. Constructors are now registered in the def map under the class name, and `new Foo(1, 2)` emits `x:`, `y:` hints when `__construct` is known.

## [0.1.29] — 2026-03-27

### Bug fixes

- **Folding — duplicate ranges for control-flow statements**: `if`, `while`, `for`, `foreach`, and `do-while` statements called `fold_stmt(body)` on their `Block` body, which emitted a second fold range identical to the outer statement's range. Fixed by introducing `fold_body()` which recurses into block contents without emitting a fold for the block itself.
- **Folding — spurious abstract method folds in interfaces**: `StmtKind::Interface` emitted a fold range for every method member, including abstract method declarations whose span bled into the closing `}` of the interface. Since interface methods have no body, method-level folds are now only emitted when a concrete body is present (consistent with `Class` and `Trait` handling).

## [0.1.28] — 2026-03-28

### Test quality

- **415 tests** (up from 394); 32 new tests added, 11 existing weak tests rewritten with exact assertions
- Replaced `assert!(!result.is_empty())` / `assert!(result.is_some())` with `assert_eq!` on exact counts, exact line numbers, exact message text, and exact command names throughout `semantic_diagnostics`, `references`, `document_highlight`, `code_lens`, and `symbols`
- New tests cover: unknown receiver completions, static-only member filtering, hover on unknown/builtin symbols, nested call signature help, method call signature help, zero-reference lens, PHPUnit lens command/title format, exact fold ranges for nested constructs, single-line no-fold, docblock union/nullable/method tag parsing

## [0.1.27] — 2026-03-28

### Improvements

- **16 new tests** — 394 total (up from 378); also fixed real bugs uncovered by writing them:
  - `deprecated_method_call_emits_warning` — method `@deprecated` calls now correctly emit a warning (the `ExprKind::MethodCall` branch was missing from `check_expr_for_deprecated`)
  - `nullable_param_resolves_to_class` — `?Foo` type hints now correctly map `$x` to `Foo` in the type map (nullable stripped)
  - `union_type_param_maps_both_classes` — `Foo|Bar` type hints now populate the type map for both classes
  - `static_return_type_resolves_to_class` — `: static` return type now resolves to the enclosing class name
  - `goto_definition_class_constant` / `goto_definition_property` — go-to-definition now finds class constants and properties
  - `finds_use_statement_reference` / `partial_match_not_included` — reference search correctly includes `use` statements and excludes partial-word matches
  - `rename_does_not_match_partial_words` / `rename_updates_use_statement` — rename correctly skips partial matches and updates `use` imports
  - `hints_outside_range_excluded` / `method_call_gets_param_hints` — inlay hints respect the requested range and work for method calls

## [0.1.26] — 2026-03-28

### Bug fixes

- **Inlay hint range character check** — `textDocument/inlayHint` `pos_in_range` now validates the cursor's column/character position, not just its line. Hints were previously emitted for any position on the same line as the hint, even outside the requested range.

## [0.1.25] — 2026-03-28

### Bug fixes

- **Diagnostics lost on `didChange`** — duplicate declaration warnings and deprecated-call warnings disappeared after the first keystroke and only reappeared on save. The `did_change` debounced parse now publishes all three diagnostic types (parse errors, duplicate declarations, deprecated calls) consistently with `did_open` and `did_save`.

## [0.1.24] — 2026-03-28

### Bug fixes

- **Range containment character check** — `textDocument/prepareCallHierarchy` and `textDocument/selectionRange` now correctly validate the column/character position, not just the line. Previously any position on the same line as a single-line symbol would match.
- **Formatting end position** — `textDocument/formatting` used `line_count` (1-based) as the end line of the replacement range; fixed to `line_count - 1` (0-based). Formatters that return the same number of lines would previously emit an out-of-bounds range.
- **Trait symbol kind in type hierarchy** — `textDocument/prepareTypeHierarchy` now returns `SymbolKind::INTERFACE` for traits instead of `SymbolKind::CLASS`.

## [0.1.23] — 2026-03-28

### Bug fixes

- **UTF-16 range lengths** — `textDocument/references`, `textDocument/documentHighlight`, `textDocument/typeDefinition`, `textDocument/definition`, `textDocument/documentLink`, and `textDocument/semanticTokens` all now report symbol lengths in UTF-16 code units as required by the LSP spec, rather than byte lengths. No visible change for ASCII identifiers; correct behaviour for any non-ASCII content.

## [0.1.22] — 2026-03-28

### Bug fixes

- **Namespace-aware duplicate detection** — `class Foo` in namespace `App` and `class Foo` in namespace `Other` no longer trigger a false "duplicate declaration" error; the check now uses fully-qualified names as keys.
- **Bracket-aware signature parameter splitting** — parameter labels in signature help no longer break when a default value contains a comma (e.g. `array $x = [1, 2, 3]`, `callable $fn = fn($a, $b) => 0`); a depth-tracking splitter is used instead of a naive `.split(',')`.
- **`collect_members_stmts` early-return fix** — member collection no longer bails out prematurely when any members are found in an earlier namespace block; the function now only short-circuits after definitively matching the target class.
- **Union type whitespace** — `Foo | Bar` (spaces around `|`) is now handled identically to `Foo|Bar` throughout the type map and completion engine.

## [0.1.21] — 2026-03-27

### New features

- **`textDocument/didSave`** — diagnostics (parse errors, duplicate declarations, deprecated-call warnings) are re-published on every save, so editors that defer diagnostics until save see up-to-date results immediately.
- **`textDocument/willSave` / `willSaveWaitUntil`** — handlers registered and advertised in server capabilities; `willSaveWaitUntil` returns no edits (format-on-save is handled by the existing `textDocument/formatting` request).

## [0.1.20] — 2026-03-27

### New features

- **Nullsafe `?->` completions** — `$obj?->` now triggers the same member completions as `$obj->` by correctly stripping the longer `?->` pattern before `->` during receiver extraction.
- **Promoted constructor property completions** — `__construct(private string $name, public readonly int $age)` — promoted params are recognized as class properties (including `readonly`) and appear in `->` completions.
- **Default values in signature help** — function parameters with defaults now show them in the hint: `int $x = 10`, `string $s = 'hello'`, `bool $flag = true`, `mixed $v = null`, `array $items = []`.
- **Property type hover** — hovering over `propName` in `$obj->propName` or `$this->propName` shows `(property) ClassName::$propName: TypeHint`, resolved from the property declaration or promoted constructor param.

## [0.1.19] — 2026-03-26

### New features

- **`@property` / `@method` docblock tags** — class docblocks with `@property Type $name`, `@property-read`, `@property-write`, and `@method [static] ReturnType name(...)` are parsed and injected into the type map; `->` completions include synthesised properties and methods from mixin-style magic classes.
- **Variable scope in completions** — variable completions are now filtered to only variables declared *before* the cursor line, eliminating false suggestions from variables that haven't been assigned yet.
- **Sub-namespace `\` completions** — when the typed prefix contains `\`, only FQN-qualified class names whose namespace prefix matches are suggested, scoping the list to the current sub-namespace.
- **Magic method completions** — inside a class body, `__construct`, `__destruct`, `__get`, `__set`, `__isset`, `__unset`, `__call`, `__callStatic`, `__toString`, `__invoke`, `__clone`, `__sleep`, `__wakeup`, `__serialize`, `__unserialize`, and `__debugInfo` are offered as snippet completions with their canonical signatures.
- **`use` alias hover** — hovering over a name on a `use` import line shows the fully-qualified class name being imported.
- **`??=` (null-coalesce-assign) inference** — `$var ??= new Foo()` is now handled in the type map: the variable retains its existing type if already set, or takes the RHS type on first assignment.
- **Duplicate declaration diagnostics** — redefining a class, function, interface, trait, or enum already declared in the same file emits an `Error` diagnostic with the message `"Duplicate declaration of '<Name>'"`.

## [0.1.18] — 2026-03-27

### New features

- **`self`/`static` return type resolution** — methods returning `: self` or `: static` now resolve to the enclosing class name in the type map, enabling fluent builder chains (`$builder->setName()->` shows `Builder` members).
- **Hover on `$variable`** — hovering over a variable shows its inferred type as `` `$var` `ClassName` ``.
- **Built-in stubs wired to hover** — hovering over a built-in class name (e.g. `PDO`, `DateTime`, `Exception`) shows its available methods, static methods, and parent class from the bundled stubs.
- **`use` FQN completions** — typing `use ` triggers namespace-qualified class name completions from all indexed documents.
- **Union type completions** — `@param Foo|Bar $x` or `function f(Foo|Bar $x)`: both `Foo` and `Bar` members appear in `$x->` completions.
- **`#[` attribute class completions** — typing `#[` triggers a completion list of all known class names for use as PHP 8 attributes.
- **`match` arm completions** — inside a `match ($var) {` block, the default completion list is prepended with `ClassName::CaseName` entries from `$var`'s enum type.
- **Deprecated call warnings** — calling a function annotated with `@deprecated` emits a `Warning` diagnostic at the call site.
- **`include`/`require` path completion infrastructure** — context detection for include/require strings wired in; full file-path suggestions require a future doc-URI pass-through.
- **`readonly` property recognition** — PHP 8.1 `readonly` properties appear in `->` completions with `"readonly"` as the detail label.

## [0.1.17] — 2026-03-27

### New features

- **`@param` docblock → type map** — `@param ClassName $var` in function and method docblocks is now read into the type map. `$var->` completions work even when the PHP parameter has no type hint. AST type hints take precedence over docblock annotations.
- **Method-chain `@return` type inference** — `$result = $obj->method()` now infers `$result`'s type from the method's return type hint (`: ClassName`) or `@return ClassName` docblock. Chains work across files when using `TypeMap::from_docs_with_meta`. Nullable return types (`?Foo`) are stripped to `Foo` automatically.
- **Built-in PHP class stubs** — `->` and `::` completions now work for PHP's standard library classes without any user-defined stubs: full Exception hierarchy (`Exception`, `RuntimeException`, `InvalidArgumentException`, all sub-classes, `Error`, `TypeError`, `ValueError`, etc.), `DateTime`/`DateTimeImmutable`/`DateInterval`/`DateTimeZone`, `PDO`/`PDOStatement`, `ArrayObject`/`ArrayIterator`, `SplStack`/`SplQueue`/`SplDoublyLinkedList`/`SplFixedArray`/`SplHeap`/`SplObjectStorage`, `Iterator`/`IteratorAggregate`/`Countable`/`ArrayAccess`/`Stringable` interfaces, `Closure`, `Generator`, `WeakReference`, `stdClass`. PDO constants (`FETCH_ASSOC`, `ATTR_ERRMODE`, etc.) appear as `::` completions.
- **Constructor-chain completions** — `(new ClassName())->` now triggers member completions for `ClassName`, including built-in stubs (e.g. `(new DateTime())->format(`).
- **`!== null` type preservation** — variables typed via `new`, typed param, or `@var` retain their type inside `if ($x !== null)` blocks.

## [0.1.16] — 2026-03-27

### New features

- **`instanceof` type narrowing** — `if ($x instanceof Foo)` narrows `$x` to `Foo` in the type map; `->` completions inside the branch now show `Foo`'s members. Fully-qualified class names are shortened to the simple name (`App\Services\Mailer` → `Mailer`). `elseif` and `else` branches are also recursed into.
- **PHP superglobals in completion** — `$_SERVER`, `$_GET`, `$_POST`, `$_FILES`, `$_COOKIE`, `$_SESSION`, `$_REQUEST`, `$_ENV`, and `$GLOBALS` appear as `Variable` completion items with a `"superglobal"` detail label. Available on both the `$` trigger character and the default (no-trigger) completion list.
- **Bound-closure `$this` completion** — `Closure::bind($fn, $obj)`, `$fn->bindTo($obj)`, and `$fn->call($obj)` patterns map `$this` to `$obj`'s inferred class in the type map, so `$this->` completions work inside top-level bound closures.
- **`array_map`/`array_filter` element-type propagation** — when the callback has an explicit return type hint (e.g. `fn($x): Widget => ...`), the element type is stored under the `$var[]` key. A `foreach ($result as $item)` over that variable then propagates `Widget` to `$item`, enabling `$item->` completions.
- **`@psalm-type` / `@phpstan-type` type aliases** — docblock parser recognises `@psalm-type Alias = TypeExpr` and `@phpstan-type Alias = TypeExpr` tags; aliases are rendered in hover as `**@type** \`Alias\` = \`TypeExpr\``.
- **Snippet completions** — functions and methods with parameters use `InsertTextFormat::SNIPPET` so the cursor lands inside the parentheses after accepting. Zero-parameter callables insert `name()` as plain text.
- **Enum built-in properties** — `->name` is offered as a completion on every enum instance; backed enums (`enum Foo: string`) also expose `->value`. `::from()`, `::tryFrom()`, and `::cases()` appear as static completions on backed enums.
- **`textDocument/moniker`** — returns a PHP-scheme moniker with the PSR-4 fully-qualified name as the identifier and `UniquenessLevel::Group`.
- **`textDocument/inlineValue` + `workspace/inlineValue/refresh`** — scans for `$variable` occurrences in the requested range and returns `InlineValueVariableLookup` entries for debugger variable display; `$this` and `$$dynamic` variables are skipped.
- **`workspace/willCreateFiles` / `workspace/didCreateFiles`** — new PHP files are indexed immediately when created; the server fires inline-value, semantic-token, code-lens, inlay-hint, and diagnostic refresh requests.
- **`workspace/willDeleteFiles`** — returns a `WorkspaceEdit` that removes all `use FullyQualifiedName;` imports referencing the deleted file across the workspace.
- **`workspace/didDeleteFiles`** — removes deleted files from the index and clears their diagnostics.

## [0.1.15] — 2026-03-26

### New features

- **`completionItem/resolve`** — documentation is fetched lazily when a completion item is focused in the menu, keeping the initial completion list instant; `resolve_provider: true` advertised in `CompletionOptions`.
- **`codeAction/resolve`** — edits for PHPDoc stub, "Implement missing methods", "Generate constructor", and "Generate getters/setters" are computed lazily when the action is selected; the action menu itself is instant.
- **`codeLens/resolve`** — code lens items use deferred resolution; pass-through handler completes the contract.
- **`inlayHint/resolve`** — hovering over a parameter-name or return-type inlay hint shows the full function/method signature as a tooltip; hint `data` carries `{"php_lsp_fn": name}` and is resolved via the existing `docs_for_symbol` helper.
- **`documentLink/resolve`** — deferred document link resolution supported.
- **`workspaceSymbol/resolve`** — `workspace/symbol` returns URI-only `WorkspaceLocation` items for speed; when a client resolves an item, the server fills in the full source `Location` (file + range).
- **`workspace/didChangeConfiguration`** — server pulls updated `phpVersion` and `excludePaths` from the client on every configuration change via `workspace/configuration`; takes effect without restarting.
- **Multi-root workspace** — all `workspaceFolders` are indexed at startup; `workspace/didChangeWorkspaceFolders` triggers incremental index updates and PSR-4 map rebuilds for added/removed roots.
- **Server-initiated refresh** — after workspace indexing or file changes, the server fires `workspace/semanticTokens/refresh`, `workspace/codeLens/refresh`, `workspace/inlayHint/refresh`, and `workspace/diagnostic/refresh` so all open editors immediately reflect updated analysis results.
- **`textDocument/linkedEditingRange`** — placing the cursor on any variable or symbol shows all its occurrences as linked ranges; editing one occurrence simultaneously edits all others (Alt+Shift+F2 in VS Code); returns the PHP word character pattern.
- **`window/showMessageRequest` + `window/showDocument` in test runner** — the "Run test" code lens now reports results via an interactive `showMessageRequest` with **Run Again** and **Open File** action buttons; clicking "Open File" opens the test file in the editor.
- **`docs_for_symbol` helper** — public function in `hover.rs` that looks up a symbol across all indexed docs and returns a formatted markdown string; shared by `completionItem/resolve` and `inlayHint/resolve`.

## [0.1.12] — 2026-03-25

### New features

- **PHP 8 enum support** — `enum` declarations are now first-class citizens throughout the server: hover shows the signature (including `implements`); semantic tokens emit a class token with `declaration` modifier; document symbols expose enum cases as `EnumMember` children and enum methods as `Method` children; workspace symbols index enums and their cases; completion suggests the enum name as `Enum` kind and each case as `EnumMember` (`SuitCase::Hearts`).
- **Attribute semantic tokens** (`#[Attr]`) — PHP 8 attribute names are emitted as `class` tokens in all semantic token responses. Applies to attributes on functions, parameters, classes, interfaces, traits, enums, methods, and properties so editors highlight them as class references.
- **Workspace scan progress** (`$/progress`) — a `window/workDoneProgress/create` request is sent to the client on startup, followed by `$/progress` Begin and End notifications bracketing the workspace scan. Editors that support work-done progress (VS Code, Neovim) will show a spinner/progress bar while indexing.

## [0.1.11] — 2026-03-25

### New features

- **Richer docblock parsing** — `@deprecated` (with optional message), `@throws`/`@throw` (class + description), `@see`, and `@link` tags are now parsed and rendered in hover responses. Deprecated symbols display a `> **Deprecated**` banner at the top of the hover tooltip.
- **Semantic token `deprecated` modifier** — functions, methods, classes, interfaces, and traits annotated with `@deprecated` now carry a `deprecated` modifier in semantic token responses, rendering with strikethrough in supporting editors (VS Code, Neovim with tree-sitter).
- **Semantic tokens range** (`textDocument/semanticTokens/range`) — clients can now request tokens for a visible viewport range rather than the entire file; the server filters the full token list to the requested range.
- **Semantic tokens delta** (`textDocument/semanticTokens/full/delta`) — incremental token updates: the server caches the previous token set per document (content-hashed `result_id`) and returns only the changed spans, reducing payload size for large files.
- **Type hierarchy dynamic registration** — `textDocument/prepareTypeHierarchy` is now registered dynamically via `client/registerCapability` in the `initialized` handler, making it discoverable by all LSP clients (fixes clients that inspect `serverCapabilities` at handshake time).
- **On-type formatting** (`textDocument/onTypeFormatting`) — two trigger characters:
  - `}` — de-indents the closing brace to align with its matching `{` line.
  - `\n` — copies the previous non-empty line's indentation; adds one extra indent level when the previous line ends with `{`.
- **File rename** (`workspace/willRenameFiles`, `workspace/didRenameFiles`) — moving or renaming a PHP file automatically updates all `use` import statements across the workspace to reflect the new PSR-4 fully-qualified class name; the index is kept current on `didRenameFiles`.
- **PHPDoc stub code action** — "Generate PHPDoc" code action offered for undocumented functions and methods; inserts a `/** ... */` stub with `@param` and `@return` tags inferred from the signature.
- **Document links** (`textDocument/documentLink`) — `include`, `require`, `include_once`, and `require_once` path arguments are returned as clickable document links.

## [0.1.7] — 2026-03-23

### New features

- **`workspace/executeCommand`** — server now advertises and handles `php-lsp.showReferences` (acknowledged, client handles the UI) and `php-lsp.runTest` (spawns `vendor/bin/phpunit --filter "ClassName::methodName"` in the project root and reports the result via `window/showMessage`). This makes code lens buttons functional.
- **Pull diagnostics** (`textDocument/diagnostic`) — implements the LSP 3.17 pull model alongside the existing push model. The server merges cached parse diagnostics with semantic diagnostics and returns them on demand. Preferred by Neovim 0.10+ and recent VS Code.

## [0.1.6] — 2026-03-23

### New features

- **Go-to-declaration** (`textDocument/declaration`) — jumps to the abstract method or interface method declaration rather than the concrete implementation; falls back to go-to-definition for concrete symbols.
- **Go-to-type-definition** (`textDocument/typeDefinition`) — resolves `$var` via `TypeMap` to find where its class is declared; also resolves non-variable identifiers via parameter type annotations.
- **Type hierarchy** (`textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`) — navigate the full class/interface inheritance chain; supertypes shows `extends`/`implements` parents, subtypes finds all implementing/extending types across the workspace.
- **Code lens** (`textDocument/codeLens`) — inline reference counts above every function, class, interface, and method; PHPUnit test methods get a "▶ Run test" lens with `php-lsp.runTest` command.
- **Document formatting** (`textDocument/formatting`, `textDocument/rangeFormatting`) — delegates to `php-cs-fixer` (PSR-12, preferred) or `phpcbf` via stdin; returns `None` gracefully if neither tool is installed.

## [0.1.5] — 2026-03-23

### New features

- **Document highlight** (`textDocument/documentHighlight`) — highlights every occurrence of the symbol under the cursor within the current file.
- **Go-to-implementation** (`textDocument/implementation`) — finds all classes that implement an interface or extend a class.
- **Semantic diagnostics** — warnings for calls to undefined functions/classes and for argument-count mismatches (too few or too many arguments).
- **Docblock parsing** — `/** ... */` annotations are now parsed and appended to hover responses (`@param`, `@return`, `@var`).
- **Return-type inlay hints** — a `: Type` label is shown after assigned function/method calls when the return type is known (e.g. `$x = make()` → `$x = make()`: `string`). `void` return types are suppressed.
- **Code actions** — "Add use import" quick-fix offered for undefined class names when the class is found in another indexed file.
- **Type-aware `->` completion** — when the receiver is a variable assigned via `new ClassName()`, completions are scoped to that class's methods instead of returning all methods in the file.
- **`use` statement awareness in find-references and rename** — renaming a class now also updates its `use` import lines; find-references includes `use` statement spans.
- **LRU eviction** — the workspace index is now capped at 10 000 indexed-only files; oldest entries are evicted when the limit is exceeded.

### Improvements

- **Debounce on `did_change`** — re-parse is delayed by 100 ms so rapid keystrokes don't queue redundant parse jobs.
- **`use_resolver` module** — new `UseMap` type resolves short class names to fully-qualified names via `use` statements (foundation for future namespace-aware features).
- **`type_map` module** — new `TypeMap` type infers variable types from `$var = new Foo()` assignments (used by typed `->` completion).

## [0.1.4] — 2026-03-22

### New features

- **Semantic tokens** (`textDocument/semanticTokens/full`) — richer syntax highlighting for functions, methods, classes, interfaces, traits, parameters, and properties with `declaration`, `static`, `abstract`, and `readonly` modifiers.
- **Selection range** (`textDocument/selectionRange`) — smart expand/shrink selection from expression → statement → function/class body → file.
- **Call hierarchy** (`textDocument/prepareCallHierarchy`, `incomingCalls`, `outgoingCalls`) — navigate callers and callees for any function or method, cross-file.
- **Async incremental re-parse** — `did_open` and `did_change` now parse off the tokio runtime via `spawn_blocking`; a version token discards stale results from superseded edits.
- **Vendor directory indexing** — the workspace scan now includes `vendor/` so cross-file features work on Composer dependencies (50 000-file cap).
- **PSR-4 autoload resolution** — reads `composer.json` and `vendor/composer/installed.json` to resolve fully-qualified class names to files on demand for go-to-definition.
- **`find_declaration_range`** — public helper in `definition.rs` used by the PSR-4 fallback to locate a class/function by short name in a freshly-loaded AST.

## [0.1.3] — 2026-03-21

### New features

- **Folding ranges** (`textDocument/foldingRange`) — collapse functions, classes, methods, loops, and control-flow blocks.
- **Inlay hints** (`textDocument/inlayHint`) — parameter name labels at call and method-call sites, with range filtering and multi-line argument support.

## [0.1.2] — 2026-03-20

### New features

- **Workspace indexing** — background scan on startup indexes all `*.php` files in the project; file watcher keeps the index current.
- **Cross-file go-to-definition** — jumps to symbols declared in other open/indexed documents.
- **Cross-file completion** — symbols from all indexed files appear in the default completion list (variables excluded from cross-file results).

## [0.1.1] — 2026-03-19

### New features

- **Find references** (`textDocument/references`) — locate all usages of a symbol across open documents.
- **Rename** (`textDocument/rename`, `textDocument/prepareRename`) — rename any function, method, or class across all open files.
- **Signature help** (`textDocument/signatureHelp`) — parameter hints while typing a call, triggered on `(` and `,`.
- **Workspace symbols** (`workspace/symbol`) — fuzzy-search symbols across all open documents.

## [0.1.0] — 2026-03-18

Initial release.

### Features

- Syntax diagnostics (parse errors reported in real time).
- Completion for keywords, functions, classes, interfaces, traits, methods, properties, and constants.
- Hover for function/method signatures and class declarations (with `extends`/`implements`).
- Go-to-definition (single-file).
- Document symbols (file outline).
