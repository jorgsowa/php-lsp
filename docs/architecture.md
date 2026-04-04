# Architecture

php-lsp is a Cargo workspace with two crates:

- **`php-lsp`** — the LSP server ([tower-lsp](https://crates.io/crates/tower-lsp), [tokio](https://crates.io/crates/tokio)), communicates over stdin/stdout
- **`mir-php`** — the static analysis engine; no LSP dependency, usable standalone

## Request flow

```
Editor / AI agent
      │  stdin/stdout (JSON-RPC)
      ▼
  backend.rs          ← implements tower-lsp LanguageServer trait
      │
      ├── document_store.rs   ← ASTs, raw text, diagnostics, LRU index
      ├── autoload.rs         ← PSR-4 FQN → file resolution
      ├── type_map.rs         ← variable → class inference
      ├── use_resolver.rs     ← short name → FQN via `use` statements
      └── <feature>.rs        ← one module per LSP feature
```

## Key modules

| Module | Responsibility |
|---|---|
| `backend.rs` | Wires all modules; owns `DocumentStore`, `Psr4Map`, `PhpStormMeta`, `LspConfig` |
| `document_store` | Text, parsed ASTs, diagnostics, LRU eviction (10k file cap) |
| `type_map` | Variable→class inference; trait resolution; constructor-promoted props |
| `use_resolver` | Resolve short class names to FQNs via `use` statements |
| `autoload` | PSR-4 map from `composer.json` / `vendor/composer/installed.json` |
| `completion` | Keyword, symbol, `->`, `::`, `\` namespace completions |
| `hover` | Function/method/class/enum signatures + docblock annotations |
| `definition` | Go-to-definition (cross-file + PSR-4 fallback) |
| `references` | Find all usages including `use` statements |
| `rename` | Rename across all indexed files |
| `diagnostics` | Parse errors via php-parser-rs |
| `semantic_diagnostics` | Bridges `mir_php::analyze` → LSP `Diagnostic` |
| `docblock` | Parse `/** */` annotations (`@param`, `@return`, `@var`, …) |
| `walk` | AST traversal helpers |
| `util` | Shared utilities (`word_at`, `fuzzy_camel_match`, `selected_text_range`, …) |

## Design notes

- **Async parsing** — edits are debounced 100 ms and parsed in `spawn_blocking`; version tokens discard stale results.
- **Text sync** — `FULL` sync mode; raw text is stored immediately on change for instant feature response before parsing completes.
- **Workspace scan** — background task on `initialized`; 50k file cap; skips hidden dirs; includes `vendor/`; respects `excludePaths`.
- **LRU eviction** — indexed-only files (not open in the editor) are evicted above 10k entries.
- **Eager vs deferred code actions** — cheap actions (extract variable/method/constant, inline, organize imports) return full edits immediately; expensive actions (PHPDoc, constructor, getters/setters, return type) strip their edit and carry a `data` payload resolved by `codeAction/resolve` when the user selects them.
- **mir-php** — `mir_php::analyze(source, stmts, all)` accepts the current document as the first `all` entry for declaration-location tracking; the remaining entries are all other indexed documents.
