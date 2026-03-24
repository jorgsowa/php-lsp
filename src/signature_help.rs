use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation,
};

use crate::ast::ParsedDoc;
use crate::hover::format_params_str;

/// Returns signature help for the function call the cursor is inside of.
pub fn signature_help(source: &str, doc: &ParsedDoc, position: Position) -> Option<SignatureHelp> {
    let (func_name, active_param) = call_context(source, position)?;
    let sig_text = find_signature(&doc.program().stmts, &func_name)?;

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
    let mut chars_before = String::new();
    for (i, line) in source.lines().enumerate() {
        if i < position.line as usize {
            chars_before.push_str(line);
            chars_before.push('\n');
        } else if i == position.line as usize {
            let col = position.character as usize;
            let line_chars: Vec<char> = line.chars().collect();
            let mut utf16 = 0usize;
            let mut char_col = 0usize;
            for ch in &line_chars {
                if utf16 >= col {
                    break;
                }
                utf16 += ch.len_utf16();
                char_col += 1;
            }
            chars_before.extend(line_chars.iter().take(char_col));
            break;
        }
    }

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
    if paren_pos == 0 {
        return String::new();
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '\\';
    let mut end = paren_pos;
    while end > 0 && text[end - 1] == ' ' {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && is_ident(text[start - 1]) {
        start -= 1;
    }
    if start == end {
        return String::new();
    }
    text[start..end].iter().collect()
}

fn find_signature(stmts: &[Stmt<'_, '_>], word: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                return Some(format_params_str(&f.params));
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == word {
                            return Some(format_params_str(&m.params));
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(s) = find_signature(inner, word) {
                        return Some(s);
                    }
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

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn returns_signature_for_known_function() {
        let src = "<?php\nfunction greet(string $name, int $times): void {}\ngreet(";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(2, 6));
        assert!(result.is_some(), "expected signature help");
        let sh = result.unwrap();
        assert_eq!(sh.signatures[0].label, "greet(string $name, int $times)");
    }

    #[test]
    fn active_parameter_tracks_comma() {
        let src = "<?php\nfunction add(int $a, int $b): int {}\nadd($x, ";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(2, 8));
        assert!(result.is_some());
        let sh = result.unwrap();
        assert_eq!(sh.active_parameter, Some(1), "second param should be active");
    }

    #[test]
    fn returns_none_outside_call() {
        let src = "<?php\nfunction greet() {}\n$x = 1;";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(2, 4));
        assert!(result.is_none());
    }

    #[test]
    fn returns_none_for_unknown_function() {
        let src = "<?php\nunknown(";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(1, 8));
        assert!(result.is_none(), "unknown function should yield no signature");
    }
}
