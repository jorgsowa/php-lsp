# tests/fixtures

Vendored real-world PHP projects used by E2E tests. Each fixture is copied
into a fresh `TempDir` per test via `TestServer::with_fixture(name)`, so
tests can mutate files without contaminating siblings.

## symfony-demo

Upstream: https://github.com/symfony/demo
Pinned commit: `c5d841b215b4bcfbf7a62e8183e159b96874f75b` (tag `v2.8.0`)
License: MIT (see `symfony-demo/LICENSE`)

**How it was acquired:**

```bash
git clone --depth 1 --branch v2.8.0 https://github.com/symfony/demo.git symfony-demo
cd symfony-demo
composer install --no-dev --no-scripts --no-interaction
rm -rf .git
# Trim bloat that exercises nothing LSP cares about:
rm -rf vendor/twbs                         # Bootstrap CSS — 20 MB, zero PHP
rm -rf vendor/symfony/intl/Resources/data  # ICU locale tables — 18 MB, 1,295 trivial return arrays
# Remove the fixture's own .gitignore files so the outer repo can track
# vendor/, var/, public/assets/, etc.
find . -name .gitignore -delete
```

**Why `--no-dev`:** prod deps only keep `vendor/` at ~74 MB. Adding dev deps
(phpunit, phpstan, etc.) roughly doubles it and doesn't exercise any LSP
feature that prod deps don't already.

**Why the trims:** the vanilla install is 77 MB. `twbs/bootstrap` is purely
front-end assets; `symfony/intl/Resources/data` is 1,295 locale lookup
tables that the parser chews through without exercising any feature a
single hand-rolled array doesn't already hit. Trimming them drops the
fixture to ~40 MB (~5,200 PHP files, still realistic PSR-4 / attributes /
Doctrine / Twig coverage).

**What it exercises:** ~6500 PHP files across `vendor/`, PSR-4 autoload,
Symfony attributes (`#[Route]`, `#[AsCommand]`), Doctrine entities, Twig
extensions, abstract controllers, traits, interfaces — a realistic workload
for cross-file goto-definition, references, and workspace symbol search.

To upgrade the pin: re-run the acquisition against a newer tag, update the
commit SHA above, and run `cargo test --test 'e2e_symfony*'`.
