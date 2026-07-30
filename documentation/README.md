# php-lsp docs site

Public documentation site at [jorgsowa.github.io/php-lsp](https://jorgsowa.github.io/php-lsp), built with [Astro Starlight](https://starlight.astro.build) and deployed to GitHub Pages by `.github/workflows/docs.yml` on every push to `main` that touches `documentation/`.

Page content lives directly under `src/content/docs/*.md` — edit those files and rerun `npm run dev` / `npm run build`. `configuration.md` is also read by the Rust crate's own test suite (`src/lang/config.rs`), which fails the build if a config option isn't documented there.

## Commands

| Command | Action |
|---|---|
| `npm install` | Install dependencies |
| `npm run dev` | Start the dev server at `localhost:4321` |
| `npm run build` | Build the static site to `./dist/` |
| `npm run preview` | Preview a production build locally |
