# Configuration

Options are passed via `initializationOptions` in your editor's LSP configuration and are also read live from the `php-lsp` settings section via `workspace/configuration` — changes take effect without restarting the server.

## Options

All options are optional.

| Option | Type | Default | Description |
|---|---|---|---|
| `phpVersion` | `string` | auto-detected | PHP version used for version-gated diagnostics and completions. Accepted values: `"7.4"`, `"8.0"`, `"8.1"`, `"8.2"`, `"8.3"`, `"8.4"`, `"8.5"`. When omitted, the server auto-detects from `composer.json` (`config.platform.php`, then `require.php`), then from the `php` binary on `$PATH`, and falls back to `"8.5"`. |
| `excludePaths` | `string[]` | `[]` | Glob patterns for paths to skip during workspace indexing. Matched against paths relative to the workspace root. |
| `diagnostics` | `object` | see below | Per-category diagnostic toggles. |
| `features` | `object` | see below | Per-feature capability toggles. |
| `maxIndexedFiles` | `number` | `50000` | Hard cap on the number of PHP files indexed during a workspace scan. Set lower to reduce memory on projects with very large vendor trees. |

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

### `features` object

All flags default to `true` (enabled). Set a flag to `false` to suppress the corresponding entry from `ServerCapabilities` at negotiation time. This is useful when a client does not support a particular capability and you want to opt out cleanly.

| Key | Default | Description |
|---|---|---|
| `completion` | `true` | Code completion (`completionProvider`). |
| `hover` | `true` | Hover documentation (`hoverProvider`). |
| `definition` | `true` | Go-to-definition (`definitionProvider`). |
| `declaration` | `true` | Go-to-declaration (`declarationProvider`). |
| `references` | `true` | Find references (`referencesProvider`). |
| `documentSymbols` | `true` | Document symbol list (`documentSymbolProvider`). |
| `workspaceSymbols` | `true` | Workspace symbol search (`workspaceSymbolProvider`). |
| `rename` | `true` | Rename symbol (`renameProvider`). |
| `signatureHelp` | `true` | Signature help (`signatureHelpProvider`). |
| `inlayHints` | `true` | Inlay hints (`inlayHintProvider`). |
| `semanticTokens` | `true` | Semantic token highlighting (`semanticTokensProvider`). |
| `selectionRange` | `true` | Smart selection ranges (`selectionRangeProvider`). |
| `callHierarchy` | `true` | Call hierarchy (`callHierarchyProvider`). |
| `documentHighlight` | `true` | Document highlight (`documentHighlightProvider`). |
| `implementation` | `true` | Go-to-implementation (`implementationProvider`). |
| `codeAction` | `true` | Code actions (`codeActionProvider`). |
| `typeDefinition` | `true` | Go-to-type-definition (`typeDefinitionProvider`). |
| `codeLens` | `true` | Code lens (`codeLensProvider`). |
| `formatting` | `true` | Full-document formatting (`documentFormattingProvider`). |
| `rangeFormatting` | `true` | Range formatting (`documentRangeFormattingProvider`). |
| `onTypeFormatting` | `true` | On-type formatting (`documentOnTypeFormattingProvider`). |
| `documentLink` | `true` | Document links (`documentLinkProvider`). |
| `linkedEditingRange` | `true` | Linked editing ranges (`linkedEditingRangeProvider`). |
| `inlineValues` | `true` | Inline values (`inlineValueProvider`). |

## Example

```json
{
  "phpVersion": "8.1",
  "excludePaths": ["cache/*", "storage/*", "tests/fixtures/*"],
  "diagnostics": {
    "enabled": true,
    "undefinedVariables": true,
    "deprecatedCalls": false
  },
  "features": {
    "callHierarchy": false,
    "inlineValues": false
  }
}
```

For editor-specific snippets showing where to paste these options, see [editors.md](editors.md).
