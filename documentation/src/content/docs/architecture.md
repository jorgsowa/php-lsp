---
title: Architecture
description: How the LSP layer and the mir static-analysis engine fit together.
---

php-lsp is a single-crate Rust project. It depends on the
**[mir-php](https://github.com/jorgsowa/mir)** family of crates for static
analysis — `mir-analyzer`, `mir-codebase`, `mir-issues`, `mir-types` — which
live in a separate repository and are published to crates.io independently.

- **`php-lsp`** — the LSP server ([tower-lsp](https://crates.io/crates/tower-lsp), [tokio](https://crates.io/crates/tokio)), communicates over stdin/stdout
- **[mir-php](https://github.com/jorgsowa/mir)** — external static analysis engine; no LSP dependency, usable standalone

## Local development with a patched mir

When working on both `php-lsp` and `mir-php` simultaneously, point Cargo at your
local checkout via a `[patch]` override in `Cargo.toml` (do not commit this):

```toml
[patch."https://github.com/jorgsowa/mir"]
mir-analyzer  = { path = "../mir/crates/mir-analyzer" }
mir-codebase  = { path = "../mir/crates/mir-codebase" }
mir-issues    = { path = "../mir/crates/mir-issues" }
mir-types     = { path = "../mir/crates/mir-types" }
```

## Request flow

```
Editor / AI agent
      │  stdin/stdout (JSON-RPC)
      ▼
  src/backend/server.rs    ← implements tower-lsp LanguageServer trait
      │
      ├── src/backend/handlers/    ← one handler file per feature family
      ├── src/document/            ← ASTs, raw text, diagnostics, open-file state
      ├── src/db/                  ← salsa query layer (memoized analysis)
      ├── src/index/               ← workspace scan + on-disk cache
      ├── src/lang/                ← config, PSR-4 autoload, docblock parser
      └── src/{feature}/           ← one module per LSP feature
```

## Key modules

| Module | Responsibility |
|---|---|
| `src/backend/` | Wires all modules; owns `DocumentStore`, `Psr4Map`, `PhpStormMeta`, `LspConfig` |
| `src/document/` | Text, parsed ASTs, diagnostics, open-file state |
| `src/db/` | Salsa-memoized queries: parse, index, codebase, semantic analysis |
| `src/index/` | Workspace scan (background, parallel via rayon) + on-disk cache |
| `src/lang/config` | `LspConfig` / `DiagnosticsConfig` / `FeaturesConfig` — all `initializationOptions` |
| `src/lang/autoload` | PSR-4 map from `composer.json` / `vendor/composer/installed.json` |
| `src/completion/` | Keyword, symbol, `->`, `::`, `\` namespace completions |
| `src/hover/` | Function/method/class/enum signatures + docblock annotations |
| `src/navigation/` | Go-to-definition, references, rename, call/type hierarchy |
| `src/analysis/` | Parse errors, semantic tokens, inlay hints, code lens |
| `src/actions/` | Code actions (extract, generate, implement, organize imports) |
| `src/lang/docblock` | Parse `/** */` annotations (`@param`, `@return`, `@var`, `@template`, …) |
| `src/navigation/walk` | AST traversal helpers |

## Design notes

- **Async parsing** — edits are debounced (default 100 ms, configurable via `initializationOptions.debounceMs`) and parsed in `spawn_blocking`; version tokens discard stale results.
- **Text sync** — `FULL` sync mode; raw text is stored immediately on change for instant feature response before parsing completes.
- **Two-tier document model** — open files carry a full `ParsedDoc` (~100 KB with arena + AST); background files store a lightweight `FileIndex` (~2 KB, declarations only) via salsa-memoized queries.
- **Workspace scan** — background task on `initialized`; 50k file cap; skips hidden dirs; excludes `vendor/` by default (set `indexVendor: true` to scan it eagerly; vendor files otherwise load on demand via PSR-4); respects `excludePaths` and `includePaths`.
- **On-disk cache** — `FileIndex` entries persisted under `~/.cache/php-lsp/`; warm starts skip re-parsing entirely.
- **Eager vs deferred code actions** — cheap actions (extract variable/method/constant, inline, organize imports) return full edits immediately; expensive actions (PHPDoc, constructor, getters/setters, return type) carry a `data` payload resolved by `codeAction/resolve` when the user selects them.
- **mir-php** — `mir_php::analyze(source, stmts, all)` accepts the current document as the first `all` entry for declaration-location tracking; the remaining entries are all other indexed documents.
