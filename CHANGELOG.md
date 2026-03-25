# Changelog

All notable changes to php-lsp are documented here.

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
