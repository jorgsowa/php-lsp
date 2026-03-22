use php_parser_rs::parser::ast::{classes::ClassMember, namespaces::NamespaceStatement, Statement};
use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation,
};

use crate::hover::format_params_str;

/// Returns signature help for the function call the cursor is inside of.
pub fn signature_help(source: &str, ast: &[Statement], position: Position) -> Option<SignatureHelp> {
    let (func_name, active_param) = call_context(source, position)?;
    let sig_text = find_signature(ast, &func_name)?;

    let label = format!("{}({})", func_name, sig_text);
    let params: Vec<ParameterInformation> = sig_text
        .split(',')
        .map(|p| ParameterInformation {
            label: ParameterLabel::Simple(p.trim().to_string()),
            documentation: None,
        })
        .filter(|p| {
            if let ParameterLabel::Simple(s) = &p.label {
                !s.is_empty()
            } else {
                true
            }
        })
        .collect();

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: if params.is_empty() { None } else { Some(params) },
            active_parameter: Some(active_param as u32),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param as u32),
    })
}

/// Scan backward from the cursor to find the enclosing function call name
/// and the index of the current parameter (0-based comma count).
fn call_context(source: &str, position: Position) -> Option<(String, usize)> {
    // Collect all source chars up to cursor
    let mut chars_before = String::new();
    for (i, line) in source.lines().enumerate() {
        if i < position.line as usize {
            chars_before.push_str(line);
            chars_before.push('\n');
        } else if i == position.line as usize {
            let col = position.character as usize;
            let line_chars: Vec<char> = line.chars().collect();
            // Convert UTF-16 offset to char offset
            let mut utf16 = 0usize;
            let mut char_col = 0usize;
            for ch in &line_chars {
                if utf16 >= col { break; }
                utf16 += ch.len_utf16();
                char_col += 1;
            }
            chars_before.extend(line_chars.iter().take(char_col));
            break;
        }
    }

    // Walk backward tracking paren depth and comma count
    let text: Vec<char> = chars_before.chars().collect();
    let mut depth = 0i32;
    let mut commas = 0usize;
    let mut i = text.len();

    while i > 0 {
        i -= 1;
        match text[i] {
            ')' | ']' => depth += 1,
            '(' | '[' if depth > 0 => depth -= 1,
            '(' if depth == 0 => {
                // Found the opening paren of the call — extract the function name
                let name = extract_name_before(&text, i);
                if !name.is_empty() {
                    return Some((name, commas));
                }
                return None;
            }
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

fn extract_name_before(text: &[char], paren_pos: usize) -> String {
    if paren_pos == 0 { return String::new(); }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '\\';
    let mut end = paren_pos;
    // Skip whitespace
    while end > 0 && text[end - 1] == ' ' { end -= 1; }
    let mut start = end;
    while start > 0 && is_ident(text[start - 1]) { start -= 1; }
    if start == end { return String::new(); }
    text[start..end].iter().collect()
}

fn find_signature(ast: &[Statement], word: &str) -> Option<String> {
    for stmt in ast {
        match stmt {
            Statement::Function(f) if f.name.value.to_string() == word => {
                return Some(format_params_str(&f.parameters));
            }
            Statement::Class(c) => {
                for member in &c.body.members {
                    match member {
                        ClassMember::ConcreteMethod(m) if m.name.value.to_string() == word => {
                            return Some(format_params_str(&m.parameters));
                        }
                        _ => {}
                    }
                }
            }
            Statement::Namespace(ns) => {
                let stmts = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                if let Some(s) = find_signature(stmts, word) {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ast(source: &str) -> Vec<Statement> {
        match php_parser_rs::parser::parse(source) {
            Ok(ast) => ast,
            Err(stack) => stack.partial,
        }
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn returns_signature_for_known_function() {
        let src = "<?php\nfunction greet(string $name, int $times): void {}\ngreet(";
        let ast = parse_ast(src);
        let result = signature_help(src, &ast, pos(2, 6));
        assert!(result.is_some(), "expected signature help");
        let sh = result.unwrap();
        assert_eq!(sh.signatures[0].label, "greet(string $name, int $times)");
    }

    #[test]
    fn active_parameter_tracks_comma() {
        let src = "<?php\nfunction add(int $a, int $b): int {}\nadd($x, ";
        let ast = parse_ast(src);
        let result = signature_help(src, &ast, pos(2, 8));
        assert!(result.is_some());
        let sh = result.unwrap();
        assert_eq!(sh.active_parameter, Some(1), "second param should be active");
    }

    #[test]
    fn returns_none_outside_call() {
        let src = "<?php\nfunction greet() {}\n$x = 1;";
        let ast = parse_ast(src);
        let result = signature_help(src, &ast, pos(2, 4));
        assert!(result.is_none());
    }

    #[test]
    fn returns_none_for_unknown_function() {
        let src = "<?php\nunknown(";
        let ast = parse_ast(src);
        let result = signature_help(src, &ast, pos(1, 8));
        assert!(result.is_none(), "unknown function should yield no signature");
    }
}
