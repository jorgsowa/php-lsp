# php-lsp

A PHP Language Server written in Rust — diagnostics, completions, hover, go-to-definition, rename, refactoring, and more.

**[Features](docs/features.md)** · **[Configuration](docs/configuration.md)** · **[Architecture](docs/architecture.md)** · **[Contributing](CONTRIBUTING.md)**

## Install

```bash
cargo install php-lsp
```

Or download a pre-built binary from [Releases](https://github.com/jorgsowa/php-lsp/releases).

---

## AI Agents

### Claude Code

Install the [Claude Code plugin](https://github.com/jorgsowa/claude-php-lsp-plugin):

```bash
claude plugin add https://github.com/jorgsowa/claude-php-lsp-plugin
```

### Cursor

Add to `.cursor/mcp.json` or open **Settings → Features → Language Servers** and set:
- **Command:** `php-lsp`
- **File pattern:** `*.php`

### Zed

In `~/.config/zed/settings.json`:

```json
{
  "lsp": {
    "php-lsp": {
      "binary": {
        "path": "php-lsp"
      }
    }
  }
}
```

---

## IDEs

### VS Code

Install any extension that supports custom LSP servers (e.g. [llllvvuu-lsp-client](https://marketplace.visualstudio.com/items?itemName=llllvvuu.llllvvuu-lsp-client)) and set the server command to `php-lsp`.

### Neovim 0.11+

Drop this file into `~/.config/nvim/lsp/php_lsp.lua`:

```lua
---@type vim.lsp.Config
return {
  cmd = { 'php-lsp' },
  filetypes = { 'php' },
  root_markers = { 'composer.json', '.git' },
  workspace_required = true,
}
```

Then enable it in `init.lua`:

```lua
vim.lsp.enable('php_lsp')
```

#### Neovim 0.10 and older

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "php",
  callback = function()
    vim.lsp.start({
      name = "php-lsp",
      cmd = { "php-lsp" },
      root_dir = vim.fs.root(0, { "composer.json", ".git" }),
    })
  end,
})
```

### PHPStorm (2023.2+)

**Settings → Languages & Frameworks → Language Servers → +**

- **Name:** `php-lsp`
- **Language:** `PHP`
- **Command:** `php-lsp`

---

## Configuration

Pass via `initializationOptions`:

```json
{
  "phpVersion": "8.1",
  "excludePaths": ["cache/*", "storage/*"]
}
```

---

## License

[MIT](LICENSE)
