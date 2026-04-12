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

### PHPStorm

> **Native plugin:** A dedicated php-lsp plugin for PhpStorm is currently [under review on the JetBrains Marketplace](https://plugins.jetbrains.com/plugin/31223-php-lsp). Once approved, it will offer a simpler setup than the LSP4IJ approach below.

1. Install the [LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij) plugin (**Settings → Plugins → Marketplace → "LSP4IJ"**).
2. Go to **Settings → Languages & Frameworks → LSP → Language Servers → +**, choose **Custom server**, and fill in the **Server** tab:

   | Field | Value |
   |---|---|
   | Name | `php-lsp` |
   | Command | `<path-to-php-lsp>` |

3. In the **Mappings** tab add file name pattern `*.php`.
4. In the **Configuration** tab paste your options into the **Initialization options** JSON field:

```json
{
  "phpVersion": "8.3",
  "excludePaths": ["cache/*", "storage/*"]
}
```

See [configuration.md](configuration.md) for all available options.

> **Known issue:** LSP4IJ throws an `UnsupportedOperationException` on `workspace/inlineValue/refresh` (tracked in [redhat-developer/lsp4ij#1470](https://github.com/redhat-developer/lsp4ij/issues/1470)). Update LSP4IJ to the latest version once a fix is released.

---

## Configuration reference

See [configuration.md](configuration.md) for all available `initializationOptions`.
