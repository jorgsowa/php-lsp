# Configuration

Options are passed via `initializationOptions` in your editor's LSP configuration and are also read live from the `php-lsp` settings section via `workspace/configuration` — changes take effect without restarting the server.

## Options

All options are optional.

| Option | Type | Default | Description |
|---|---|---|---|
| `phpVersion` | `string` | auto-detected | PHP version used for version-gated diagnostics and completions. Accepted values: `"7.4"`, `"8.0"`, `"8.1"`, `"8.2"`, `"8.3"`, `"8.4"`, `"8.5"`. When omitted, the server auto-detects from `composer.json` (`config.platform.php`, then `require.php`), then from the `php` binary on `$PATH`, and falls back to `"8.5"`. |
| `excludePaths` | `string[]` | `[]` | Glob patterns for paths to skip during workspace indexing. Matched against paths relative to the workspace root. |
| `diagnostics` | `object` | see below | Per-category diagnostic toggles. |

### `diagnostics` object

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Master switch — diagnostics are off by default; set to `true` to emit them. |
| `undefinedVariables` | `true` | Undefined variable references. |
| `undefinedFunctions` | `true` | Calls to undefined functions. |
| `undefinedClasses` | `true` | References to undefined classes, interfaces, or traits. |
| `arityErrors` | `true` | Wrong number of arguments passed to a function. |
| `typeErrors` | `true` | Return-type mismatches. |
| `deprecatedCalls` | `true` | Calls to `@deprecated` members. |
| `duplicateDeclarations` | `true` | Duplicate class or function declarations. |

## Example

```json
{
  "phpVersion": "8.1",
  "excludePaths": ["cache/*", "storage/*", "tests/fixtures/*"],
  "diagnostics": {
    "enabled": true,
    "undefinedVariables": true,
    "deprecatedCalls": false
  }
}
```

For editor-specific snippets showing where to paste these options, see [editors.md](editors.md).
