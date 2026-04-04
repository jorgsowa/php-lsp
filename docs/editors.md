# Editor & AI Client Setup

This page covers setup for every supported editor and AI coding client.

## Finding the binary path

The path to the `php-lsp` binary depends on how you installed it.

**Installed via `cargo install php-lsp`:**
```
~/.cargo/bin/php-lsp
```

**Downloaded a pre-built binary** from [Releases](https://github.com/jorgsowa/php-lsp/releases):

Move it somewhere on your PATH, e.g.:
```bash
sudo mv php-lsp /usr/local/bin/php-lsp
```

Then the path is `/usr/local/bin/php-lsp`.

You can verify the path with:
```bash
which php-lsp
```

---

## AI Coding Clients

### Claude Code

Install the official plugin:

```bash
claude plugin add https://github.com/jorgsowa/claude-php-lsp-plugin
```

The plugin configures everything automatically. The server binary must be on your PATH (or set the full path in the plugin's `.lsp.json` after installation).

To override `initializationOptions`, edit `.lsp.json` in the plugin directory:

```json
{
  "command": "/usr/local/bin/php-lsp",
  "initializationOptions": {
    "phpVersion": "8.2",
    "excludePaths": ["cache/*"]
  }
}
```

---

### Cursor

Open **Settings → Features → Language Servers → +** and set:

| Field | Value |
|---|---|
| Command | `/usr/local/bin/php-lsp` |
| File pattern | `*.php` |

Or add to `.cursor/mcp.json` in your project root:

```json
{
  "languageServers": {
    "php-lsp": {
      "command": "/usr/local/bin/php-lsp",
      "filetypes": ["php"],
      "initializationOptions": {
        "phpVersion": "8.3"
      }
    }
  }
}
```

---

## Editors

### Zed

Add to `~/.config/zed/settings.json`:

```json
{
  "lsp": {
    "php-lsp": {
      "binary": {
        "path": "/usr/local/bin/php-lsp"
      },
      "initialization_options": {
        "phpVersion": "8.3",
        "excludePaths": []
      }
    }
  },
  "languages": {
    "PHP": {
      "language_servers": ["php-lsp"]
    }
  }
}
```

---

### VS Code

1. Install an extension that supports custom LSP servers, such as [llllvvuu-lsp-client](https://marketplace.visualstudio.com/items?itemName=llllvvuu.llllvvuu-lsp-client).
2. Add to your `.vscode/settings.json` (project) or `~/.config/Code/User/settings.json` (global):

```json
{
  "llllvvuu-lsp-client.servers": {
    "php-lsp": {
      "command": "/usr/local/bin/php-lsp",
      "filetypes": ["php"],
      "initializationOptions": {
        "phpVersion": "8.3",
        "excludePaths": []
      }
    }
  }
}
```

---

### Neovim 0.11+

Create `~/.config/nvim/lsp/php_lsp.lua`:

```lua
---@type vim.lsp.Config
return {
  cmd = { '/usr/local/bin/php-lsp' },
  filetypes = { 'php' },
  root_markers = { 'composer.json', '.git' },
  workspace_required = true,
  init_options = {
    phpVersion = '8.3',
    excludePaths = {},
  },
}
```

Then enable it in `init.lua`:

```lua
vim.lsp.enable('php_lsp')
```

---

### Neovim 0.10 and older

Add to `init.lua`:

```lua
vim.api.nvim_create_autocmd('FileType', {
  pattern = 'php',
  callback = function()
    vim.lsp.start({
      name = 'php-lsp',
      cmd = { '/usr/local/bin/php-lsp' },
      root_dir = vim.fs.root(0, { 'composer.json', '.git' }),
      init_options = {
        phpVersion = '8.3',
        excludePaths = {},
      },
    })
  end,
})
```

---

### PHPStorm (2023.2+)

Go to **Settings → Languages & Frameworks → Language Servers → +** and fill in:

| Field | Value |
|---|---|
| Name | `php-lsp` |
| Language | `PHP` |
| Command | `/usr/local/bin/php-lsp` |

To pass `initializationOptions`, PHPStorm does not have a built-in UI for this. Use a wrapper script as the command:

Create `~/bin/php-lsp-wrapper.sh`:

```bash
#!/bin/sh
exec /usr/local/bin/php-lsp
```

PHPStorm reads `initializationOptions` from a JSON file if your LSP client supports it — check your PHPStorm version's Language Server documentation for the exact field name.

---

## Configuration reference

See [configuration.md](configuration.md) for all available `initializationOptions`.
