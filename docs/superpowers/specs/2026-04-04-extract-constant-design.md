# Extract Constant Code Action — Design Spec

**Issue:** #22  
**Date:** 2026-04-04

## Summary

Add a "Extract constant" refactoring code action. When a user selects a literal value (string, int, float), the action extracts it into a named PHP constant and replaces the selection with a reference to that constant.

## Approach

Text-only (no AST), consistent with `extract_variable`. Eager action (no `code_action_resolve` needed).

## Literal Detection

Selected text must match one of:
- String literal: starts and ends with `"` or `'`
- Integer: digits only (`^\d+$`)
- Float: digits with decimal point (`^\d+\.\d*$`)

Returns empty vec for any other selection.

## Constant Name Derivation

- **String:** strip quotes → replace non-alphanumeric with `_` → uppercase → strip leading digits → deduplicate underscores → trim → fallback to `EXTRACTED_CONSTANT` if empty
- **Integer/Float:** `CONSTANT_` + value with `.` replaced by `_`

## Scope Detection

Scan source lines backwards from the selection start:
- Match `class\s+\w+` → class scope; scan forward to find `{` → insert after it
- No match → file scope; insert after `<?php` / namespace / use block

## Edits

Two `TextEdit`s in one `WorkspaceEdit`:
1. Insert constant declaration at insertion point:
   - Class scope: `    private const NAME = value;\n`
   - File scope: `const NAME = value;\n`
2. Replace selected range with:
   - Class scope: `self::NAME`
   - File scope: `NAME`

## New File

`src/extract_constant_action.rs` — one public function:
```rust
pub fn extract_constant_actions(source: &str, range: Range, uri: &Url) -> Vec<CodeActionOrCommand>
```

## Registration

In `backend.rs` `code_action()` handler, alongside `extract_variable_actions` in the eager actions block.

## Tests

- Class scope: string literal inside class → inserts `private const`, replaces with `self::NAME`
- File scope: integer literal outside class → inserts `const`, replaces with `NAME`
- Non-literal selection → returns empty vec
