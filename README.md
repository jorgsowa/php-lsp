# php-lsp

A PHP Language Server Protocol (LSP) implementation written in Rust.

## Features

### Language intelligence
- **Diagnostics** — syntax errors reported in real time; semantic warnings for undefined symbols and argument-count mismatches
- **Hover** — PHP signature for functions, methods, classes, interfaces, and traits; includes `@param`/`@return` docblock annotations when present
- **Go-to-definition** — jump to where a symbol is declared, including across open files and into Composer vendor packages via PSR-4 autoload maps
- **Go-to-implementation** — find all classes that implement an interface or extend a class
- **Find references** — locate every usage of a symbol across the workspace, including `use` import statements
- **Rename** — rename any function, method, or class across all open files, including its `use` import statements

### Editing aids
- **Completion** — keywords, functions, classes, methods, properties, constants; `->` completions scoped to the inferred receiver type (`$obj = new Foo()` → `$obj->` shows only `Foo`'s methods); cross-file symbols from all indexed documents
- **Signature help** — parameter hints while typing a call, including overload narrowing
- **Inlay hints** — parameter name labels at call sites; return-type labels after assigned function calls
- **Code actions** — "Add use import" quick-fix for undefined class names

### Navigation
- **Document symbols** — file outline of all functions, classes, methods, properties, and constants
- **Workspace symbols** — fuzzy-search symbols across the entire project
- **Call hierarchy** — incoming callers and outgoing callees for any function or method, including cross-file
- **Selection range** — smart expand/shrink selection (Alt+Shift+→) from expression → statement → function/class → file
- **Document highlight** — highlights all occurrences of the symbol under the cursor in the current file
- **Folding ranges** — collapse functions, classes, methods, loops, and control-flow blocks

### Syntax
- **Semantic tokens** — richer syntax highlighting for functions, methods, classes, interfaces, traits, parameters, and properties with `declaration`/`static`/`abstract`/`readonly` modifiers

### Workspace
- **Workspace indexing** — background scan indexes all `*.php` files on startup (including `vendor/`), with a 50 000-file cap; LRU eviction keeps memory bounded at 10 000 indexed-only files
- **PSR-4 resolution** — reads `composer.json` and `vendor/composer/installed.json` to resolve fully-qualified class names to files on demand
- **File watching** — index stays up to date when files are created, changed, or deleted on disk
- **Async parsing** — edits are debounced (100 ms) and parsed off the tokio runtime; stale results from superseded edits are discarded

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

The server communicates over stdin/stdout using the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/). It uses [php-parser-rs](https://github.com/php-rust-tools/parser) to parse PHP source into an AST, which is cached per document and reused across all requests.

## License

MIT
