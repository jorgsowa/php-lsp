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

## Comparison

| Server | Language | License | Maintained |
|---|---|---|---|
| **php-lsp** | Rust | ✅ Free/OSS | ✅ Active |
| Intelephense | TypeScript | ⚠️ Freemium | ✅ Active |
| PHPantom | Rust | ✅ Free/OSS | ✅ Active |
| Phpactor | PHP | ✅ Free/OSS | ✅ Active |
| DEVSENSE | Node.js | 🔒 Paid | ✅ Active |
| Psalm LSP | PHP | ✅ Free/OSS | ✅ Active |
| phpls | Go | ✅ Free/OSS | ✅ Active |
| felixfbecker | PHP | ✅ Free/OSS | ❌ Abandoned |

**Feature matrix:**

| Feature | php-lsp | Intelephense | PHPantom | Phpactor | DEVSENSE | Psalm |
|---|---|---|---|---|---|---|
| Completion | ✅ | ✅ | ✅ | ✅ | ✅ | ⚠️ limited |
| Hover | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Go-to-definition | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Go-to-declaration | ✅ | 🔒 Premium | ❌ | ✅ | ✅ | ❌ |
| Go-to-type-definition | ✅ | 🔒 Premium | ❌ | ❌ | ✅ | ❌ |
| Find references | ✅ | ✅ | ❌ | ✅ | ✅ | ❌ |
| Rename | ✅ | 🔒 Premium | ✅ | ✅ | ✅ | ❌ |
| Call hierarchy | ✅ | ❌ | ❌ | ❌ | ✅ | ❌ |
| Type hierarchy | ✅ | 🔒 Premium | ❌ | ❌ | ✅ | ❌ |
| Implementations | ✅ | 🔒 Premium | ❌ | ✅ | ✅ | ❌ |
| Semantic tokens | ✅ | ❌ | ❌ | ❌ | ✅ | ❌ |
| Inlay hints | ✅ | ❌ | ❌ | ✅ | ✅ | ❌ |
| Code lens | ✅ | 🔒 Premium | ❌ | ❌ | ✅ | ❌ |
| Signature help | ✅ | ✅ | ❌ | ✅ | ✅ | ❌ |
| Selection range | ✅ | ❌ | ❌ | ❌ | ✅ | ❌ |
| Document highlight | ✅ | ❌ | ❌ | ❌ | ✅ | ❌ |
| Folding | ✅ | 🔒 Premium | ❌ | ❌ | ✅ | ❌ |
| On-type formatting | ✅ | ❌ | ❌ | ❌ | ✅ | ❌ |
| Document links | ✅ | ❌ | ❌ | ❌ | ✅ | ❌ |
| PSR-4 autoload | ✅ | ✅ | ❌ | ✅ | ✅ | ❌ |
| PhpStorm meta | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Static analysis | ✅ | ✅ | ✅ | ✅ | ✅ | ✅✅ |
| Embedded HTML/JS/CSS | ❌ | ✅ | ❌ | ❌ | ✅ | ❌ |
| Laravel/framework aware | ❌ | ⚠️ plugin | ✅ built-in | ❌ | ⚠️ plugin | ❌ |
| Debugger | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ |
| Deep generics / PHPStan types | ⚠️ partial | ⚠️ partial | ✅ | ❌ | ✅ | ✅ |

**Where php-lsp is strong:**

- **Rust-based** — no GC pauses, async-first with `tokio`, lock-free document store via `dashmap`
- **mir-php static analysis** — two-pass cross-file engine: undefined vars/functions, arity errors, type mismatches, deprecated calls
- **PhpStorm metadata** — the only open-source LSP that parses `.phpstorm.meta.php` for DI container type inference
- **Breadth of LSP coverage** — call/type hierarchy, semantic tokens, inlay hints, selection range, and 10 code action types all free
- **Completion depth** — type-aware chains, `match` enum completions, named args, auto `use` insertion, camel/underscore fuzzy matching

**Where others are stronger:**

- **Intelephense / DEVSENSE** — embedded HTML/JS/CSS intelligence inside PHP files; more battle-tested on very large codebases
- **PHPantom** — deeper generics, PHPStan annotations, conditional return types; built-in Laravel Eloquent and Drupal support
- **Psalm** — strictest type analysis available; best-in-class for type correctness at the cost of IDE feature breadth
- **DEVSENSE** — integrated debugger, PHPUnit UI, professional support
- **php-lsp** — newer and has a smaller community than Intelephense or Phpactor

---

## License

[MIT](LICENSE)
