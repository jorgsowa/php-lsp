/// Code action: "Extract constant" — extracts a selected literal into a named PHP constant.
use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::util::selected_text_range;

/// When the selection is a string, integer, or float literal, offer to extract
/// it into a named constant.
///
/// - Inside a `class` or `trait`: inserts `private const NAME = value;` and
///   replaces the selection with `self::NAME`.
/// - Inside an `interface`: inserts `const NAME = value;` (interface constants
///   are implicitly public; `private` is invalid there).
/// - At file scope: inserts `const NAME = value;` and replaces with `NAME`.
///
/// The constant name is derived from the literal value (SCREAMING_SNAKE_CASE
/// for strings, `CONSTANT_<value>` for numbers). Use the LSP rename action to
/// pick a more meaningful name.
pub fn extract_constant_actions(source: &str, range: Range, uri: &Url) -> Vec<CodeActionOrCommand> {
    if range.start == range.end {
        return vec![];
    }

    let selected = selected_text_range(source, range);
    let trimmed = selected.trim();
    if trimmed.is_empty() || !is_literal(trimmed) {
        return vec![];
    }

    let const_name = derive_const_name(trimmed);
    let lines: Vec<&str> = source.lines().collect();
    let sel_line = range.start.line as usize;

    match find_class_scope(&lines, sel_line) {
        Some((insert_line, kind)) => {
            let insert_pos = Position {
                line: insert_line as u32 + 1,
                character: 0,
            };
            let decl = match kind {
                ContainerKind::Interface => format!("    const {const_name} = {trimmed};\n"),
                ContainerKind::ClassOrTrait => {
                    format!("    private const {const_name} = {trimmed};\n")
                }
            };
            let reference = format!("self::{const_name}");
            build_action("Extract constant", decl, insert_pos, reference, range, uri)
        }
        None => {
            let insert_line = file_scope_insert_line(&lines);
            let insert_pos = Position {
                line: insert_line as u32,
                character: 0,
            };
            let decl = format!("const {const_name} = {trimmed};\n");
            build_action("Extract constant", decl, insert_pos, const_name, range, uri)
        }
    }
}

// ── Literal detection ─────────────────────────────────────────────────────────

fn is_literal(s: &str) -> bool {
    is_string_literal(s) || is_int_literal(s) || is_float_literal(s)
}

fn is_string_literal(s: &str) -> bool {
    (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
}

fn is_int_literal(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn is_float_literal(s: &str) -> bool {
    let mut dots = 0u32;
    !s.is_empty()
        && s.chars().all(|c| {
            if c == '.' {
                dots += 1;
                dots == 1
            } else {
                c.is_ascii_digit()
            }
        })
        && dots == 1
}

// ── Constant name derivation ──────────────────────────────────────────────────

fn derive_const_name(literal: &str) -> String {
    if is_string_literal(literal) {
        let inner = &literal[1..literal.len() - 1];
        derive_name_from_string(inner)
    } else {
        let sanitised = literal.replace('.', "_");
        format!("CONSTANT_{sanitised}")
    }
}

fn derive_name_from_string(s: &str) -> String {
    let raw: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .to_uppercase();

    // Collapse consecutive underscores, strip leading and trailing underscores.
    let mut name = String::new();
    let mut prev_under = true;
    for c in raw.chars() {
        if c == '_' {
            if !prev_under {
                name.push('_');
            }
            prev_under = true;
        } else {
            name.push(c);
            prev_under = false;
        }
    }
    let name = name.trim_end_matches('_').to_string();

    // PHP identifiers cannot start with a digit.
    let name = if name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("CONSTANT_{name}")
    } else {
        name
    };

    if name.is_empty() {
        "EXTRACTED_CONSTANT".to_string()
    } else {
        name
    }
}

// ── Scope detection ───────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum ContainerKind {
    ClassOrTrait,
    Interface,
}

/// Scan backwards from `sel_line` to find an enclosing class, interface, or
/// trait declaration.  Returns `(brace_line, kind)` where `brace_line` is the
/// 0-based index of the line containing the opening `{`.
///
/// The selection must be strictly inside the container body (between the
/// opening `{` and its matching `}`).
fn find_class_scope(lines: &[&str], sel_line: usize) -> Option<(usize, ContainerKind)> {
    for i in (0..=sel_line).rev() {
        let line = lines[i].trim();
        if let Some(kind) = container_kind(line) {
            // Find the opening brace.
            for (j, brace_line) in lines.iter().enumerate().skip(i) {
                if brace_line.contains('{') {
                    // Verify the selection falls inside the container body.
                    if find_matching_close(lines, j)
                        .is_some_and(|close| sel_line > j && sel_line < close)
                    {
                        return Some((j, kind));
                    }
                    break;
                }
            }
        }
    }
    None
}

