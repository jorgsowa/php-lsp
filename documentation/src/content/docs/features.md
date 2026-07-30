---
title: Features
description: The full list of diagnostics, completions, navigation, and refactoring support.
---

## Diagnostics

Core diagnostics (undefined symbols, type errors, arity checks, unused symbols, and the rest of
this list) are powered by [mir](https://github.com/jorgsowa/mir), a standalone static-analysis
engine developed alongside php-lsp — the LSP layer itself only translates mir's findings into
the protocol's `Diagnostic` shape, it does no type-checking of its own.

- Syntax errors reported in real time
- Undefined variables, undefined functions, argument-count mismatches
- Return-type literal mismatches and null-safety violations
- `@deprecated` call warnings at call sites
- Duplicate class/function/interface/trait/enum declarations
- Workspace-wide diagnostics for all indexed files (not just open ones)
- `workspace/diagnostic/refresh` support
- Optional PHPStan / PHPCS integration — run as external processes on save and merged
  alongside the built-in diagnostics, attributed with `source: "phpstan"` / `"phpcs"`. Off by
  default; see [configuration.md](/configuration/#externaltools-object).

## Hover

- PHP signature for functions, methods, classes, interfaces, traits, and enums
- Variable type hover — shows inferred class
- Property type hover — `(property) ClassName::$prop: TypeHint`
- Built-in class hover (PDO, DateTime, Exception, …) from bundled stubs
- `use` alias hover — shows fully-qualified class name
- `@param` / `@return` / `@throws` / `@deprecated` / `@see` / `@link` / `@template` / `@mixin` docblock annotations
- Deprecated symbols show a `> Deprecated` banner
- Built-in PHP functions link to [php.net](https://www.php.net)

## Completion

- Keywords, ~200 built-in PHP functions, superglobals, classes, methods, properties, constants, enum cases
- `->` / `?->` completions scoped to the inferred receiver type
- `ClassName::` / `self::` / `static::` / `parent::` static members and constants
- Built-in class member completions (Exception, DateTime, PDO, SPL, iterators, …)
- Method-chain type inference; `self`/`static` fluent return types; union types
- `@param` docblock inference; `instanceof` type narrowing
- Constructor-chain `(new DateTime())->`; bound-closure `$this`
- `closure use` variable type propagation; `array_map`/`array_filter` propagation
- Named-argument (`param:`) completions; attribute argument completions
- `use` FQN completions; sub-namespace `\` completions; `#[` attribute completions
- `match` arm enum-case completions; `readonly` property detail
- Magic method and magic constant completions
- Camel/underscore-case fuzzy matching (`GRF` → `getRecentFiles`)
- Snippet completions with cursor inside parentheses
- Auto `use` insertion on accepting a class from another namespace
- `completionItem/resolve` — documentation fetched lazily

## Navigation

- **Go-to-definition** — across files and into Composer vendor via PSR-4; variable → first assignment
- **Go-to-declaration** — abstract/interface method declaration
- **Go-to-type-definition** — jump to the class of a variable's type
- **Go-to-implementation** — all classes implementing an interface or extending a class
- **Find references** — all usages including `use` imports, workspace-wide
- **Rename** — functions, methods, classes, variables/parameters (scope-local), properties (cross-file)
- **Call hierarchy** — incoming callers / outgoing callees, cross-file
- **Type hierarchy** — supertypes and subtypes
- **Document symbols** — file outline
- **Workspace symbols** — fuzzy search with kind-filter prefix (`#class:`, `#fn:`, `#method:`, …)
- **Document highlight** — all occurrences of a symbol in the current file
- **Selection range** — smart expand/shrink (expression → statement → function → file)

## Code Actions

- **Add use import** — quick-fix for undefined class names
- **Add PHPDoc** — generates `/** */` stub for undocumented functions/methods
- **Implement missing methods** — stubs for abstract/interface methods
- **Generate constructor** — from declared properties
- **Generate getters/setters** — from declared properties
- **Extract variable** — wraps selected expression in `$extracted`
- **Extract method** — moves a multi-line selection into a new `private` method
- **Extract constant** — extracts a selected literal into a named `const`
- **Inline variable** — replaces all usages with the initializer and removes the assignment
- **Add return type** — inserts `: void` or `: mixed` after a function signature
- **Organize imports** — sorts `use` statements and removes unused ones
- `codeAction/resolve` — expensive edits computed lazily

## Editing Aids

- **Signature help** — parameter hints while typing; ~150 built-in function signatures bundled
- **Inlay hints** — parameter name labels; return-type labels; `inlayHint/resolve` tooltip
- **Semantic tokens** — rich highlighting with `declaration`/`static`/`abstract`/`readonly`/`deprecated` modifiers; `@deprecated` symbols rendered with strikethrough
- **On-type formatting** — auto-indent on Enter; `}` aligned to matching `{`
- **Formatting** — delegates to `php-cs-fixer` (PSR-12) or `phpcbf`; full-file and range; format-on-save via `willSaveWaitUntil`
- **Document links** — `include`/`require` paths are clickable
- **Linked editing** — typing replaces all occurrences of a symbol simultaneously

## Workspace

- **Multi-root workspace** — all `workspaceFolders` indexed; incremental updates on add/remove
- **PSR-4 resolution** — reads `composer.json` and `vendor/composer/installed.json`
- **PHPStorm metadata** — `.phpstorm.meta.php` factory-method return-type inference
- **File watching** — index updated on create/change/delete
- **File rename** — `workspace/willRenameFiles` updates all `use` imports across the workspace
- **File create stub** — `workspace/willCreateFiles` inserts `<?php declare(strict_types=1); namespace …; class ClassName {}` derived from the PSR-4 map
- **Live configuration** — `phpVersion` and `excludePaths` take effect without restarting
- **Workspace indexing** — background scan at startup, 50 000-file cap, LRU eviction at 10 000 indexed-only files, `$/progress` spinner
- **Async parsing** — edits debounced 100 ms, parsed off the main thread; stale results discarded
