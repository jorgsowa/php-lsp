# Changelog

All notable changes to php-lsp are documented here.

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