/// Starting at `open_line` (which contains the opening `{`), scan forward and
/// return the 0-based line index of the matching closing `}`.
fn find_matching_close(lines: &[&str], open_line: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, line) in lines.iter().enumerate().skip(open_line) {
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Returns `Some(ContainerKind)` if `line` is a class/interface/trait declaration.
fn container_kind(line: &str) -> Option<ContainerKind> {
    // Strip PHP modifier keywords before the type keyword.
    let stripped = line
        .trim_start_matches("abstract ")
        .trim_start_matches("final ")
        .trim_start_matches("readonly ");
    if stripped.starts_with("class ")
        || stripped.starts_with("class{")
        || stripped.starts_with("trait ")
        || stripped.starts_with("trait{")
    {
        Some(ContainerKind::ClassOrTrait)
    } else if stripped.starts_with("interface ") || stripped.starts_with("interface{") {
        Some(ContainerKind::Interface)
    } else {
        None
    }
}

/// Find the first line after `<?php`, blank lines, `namespace`, and `use`
/// statements.  The new `const` declaration will be inserted before that line.
///
/// Scanning stops at the first non-preamble line to prevent the insertion point
/// from jumping past code that already exists in the file.
fn file_scope_insert_line(lines: &[&str]) -> usize {
    let mut last_preamble = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("<?php")
            || t.is_empty()
            || t.starts_with("namespace ")
            || t.starts_with("use ")
        {
            last_preamble = i + 1;
        } else {
            break;
        }
    }
    last_preamble
}

// ── Action builder ────────────────────────────────────────────────────────────

