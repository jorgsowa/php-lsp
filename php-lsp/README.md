# php-lsp

A PHP Language Server Protocol (LSP) implementation written in Rust.

## Features

### Language intelligence
- **Diagnostics** — syntax errors reported in real time; semantic warnings for undefined symbols, argument-count mismatches, undefined variables inside function/method bodies, return-type literal mismatches, null-safety violations, **`@deprecated` call warnings** — calling a function annotated with `@deprecated` emits a warning at the call site, and **duplicate declaration errors** — redefining a class, function, interface, trait, or enum in the same file emits an `Error`; workspace-wide diagnostics available for all indexed files (not just open ones); clients can refresh diagnostics on demand via `workspace/diagnostic/refresh`
- **Hover** — PHP signature for functions, methods, classes, interfaces, traits, and enums (including `implements`); **variable type hover** — hovering `$var` shows its inferred class; **property type hover** — hovering `$obj->propName` shows `(property) ClassName::$propName: TypeHint`; **built-in class hover** — hovering `PDO`, `DateTime`, `Exception`, etc. shows available methods from bundled stubs; **`use` alias hover** — hovering a name on a `use` import line shows its fully-qualified class name; includes `@param`/`@return`/`@throws`/`@deprecated`/`@see`/`@link`/`@template`/`@mixin` docblock annotations when present; deprecated symbols show a `> Deprecated` banner; built-in PHP functions include a link to the official [php.net](https://www.php.net) documentation
- **PHPDoc type system** — full docblock support: `@param`, `@return`, `@var`, `@throws`, `@deprecated`, `@see`, `@link`; `@template T` / `@template T of Base` generics; `@mixin ClassName`; `@psalm-type` / `@phpstan-type` type aliases; `@property`/`@property-read`/`@property-write` and `@method [static]` tags — synthesised members appear in `->` completions; callable type signatures `callable(int, string): void` parsed correctly
- **Go-to-definition** — jump to where a symbol is declared, including across open files and into Composer vendor packages via PSR-4 autoload maps
- **Go-to-implementation** — find all classes that implement an interface or extend a class
- **Find references** — locate every usage of a symbol across the workspace, including `use` import statements
- **Rename** — rename any function, method, or class across all open files, including its `use` import statements

### Editing aids
- **Completion** — keywords, ~200 built-in PHP functions, PHP superglobals (`$_SERVER`, `$_GET`, `$_POST`, etc.), classes, methods, properties, constants, enums, and enum cases; `->` and `?->` (nullsafe) completions scoped to the inferred receiver type; **built-in class stubs** — full member completions for PHP's standard library (Exception hierarchy, DateTime, PDO, SPL collections, Iterator/Countable/ArrayAccess interfaces, Closure, Generator, and more); **method-chain type inference** — `$result = $obj->method()` uses the method's return type hint or `@return` docblock to scope subsequent `$result->` completions; **`self`/`static` return types** — fluent builder chains resolve correctly; **union types** — `Foo|Bar` typed params show members from both classes; **`@param` docblock inference** — `@param Foo $x` in docblocks maps `$x` to `Foo` even without a PHP type hint; **`instanceof` type narrowing** — `if ($x instanceof Foo)` makes `Foo`'s members available in `$x->`; **constructor-chain** — `(new DateTime())->` completes DateTime's members; **bound-closure `$this`** — `Closure::bind` / `bindTo` / `call` map `$this` to the bound object's class; **`array_map`/`array_filter` propagation** — typed callback return type flows through `foreach` to the loop variable; `ClassName::`/`self::`/`static::` show static members and constants; `parent::` shows parent-class static members; `funcName(` offers named-argument (`param:`) completions; **`use` FQN completions** — typing `use ` suggests fully-qualified class names from the index; **sub-namespace `\` completions** — typing a partial namespace filters to matching FQNs; **`#[` attribute completions** — PHP 8 attribute classes suggested on `#[`; **`match` arm completions** — enum cases suggested inside `match ($var) {`; **`readonly` properties** — PHP 8.1 readonly properties shown with `readonly` detail; **magic method completions** — inside a class body `__construct`, `__get`, `__set`, `__toString`, `__invoke`, and 12 other magic methods are offered as snippets; **variable scope** — variable suggestions are limited to those declared before the cursor; cross-file symbols from all indexed documents; `@mixin ClassName` docblock causes mixin members to appear in `->` completions; **`@property`/`@method` tag completions** — synthesised members from class docblocks included in `->` completions; **enum built-ins** — `->name`, `->value` (backed enums), `::from()`, `::tryFrom()`, `::cases()`; **camel/underscore-case fuzzy matching** — typing `GRF` matches `getRecentFiles`, `str_r` matches `str_replace`; **snippet completions** — functions with parameters use snippet format so the cursor lands inside parentheses; **auto use-insertion** — selecting a class from another namespace automatically inserts the required `use` statement; **`completionItem/resolve`** — documentation is fetched lazily when a completion item is focused, keeping the menu instant
- **Signature help** — parameter hints while typing a call, including overload narrowing; signatures for ~150 PHP built-in functions are bundled so hints work without any external source
- **Inlay hints** — parameter name labels at call sites; return-type labels after assigned function calls, closures, and arrow functions; **`inlayHint/resolve`** — hovering over an inlay hint shows the full function/method signature as a tooltip
- **Code actions** — "Add use import" quick-fix for undefined class names; PHPDoc stub generation; "Implement missing methods" generates stubs for all abstract/interface methods not yet present; "Generate constructor"/"Generate getters/setters" from declared properties; "Extract variable" from a selection; **`codeAction/resolve`** — edits for PHPDoc, implement, constructor, and getters/setters are computed lazily when the action is selected, so the action menu appears instantly
- **Document links** — `include`/`require` paths are clickable links to the target file; **`documentLink/resolve`** supported
- **Linked editing** — placing the cursor on any variable or symbol shows all its occurrences as linked ranges; typing replaces all occurrences simultaneously (Alt+Shift+F2 in VS Code)

### Navigation
- **Document symbols** — file outline of all functions, classes, enums (with cases and methods), methods, properties, and constants
- **Workspace symbols** — fuzzy-search symbols across the entire project; **`workspaceSymbol/resolve`** fills in source ranges lazily for clients that request them
- **Call hierarchy** — incoming callers and outgoing callees for any function or method, including cross-file
- **Type hierarchy** — navigate supertypes and subtypes for classes and interfaces; registered dynamically so all LSP clients discover it correctly
- **Go-to-declaration** — jump to the abstract or interface declaration of a method
- **Go-to-type-definition** — jump to the class of the type of a variable
- **Selection range** — smart expand/shrink selection (Alt+Shift+→) from expression → statement → function/class → file
- **Document highlight** — highlights all occurrences of the symbol under the cursor in the current file
- **Folding ranges** — collapse functions, classes, methods, loops, and control-flow blocks; consecutive `use` import groups fold as a single region; multi-line comments fold; `// #region` / `// #endregion` markers create named foldable regions
- **Code lens** — inline reference counts on functions, classes, and methods; implementations count on interfaces and abstract classes; "overrides" label on methods that override a parent-class method; "Run test" lens for PHPUnit test methods — result shown via `window/showMessageRequest` with **Run Again** and **Open File** action buttons; **`codeLens/resolve`** supported

### Syntax & formatting
- **Semantic tokens** — richer syntax highlighting for functions, methods, classes, interfaces, traits, enums, parameters, properties, and PHP 8 `#[Attribute]` names with `declaration`/`static`/`abstract`/`readonly`/`deprecated` modifiers; symbols marked `@deprecated` render with strikethrough; supports full, range, and incremental delta requests; clients are notified to refresh via `workspace/semanticTokens/refresh` after indexing completes
- **On-type formatting** — auto-indents the new line on Enter; aligns `}` to its matching `{` on keypress
- **Formatting** — delegates to `php-cs-fixer` (PSR-12) or `phpcbf`; supports full-file and range formatting

### Workspace
- **Multi-root workspace** — all `workspaceFolders` are indexed at startup; folders added or removed at runtime via `workspace/didChangeWorkspaceFolders` trigger incremental scans and PSR-4 map updates
- **Live configuration** — the server registers `workspace/didChangeConfiguration` and pulls settings via `workspace/configuration` whenever the client changes them, so `phpVersion` and `excludePaths` take effect without restarting
- **Workspace indexing** — background scan indexes all `*.php` files on startup (including `vendor/`), with a 50 000-file cap; LRU eviction keeps memory bounded at 10 000 indexed-only files; progress is reported via `$/progress` so editors display a spinner; after indexing completes, semantic tokens, code lenses, inlay hints, and diagnostics are automatically refreshed in all open editors
- **PSR-4 resolution** — reads `composer.json` and `vendor/composer/installed.json` to resolve fully-qualified class names to files on demand; merged across all workspace roots in multi-root setups
- **PHPStorm metadata** — reads `.phpstorm.meta.php` from the workspace root and uses `override(ClassName::method(0), map([...]))` declarations to infer factory method return types
- **File watching** — index stays up to date when files are created, changed, or deleted on disk; open editors are refreshed automatically
- **File rename** — moving or renaming a PHP file automatically updates all `use` import statements across the workspace (`workspace/willRenameFiles`)
- **File create/delete lifecycle** — `workspace/willCreateFiles` indexes new files immediately; `workspace/willDeleteFiles` removes all `use` imports referencing the deleted file; `workspace/didDeleteFiles` drops the file from the index and clears its diagnostics
- **`textDocument/moniker`** — returns a PHP-scheme moniker with the PSR-4 FQN as the identifier, for cross-repository symbol linking
- **`textDocument/inlineValue`** — returns variable lookup entries in the requested range for debugger variable display; refreshed via `workspace/inlineValue/refresh`
- **Save lifecycle** — `textDocument/didSave` re-publishes diagnostics on save; `textDocument/willSave` and `willSaveWaitUntil` are registered so clients that gate on save notifications work correctly
- **Async parsing** — edits are debounced (100 ms) and parsed off the tokio runtime; stale results from superseded edits are discarded

## Configuration

Pass options via `initializationOptions` in your editor's LSP config:

```json
{
  "phpVersion": "8.1",
  "excludePaths": ["cache/*", "storage/*"]
}
```

The same options are also read live from the `php-lsp` settings section via `workspace/configuration`, so changes take effect without restarting.

## Installation

```bash
cargo install php-lsp
```

Or build from source:

```bash
git clone https://github.com/jorgsowa/php-lsp
cd php-lsp
cargo build --release
# binary at target/release/php-lsp
```

## Editor Setup

### PHPStorm (2023.2+)

1. Open **Settings → Languages & Frameworks → Language Servers**
2. Click **+** and configure:
   - **Name:** `php-lsp`
   - **Language:** `PHP`
   - **Command:** `/path/to/php-lsp`
3. Set file pattern to `*.php`

### Neovim (via nvim-lspconfig)

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "php",
  callback = function()
    vim.lsp.start({
      name = "php-lsp",
      cmd = { "/path/to/php-lsp" },
      root_dir = vim.fs.root(0, { "composer.json", ".git" }),
    })
  end,
})
```

### VS Code

Use the [custom LSP client extension](https://marketplace.visualstudio.com/items?itemName=llllvvuu.llllvvuu-lsp-client) or any extension that supports arbitrary LSP servers. Set the server command to the `php-lsp` binary.

## How It Works

The server communicates over stdin/stdout using the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/). It uses [php-ast](https://crates.io/crates/php-ast) (backed by [php-rs-parser](https://crates.io/crates/php-rs-parser) and a [bumpalo](https://crates.io/crates/bumpalo) arena) to parse PHP source into an AST, which is cached per document and reused across all requests.

## License

MIT
