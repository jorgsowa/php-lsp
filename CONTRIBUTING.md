# Contributing

## Getting started

```bash
git clone https://github.com/jorgsowa/php-lsp
cd php-lsp
cargo build
cargo test
```

Requires Rust stable. No additional system dependencies.

## Submitting changes

1. Open an issue first for anything non-trivial — this avoids duplicate work.
2. Fork the repo, create a branch (`feat/my-feature` or `fix/issue-42`).
3. Write tests. Every new function or behaviour should have at least one unit test.
4. Run `cargo test` and `cargo clippy` — both must be clean before opening a PR.
5. Open a PR against `main` with a short description of what and why.

## Code style

- `cargo fmt` is enforced by a pre-push hook (run it yourself or it runs automatically).
- `cargo clippy -- -D warnings` must produce zero warnings.
- Keep functions small and focused. Prefer pure functions over methods with side effects where practical.
- New LSP features go in their own module (e.g. `src/my_feature.rs`) and are wired into `src/backend.rs`.

## Architecture

See [docs/architecture.md](docs/architecture.md) for a map of the codebase.

## Issues

Bug reports and feature requests are welcome via [GitHub Issues](https://github.com/jorgsowa/php-lsp/issues). Include a minimal PHP snippet that reproduces the problem when reporting bugs.