fn build_action(
    title: &str,
    decl: String,
    insert_pos: Position,
    reference: String,
    replace_range: Range,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![
            TextEdit {
                range: Range {
                    start: insert_pos,
                    end: insert_pos,
                },
                new_text: decl,
            },
            TextEdit {
                range: replace_range,
                new_text: reference,
            },
        ],
    );
    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_string(),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Position;

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
    }

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Range {
        Range {
            start: Position {
                line: sl,
                character: sc,
            },
            end: Position {
                line: el,
                character: ec,
            },
        }
    }

    fn get_edits(action: &CodeActionOrCommand) -> Vec<&TextEdit> {
        if let CodeActionOrCommand::CodeAction(a) = action {
            a.edit
                .as_ref()
                .unwrap()
                .changes
                .as_ref()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .iter()
                .collect()
        } else {
            vec![]
        }
    }

    #[test]
    fn non_literal_selection_returns_empty() {
        let src = "<?php\n$x = foo();";
        let r = range(1, 5, 1, 10);
        assert!(extract_constant_actions(src, r, &uri()).is_empty());
    }

    #[test]
    fn empty_selection_returns_empty() {
        let src = "<?php\n$x = 42;";
        let r = range(1, 5, 1, 5);
        assert!(extract_constant_actions(src, r, &uri()).is_empty());
    }

    #[test]
    fn string_literal_in_class_scope() {
        let src = "<?php\nclass Foo {\n    public function bar() {\n        $x = \"hello_world\";\n    }\n}";
        // Select `"hello_world"` on line 3, chars 13..26
        let r = range(3, 13, 3, 26);
        let actions = extract_constant_actions(src, r, &uri());
        assert!(!actions.is_empty(), "expected extract constant action");

        let edits = get_edits(&actions[0]);
        let decl = edits
            .iter()
            .find(|e| e.new_text.contains("private const"))
            .unwrap();
        assert!(
            decl.new_text.contains("HELLO_WORLD"),
            "derived name should be HELLO_WORLD"
        );
        assert!(
            decl.new_text.contains("\"hello_world\""),
            "should preserve literal value"
        );

        let replacement = edits
            .iter()
            .find(|e| e.new_text.contains("self::"))
            .unwrap();
        assert_eq!(replacement.new_text, "self::HELLO_WORLD");
    }

    #[test]
    fn integer_literal_at_file_scope() {
        let src = "<?php\n\n$timeout = 30;";
        // Select `30` on line 2, chars 11..13
        let r = range(2, 11, 2, 13);
        let actions = extract_constant_actions(src, r, &uri());
        assert!(!actions.is_empty(), "expected extract constant action");

        let edits = get_edits(&actions[0]);
        let decl = edits
            .iter()
            .find(|e| e.new_text.starts_with("const "))
            .unwrap();
        assert!(
            decl.new_text.contains("CONSTANT_30"),
            "should derive name CONSTANT_30"
        );

        let replacement = edits.iter().find(|e| e.new_text == "CONSTANT_30").unwrap();
        assert_eq!(replacement.new_text, "CONSTANT_30");
    }

    #[test]
    fn float_literal_at_file_scope() {
        let src = "<?php\n$ratio = 1.5;";
        // Select `1.5` on line 1, chars 9..12
        let r = range(1, 9, 1, 12);
        let actions = extract_constant_actions(src, r, &uri());
        assert!(!actions.is_empty());

        let edits = get_edits(&actions[0]);
        let decl = edits
            .iter()
            .find(|e| e.new_text.starts_with("const "))
            .unwrap();
        assert!(
            decl.new_text.contains("CONSTANT_1_5"),
            "dot replaced with underscore"
        );
    }

    #[test]
    fn literal_inside_interface_uses_unqualified_const() {
        // PHP interface constants cannot be private; the action should emit
        // `const` (no visibility modifier) when inside an interface body.
        // Using a default parameter value as the selected literal.
        let src = "<?php\ninterface Greeter {\n    public function greet(string $lang = \"en\"): string;\n}";
        // Select `"en"` on line 2, chars 41..45
        let r = range(2, 41, 2, 45);
        let actions = extract_constant_actions(src, r, &uri());
        assert!(!actions.is_empty(), "expected action inside interface");
        let edits = get_edits(&actions[0]);
        let decl = edits
            .iter()
            .find(|e| e.new_text.contains("const "))
            .unwrap();
        assert!(
            !decl.new_text.contains("private"),
            "interface const must not be private; got: {}",
            decl.new_text
        );
        let replacement = edits
            .iter()
            .find(|e| e.new_text.contains("self::"))
            .unwrap();
        assert!(replacement.new_text.starts_with("self::"));
    }

    #[test]
    fn literal_after_interface_falls_to_file_scope() {
        // A literal on a line that is OUTSIDE the interface body must be treated
        // as file scope, not interface scope.
        let src = "<?php\ninterface PaymentGateway {\n    public function charge(): void;\n}\n$fee = 250;";
        // Select `250` on line 4, chars 7..10
        let r = range(4, 7, 4, 10);
        let actions = extract_constant_actions(src, r, &uri());
        assert!(!actions.is_empty());
        let edits = get_edits(&actions[0]);
        let decl = edits
            .iter()
            .find(|e| e.new_text.starts_with("const "))
            .unwrap();
        assert!(
            !decl.new_text.contains("private"),
            "file-scope const must not be private"
        );
        // Replacement must not use self:: at file scope.
        let replacement = edits.iter().find(|e| e.new_text == "CONSTANT_250").unwrap();
        assert_eq!(replacement.new_text, "CONSTANT_250");
    }

    #[test]
    fn literal_inside_trait_uses_private_const() {
        let src = "<?php\ntrait Logging {\n    public function log(): void {\n        $level = \"info\";\n    }\n}";
        // Select `"info"` on line 3, chars 17..23
        let r = range(3, 17, 3, 23);
        let actions = extract_constant_actions(src, r, &uri());
        assert!(!actions.is_empty(), "expected action inside trait");
        let edits = get_edits(&actions[0]);
        let decl = edits
            .iter()
            .find(|e| e.new_text.contains("const "))
            .unwrap();
        assert!(
            decl.new_text.contains("private const"),
            "trait const should be private"
        );
    }

    #[test]
    fn file_scope_insert_does_not_jump_past_code() {
        // A `use` statement that appears AFTER the selection must not push the
        // insertion point below the selection.
        let src = "<?php\n$x = \"hello\";\nuse Foo\\Bar;";
        // Select `"hello"` on line 1, chars 5..12
        let r = range(1, 5, 1, 12);
        let actions = extract_constant_actions(src, r, &uri());
        assert!(!actions.is_empty());
        let edits = get_edits(&actions[0]);
        let decl = edits
            .iter()
            .find(|e| e.new_text.contains("const "))
            .unwrap();
        // The const declaration must be inserted BEFORE line 1 (i.e. at line 1,
        // pushing `$x = "hello"` down) — not after the `use Foo\Bar;` on line 2.
        assert!(
            decl.range.start.line <= 1,
            "const declaration must not be inserted after the selection"
        );
    }

    #[test]
    fn derive_name_from_url_string() {
        assert_eq!(
            derive_name_from_string("https://api.example.com"),
            "HTTPS_API_EXAMPLE_COM"
        );
    }

    #[test]
    fn derive_name_empty_string_fallback() {
        assert_eq!(derive_name_from_string("!!!"), "EXTRACTED_CONSTANT");
    }

    #[test]
    fn derive_name_leading_digit_prefixed() {
        assert_eq!(derive_name_from_string("42abc"), "CONSTANT_42ABC");
    }
}
