# mir-php

A PHP static analysis engine written in Rust.

`mir-php` performs type inference and semantic diagnostics on PHP source code.
It operates directly on [`php_ast`](https://crates.io/crates/php-ast) AST slices
with **no dependency on any LSP framework**, making it usable as a standalone
linter, a CI tool, or an embedded library.

## Features

- **Type inference** — infers variable types from assignments (`$x = new Foo()`,
  `$x = 42`, typed parameters, etc.) and exposes them via `TypeEnv`
- **Undefined symbol detection** — flags calls to undefined functions and classes
- **Arity checking** — warns when too few or too many arguments are passed
- **Undefined variable detection** — finds uses of variables never assigned in
  the current function scope
- **Return-type checking** — warns when a literal return value is incompatible
  with the declared return type (e.g. `return "hello"` in an `int` function)
- **Null-safety** — warns when a method is called directly on `null`
- **Built-in stubs** — ~200 PHP core functions and ~60 built-in classes/interfaces
  are pre-loaded so standard-library calls are never flagged as undefined

## Library usage

```rust
use bumpalo::Bump;

let arena = Bump::new();
let source = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1);";
let program = php_rs_parser::parse(&arena, source).program;

// Semantic diagnostics
let diags = mir_php::analyze(source, &program.stmts, &[(source, &program.stmts)]);
for d in &diags {
    println!("{}:{} — {}", d.start_line + 1, d.start_char + 1, d.message);
}

// Type inference
let env = mir_php::infer(&program.stmts);
if let Some(cls) = env.class_name("$obj") {
    println!("$obj is a {cls}");
}
```

## CLI usage

```bash
cargo install mir-php

# Human-readable output
mir-php src/Foo.php src/Bar.php

# JSON output (suitable for editor integrations)
mir-php --json src/**/*.php
```

Exit code is `0` when no issues are found, `1` when there are warnings or errors,
and `2` for usage errors (bad arguments, unreadable files).

## Integration with php-lsp

`mir-php` is the analysis backend for [`php-lsp`](https://crates.io/crates/php-lsp).
`php-lsp` parses documents into `ParsedDoc` values (owning their bumpalo arenas),
then passes the raw AST slices into `mir_php::analyze()` and converts the results
into LSP `Diagnostic` objects.

## License

MIT
