# Getting Started

This guide takes you from zero to a working php-lsp setup in about five minutes.

## Step 1 — Install

Choose the method that suits your environment.

**Cargo** (recommended if you have a Rust toolchain):
```bash
cargo install php-lsp
```
The binary is placed at `~/.cargo/bin/php-lsp`.

**Pre-built binary** (no Rust required):

Download the binary for your platform from [Releases](https://github.com/jorgsowa/php-lsp/releases):

| File | Platform |
|---|---|
| `php-lsp-aarch64-apple-darwin` | macOS — Apple Silicon |
| `php-lsp-x86_64-apple-darwin` | macOS — Intel |
| `php-lsp-x86_64-unknown-linux-gnu` | Linux — 64-bit |

Then place it on your `PATH`:
```bash
sudo mv php-lsp /usr/local/bin/php-lsp
chmod +x /usr/local/bin/php-lsp
```

**Build from source:**
```bash
git clone https://github.com/jorgsowa/php-lsp
cd php-lsp
cargo build --release
# binary at target/release/php-lsp
```

## Step 2 — Verify

```bash
php-lsp --version
# php-lsp 0.1.50
```

If you get `command not found`, ensure the install directory is on your `PATH`:
```bash
# Cargo installs
export PATH="$HOME/.cargo/bin:$PATH"

# Manual installs — check with:
which php-lsp
```

## Step 3 — Connect your editor

php-lsp communicates over stdin/stdout and works with any editor that supports custom LSP servers. Follow the guide for your editor:

- **[VS Code](editors.md#vs-code)**
- **[Neovim 0.11+](editors.md#neovim-011)**
- **[Neovim 0.10 and older](editors.md#neovim-010-and-older)**
- **[Zed](editors.md#zed)**
- **[Cursor](editors.md#cursor)**
- **[PHPStorm](editors.md#phpstorm)**
- **[Claude Code](editors.md#claude-code)**

The key setting in every editor is the **command**: set it to the full path returned by `which php-lsp` (e.g. `/usr/local/bin/php-lsp` or `~/.cargo/bin/php-lsp`), and associate it with the `php` file type.

## Step 4 — Open a PHP project

Open a folder that contains a `composer.json`. php-lsp detects the workspace root from `composer.json` or `.git`, builds a PSR-4 index in the background, and starts serving completions, diagnostics, and navigation immediately.

You will see a `$/progress` spinner in your editor's status bar while the initial index is being built. Features work before indexing completes — the index just improves cross-file results.

## Next steps

- **[Configuration](configuration.md)** — set `phpVersion`, suppress noisy diagnostics, exclude generated paths
- **[Features](features.md)** — full list of everything php-lsp supports
- **[Architecture](architecture.md)** — internals for contributors and advanced users
