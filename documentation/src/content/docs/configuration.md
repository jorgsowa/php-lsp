---
title: Configuration
description: Every initializationOptions key, its default, and what it does.
---

Options are passed via `initializationOptions` in your editor's LSP configuration and are also read live from the `php-lsp` settings section via `workspace/configuration` — changes take effect without restarting the server.

## Options

All options are optional.

| Option | Type | Default | Description |
|---|---|---|---|
| `phpVersion` | `string` | auto-detected | PHP version used for version-gated diagnostics and completions. Accepted values: `"7.4"`, `"8.0"`, `"8.1"`, `"8.2"`, `"8.3"`, `"8.4"`, `"8.5"`. When omitted, the server auto-detects from `composer.json` (`config.platform.php`, then `require.php`), then from the `php` binary on `$PATH`, and falls back to `"8.5"`. |
| `excludePaths` | `string[]` | `[]` | Glob patterns for paths to skip during workspace indexing. Matched against paths relative to the workspace root. |
| `includePaths` | `string[]` | `[]` | Glob patterns for paths that must be indexed even if they match an `excludePaths` entry. Matched the same way as `excludePaths`, e.g. `["vendor/yiisoft"]` to index one package while `vendor/` itself stays excluded. |
| `indexVendor` | `boolean` | `true` | Eagerly walk and index `vendor/` during the workspace scan, so vendor classes appear in bare-name completion, workspace symbols, and find-implementations/type-hierarchy without extra configuration. This is a cheap declaration-only scan (no type inference), separate from the deeper background analysis that powers find-references/rename — vendor is never included in that background sweep, so this doesn't multiply analysis cost with vendor size. Set to `false` for very large vendor trees where even the cheap scan isn't worth it; vendor classes then still resolve on demand (composer autoload + per-file parse) when directly referenced. |
| `stubDirs` | `string[]` | `[]` | Directories of user-supplied PHP stub files to load in addition to the bundled built-ins, e.g. for PHP extensions or frameworks the bundled stubs don't cover. Each entry is resolved relative to the workspace root if not already absolute; every `.php` file found recursively is registered as a read-only, highest-precedence symbol source. |
| `diagnostics` | `object` | see below | Per-category diagnostic toggles. |
| `features` | `object` | see below | Per-feature capability toggles. |
| `maxIndexedFiles` | `number` | `50000` | Hard cap on the number of PHP files indexed during a workspace scan. Set lower to reduce memory on projects with very large vendor trees. |
| `warmAnalysis` | `boolean` | `true` | Background-analyze the workspace after indexing (and re-warm after edits settle) so find-references and rename answer from warm analysis caches instead of paying a cold per-file analysis at request time. Set to `false` to trade slower references for a smaller resident footprint. |
| `analysisCacheFlushIntervalMs` | `number` | `20000` | How often staged analysis-cache postings are persisted to disk in the background, bounding data loss on an unclean exit (crash, kill) to roughly one interval. |
| `debounceMs` | `number` | `100` | Delay in milliseconds between the last `textDocument/didChange` and the parse + analysis run. Set lower for fast machines, higher for slow machines or large files to reduce thrashing. |
| `debug` | `boolean` | `false` | Emit extra diagnostic log messages on startup: cache hit/miss ratio, workspace root paths, and PSR-4 namespace count. |
| `cachePath` | `string` | platform default | Override the on-disk analysis-cache directory (used verbatim, no schema-version or workspace-hash subdirectories appended). Falls back to `$XDG_CACHE_HOME`/`$HOME/.cache` on Unix, `%LOCALAPPDATA%` on Windows. Mainly useful for non-standard cache locations (containers, CI). |
| `externalTools` | `object` | see below | Optional PHPStan / PHPCS integration, run as external processes on save. |

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
| `unusedSymbols` | `false` | Unused-symbol warnings (unused variables, parameters, methods, properties, functions). Off by default so existing workspaces don't get a wave of new noisy warnings. |
| `missingTypes` | `false` | Missing type annotations on interface methods and class properties. |
| `mixedUsage` | `false` | Mixed-type usage lints: passing `mixed` to a typed parameter, assigning `mixed` to a typed property, array/property access on `mixed`, etc. |

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

### `externalTools` object

Two nested objects, `phpstan` and `phpcs`, each running the corresponding tool as a child
process and merging its findings into published diagnostics, alongside (not instead of) the
built-in analyzer. Both default to disabled: they can take seconds rather than milliseconds
and depend on project-specific configuration, so enabling one is an explicit per-workspace
opt-in. Findings are attributed with `source: "phpstan"` / `"phpcs"` on each diagnostic and
refresh after every `textDocument/didSave`.

#### `externalTools.phpstan` object

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Run PHPStan on save and merge its findings into diagnostics. |
| `binPath` | `"phpstan"` | Executable name or path, resolved via `$PATH` by default. Set to e.g. `"vendor/bin/phpstan"` for a project-local install. |
| `configPath` | none | Passed as `-c <path>`. When omitted, PHPStan uses its own discovery (`phpstan.neon` / `phpstan.neon.dist` in the workspace root). |

#### `externalTools.phpcs` object

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Run PHPCS on save and merge its findings into diagnostics. |
| `binPath` | `"phpcs"` | Executable name or path, resolved via `$PATH` by default. Set to e.g. `"vendor/bin/phpcs"` for a project-local install. |
| `standard` | none | Passed as `--standard=<value>` (e.g. `"PSR12"`). When omitted, PHPCS uses its own default/ruleset discovery. |

```json
{
  "externalTools": {
    "phpstan": { "enabled": true, "binPath": "vendor/bin/phpstan" },
    "phpcs": { "enabled": true, "standard": "PSR12" }
  }
}
```

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

For editor-specific snippets showing where to paste these options, see [editors.md](/editors/).
