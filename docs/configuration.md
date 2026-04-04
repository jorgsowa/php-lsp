# Configuration

Options are passed via `initializationOptions` in your editor's LSP configuration and are also read live from the `php-lsp` settings section via `workspace/configuration` — changes take effect without restarting the server.

## Options

| Option | Type | Default | Description |
|---|---|---|---|
| `phpVersion` | `string` | `"8.3"` | PHP version used for version-gated diagnostics and completions. Accepted values: `"7.4"`, `"8.0"`, `"8.1"`, `"8.2"`, `"8.3"`. |
| `excludePaths` | `string[]` | `[]` | Glob patterns for paths to skip during workspace indexing. Matched against paths relative to the workspace root. |

## Example

```json
{
  "phpVersion": "8.1",
  "excludePaths": ["cache/*", "storage/*", "tests/fixtures/*"]
}
```

## Editor-specific setup

### VS Code (`settings.json`)

```json
{
  "php-lsp.phpVersion": "8.2",
  "php-lsp.excludePaths": ["cache/*"]
}
```

### Neovim

Pass `init_options` in `vim.lsp.start`:

```lua
vim.lsp.start({
  name = "php-lsp",
  cmd = { "php-lsp" },
  root_dir = vim.fs.root(0, { "composer.json", ".git" }),
  init_options = {
    phpVersion = "8.2",
    excludePaths = { "cache/*" },
  },
})
```

### Claude Code (`.claude/settings.json`)

```json
{
  "lsp": {
    "php-lsp": {
      "command": "php-lsp",
      "extensionToLanguage": {
        ".php": "php"
      },
      "initializationOptions": {
        "phpVersion": "8.2",
        "excludePaths": ["cache/*"]
      }
    }
  }
}
```
