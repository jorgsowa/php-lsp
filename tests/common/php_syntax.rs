//! PHP syntax validation for test fixtures using `php -l`.

use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

/// Validates that PHP code is syntactically correct using `php -l`.
/// Returns Ok(()) if valid, Err with the lint error if invalid.
/// Panics if `php` is not in PATH — it is a required test dependency.
pub fn validate(php_code: &str) -> Result<(), String> {
    let mut temp = NamedTempFile::new().map_err(|e| format!("failed to create temp file: {e}"))?;
    temp.write_all(php_code.as_bytes())
        .map_err(|e| format!("failed to write temp file: {e}"))?;

    let output = match Command::new("php").arg("-l").arg(temp.path()).output() {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            panic!("`php` is not in PATH — install PHP to run these tests")
        }
        Err(e) => return Err(format!("failed to run php -l: {e}")),
    };

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(if !stderr.is_empty() {
            stderr.to_string()
        } else {
            stdout.to_string()
        })
    }
}

/// Validates a fixture file and panics if syntax is invalid.
/// Use `allow_invalid_php()` before the fixture to suppress this check.
pub fn validate_fixture_file(path: &str, code: &str, allow_invalid: bool) {
    if allow_invalid {
        return;
    }

    if let Err(e) = validate(code) {
        panic!(
            "invalid PHP syntax in fixture file {path}:\n{e}\n\n\
             To allow intentional syntax errors, add `allow_invalid_php()` call before the fixture",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_correct_php() {
        let code = r#"<?php
class Foo {
    public function bar(): string {
        return "hello";
    }
}
"#;
        assert!(validate(code).is_ok());
    }

    #[test]
    fn rejects_invalid_php() {
        let code = r#"<?php
class Foo {
    public function bar() {
        return
    }
"#;
        let err = validate(code).expect_err("missing closing braces must fail php -l");
        assert!(
            err.contains("syntax error"),
            "expected a syntax error message, got: {err}"
        );
    }

    #[test]
    fn accepts_minimal_code() {
        assert!(validate("<?php").is_ok());
    }

    #[test]
    fn accepts_code_without_php_tag() {
        assert!(validate("class Foo {}").is_ok());
    }

    #[test]
    fn rejects_code_with_cursor_marker() {
        // $0 is invalid PHP syntax (fixture DSL marker)
        let code = r#"<?php
class Foo$0 {}"#;
        let err = validate(code).expect_err("$0 cursor marker must fail php -l");
        assert!(
            err.contains("syntax error"),
            "expected a syntax error message, got: {err}"
        );
    }

    #[test]
    fn validates_code_after_cursor_removal() {
        let code_with_marker = r#"<?php
class Foo$0 {}"#;
        let code_cleaned = code_with_marker.replace("$0", "");
        assert!(validate(&code_cleaned).is_ok());
    }

    #[test]
    fn accepts_annotation_comments() {
        let code = r#"<?php
foo();
// ^^^ error: not defined"#;
        assert!(validate(code).is_ok());
    }
}
