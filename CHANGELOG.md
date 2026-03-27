# Changelog

All notable changes to php-lsp are documented here.

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
