# php-lsp

A PHP Language Server written in Rust — diagnostics, completions, hover, go-to-definition, rename, refactoring, and more.

**[Features](docs/features.md)** · **[Editors & AI Clients](docs/editors.md)** · **[Configuration](docs/configuration.md)** · **[Architecture](docs/architecture.md)** · **[Contributing](CONTRIBUTING.md)**

## Install

```bash
cargo install php-lsp
```

Or download a pre-built binary from [Releases](https://github.com/jorgsowa/php-lsp/releases).

---

## Setup

For full setup instructions for all editors and AI clients (Claude Code, Cursor, Zed, VS Code, Neovim, PHPStorm) see **[docs/editors.md](docs/editors.md)**.

The binary path after `cargo install` is `~/.cargo/bin/php-lsp`. Run `which php-lsp` to confirm.

---

## Configuration

Pass options via `initializationOptions`:

```json
{
  "phpVersion": "8.1",
  "excludePaths": ["cache/*", "storage/*"]
}
```

See **[docs/configuration.md](docs/configuration.md)** for all options.

---

## Why php-lsp?

The only free, open-source PHP language server with enterprise-grade feature completeness.

| Server | Language | License | Semantic Tokens | Inlay Hints | Call Hierarchy | Type Hierarchy | Code Actions |
|---|---|---|---|---|---|---|---|
| **php-lsp** | Rust | Free/OSS | ✓ | ✓ | ✓ | ✓ | 10 types |
| Intelephense | TypeScript | Freemium | ✗ | ✗ | ✗ | Premium | ~3 free |
| PHPantom | Rust | Free/OSS | ✗ | ✗ | ✗ | ✗ | ~4 |
| Phpactor | PHP | Free/OSS | ✗ | ✓ | ✗ | ✗ | ~6 |
| DEVSENSE | Node.js | Paid | ✓ | ✓ | ✓ | ✓ | ~8 |
| Psalm LSP | PHP | Free/OSS | ✗ | ✗ | ✗ | ✗ | ✗ |
| phpls | Go | Free/OSS | ✗ | ✗ | ✗ | ✗ | ✗ |

**Full feature comparison:**

| Feature | php-lsp | Intelephense | PHPantom | Phpactor | DEVSENSE |
|---|---|---|---|---|---|
| Completion | ✓ | ✓ | ✓ | ✓ | ✓ |
| Hover | ✓ | ✓ | ✓ | ✓ | ✓ |
| Go-to-definition | ✓ | ✓ | ✓ | ✓ | ✓ |
| Go-to-declaration | ✓ | Premium | ✗ | ✓ | ✓ |
| Go-to-type-definition | ✓ | Premium | ✗ | ✗ | ✓ |
| Find references | ✓ | ✓ | ✗ | ✓ | ✓ |
| Rename | ✓ | Premium | ✓ | ✓ | ✓ |
| Call hierarchy | ✓ | ✗ | ✗ | ✗ | ✓ |
| Type hierarchy | ✓ | Premium | ✗ | ✗ | ✓ |
| Implementations | ✓ | Premium | ✗ | ✓ | ✓ |
| Semantic tokens | ✓ | ✗ | ✗ | ✗ | ✓ |
| Inlay hints | ✓ | ✗ | ✗ | ✓ | ✓ |
| Code lens | ✓ | Premium | ✗ | ✗ | ✓ |
| Signature help | ✓ | ✓ | ✗ | ✓ | ✓ |
| Selection range | ✓ | ✗ | ✗ | ✗ | ✓ |
| Document highlight | ✓ | ✗ | ✗ | ✗ | ✓ |
| Folding | ✓ | Premium | ✗ | ✗ | ✓ |
| On-type formatting | ✓ | ✗ | ✗ | ✗ | ✓ |
| Document links | ✓ | ✗ | ✗ | ✗ | ✓ |
| PSR-4 autoload | ✓ | ✓ | ✗ | ✓ | ✓ |
| PhpStorm meta | ✓ | ✗ | ✗ | ✗ | ✗ |
| Static analysis | ✓ | ✓ | ✓ | ✓ | ✓ |

**Key advantages:**

- **Rust-based** — no GC pauses, async-first with `tokio`, lock-free document store via `dashmap`
- **mir-php static analysis** — two-pass cross-file engine: undefined vars/functions, arity errors, type mismatches, deprecated calls
- **PhpStorm metadata** — the only open-source LSP that parses `.phpstorm.meta.php` for DI container type inference
- **Deepest completion engine** — type-aware `->` / `::` chains, `match` arm enum completions, named args, attribute completions, auto `use` insertion, camel/underscore fuzzy matching
- **10 code action types** — extract variable/method/constant, inline variable, implement methods, add PHPDoc, generate constructor/getters/setters, organize imports, add return type

---

## License

[MIT](LICENSE)
