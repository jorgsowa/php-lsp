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
        "phpVersion": "8.5"
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
        "phpVersion": "8.5",
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

Install the [php-lsp](https://marketplace.visualstudio.com/items?itemName=jorgsowa.php-lsp) extension ([source](https://github.com/jorgsowa/php-lsp-vscode-plugin)). It ships a pre-built binary — no separate install required.

Via the Quick Open palette (`Ctrl+P` / `Cmd+P`):

```
ext install jorgsowa.php-lsp
```

Available settings (VS Code `settings.json`):

| Setting | Default | Description |
|---|---|---|
| `php-lsp.serverPath` | *(auto)* | Path to the `php-lsp` binary; leave empty for auto-detection |
| `php-lsp.phpVersion` | `8.5` | PHP version (`7.4` – `8.5`) |
| `php-lsp.excludePaths` | `[]` | Glob patterns to exclude from the workspace |
| `php-lsp.diagnostics.*` | `true` | Per-diagnostic toggles (undefined variables/functions/classes, arity errors, type mismatches, deprecated calls, duplicate declarations) |

> **Note:** If Intelephense (or another PHP extension) is installed, it will conflict with php-lsp. Disable it via **Extensions → Intelephense → Disable** before using this extension.

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
    phpVersion = '8.5',
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
        phpVersion = '8.5',
        excludePaths = {},
      },
    })
  end,
})
```

---

### PHPStorm

Install the [php-lsp](https://plugins.jetbrains.com/plugin/31223-php-lsp) plugin from the JetBrains Marketplace (**Settings → Plugins → Marketplace → "php-lsp"**).

The plugin handles everything automatically — no manual server configuration required. Source is available at [jorgsowa/php-lsp-phpstorm-plugin](https://github.com/jorgsowa/php-lsp-phpstorm-plugin).

See [configuration.md](configuration.md) for all available options.

---

## Configuration reference

See [configuration.md](configuration.md) for all available `initializationOptions`.
