# php-lsp

A PHP Language Server Protocol (LSP) implementation written in Rust.

## Features

- **Diagnostics** — syntax errors reported in real time as you type
- **Completion** — keywords, functions, classes, interfaces, traits, methods, properties, constants, and cross-file symbols from all open documents
- **Hover** — signatures for functions, classes (with `extends`/`implements`), interfaces, and traits
- **Go-to-definition** — jump to where a symbol is declared, including across open files
- **Document symbols** — outline of all functions, classes, methods, properties, and constants in the file

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
