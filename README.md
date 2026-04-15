# php-lsp

> A high-performance PHP language server written in Rust.

[![Build](https://github.com/jorgsowa/php-lsp/actions/workflows/release.yml/badge.svg)](https://github.com/jorgsowa/php-lsp/actions/workflows/release.yml)
[![Crates.io](https://img.shields.io/crates/v/php-lsp.svg)](https://crates.io/crates/php-lsp)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**[Getting Started](docs/getting-started.md)** · **[Features](docs/features.md)** · **[Editors & AI Clients](docs/editors.md)** · **[Configuration](docs/configuration.md)** · **[Architecture](docs/architecture.md)** · **[Contributing](CONTRIBUTING.md)**

---

php-lsp is a full-featured LSP implementation for PHP: real-time cross-file diagnostics, type-aware completions, navigation, and refactoring — written in Rust for low memory usage, fast startup, and zero GC pauses.

**Unique strengths:**
- **Full LSP 3.17 specification support** — call hierarchy, type hierarchy, semantic tokens, inlay hints, selection range, linked editing, and more — features that competing servers lock behind premium tiers or skip entirely
- **Rich code actions** — 10 actions (extract variable/method/constant, inline variable, generate constructor/getters/setters, implement missing methods, organize imports, add PHPDoc, add return type), all free
- **Clean separation of concerns** — parsing ([php-rs-parser](https://crates.io/crates/php-rs-parser), [php-ast](https://crates.io/crates/php-ast)) and static analysis ([mir-php](https://github.com/jorgsowa/mir)) are dedicated crates, keeping the LSP layer lightweight and focused purely on protocol features
- **Rust-native performance** — async-first with tokio, lock-free document store via dashmap, no GC pauses
- **Completion depth** — type-aware `->` / `::` chains, `match` enum-case completions, auto `use` insertion, fuzzy camel/underscore matching

---

## Install

```bash
cargo install php-lsp
```

Or download a pre-built binary (macOS, Linux) from [Releases](https://github.com/jorgsowa/php-lsp/releases) and place it on your `PATH`.

Verify:
```bash
php-lsp --version
```

For full installation options see **[docs/getting-started.md](docs/getting-started.md)**.

---

## Editor Setup

| Editor | Setup |
|---|---|
| VS Code | [php-lsp](https://marketplace.visualstudio.com/items?itemName=jorgsowa.php-lsp) extension |
| Neovim 0.11+ | Native `vim.lsp.enable` |
| Neovim 0.10 | `vim.lsp.start` in a `FileType` autocmd |
| Zed | `lsp` block in `~/.config/zed/settings.json` |
| Cursor | Settings → Features → Language Servers |
| PHPStorm | [php-lsp](https://plugins.jetbrains.com/plugin/31223-php-lsp) plugin ([source](https://github.com/jorgsowa/php-lsp-phpstorm-plugin)) |
| Claude Code | `claude plugin add https://github.com/jorgsowa/claude-php-lsp-plugin` |

Config snippets for every editor: **[docs/editors.md](docs/editors.md)**

---

## Configuration

Pass options via `initializationOptions`:

```json
{
  "phpVersion": "8.2",
  "excludePaths": ["cache/*", "storage/*"],
  "diagnostics": {
    "deprecatedCalls": false
  }
}
```

`phpVersion` is optional — the server auto-detects it in priority order: `config.platform.php` in `composer.json` (explicit platform pin), then the `php` binary on `$PATH` (actual runtime), then `require.php` in `composer.json` (compatibility range, last resort), then defaults to `8.5`. The detected version and its source are logged on startup. Set `phpVersion` explicitly to override, e.g. when running PHP inside Docker.

See **[docs/configuration.md](docs/configuration.md)** for all options including per-diagnostic toggles.

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

---

## Contributing

See **[CONTRIBUTING.md](CONTRIBUTING.md)**. Open an issue before starting non-trivial work. PRs require clean `cargo test` and `cargo clippy -- -D warnings`.

---

## License

[MIT](LICENSE)
