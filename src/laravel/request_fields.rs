//! Heuristic field-name harvesting for `$request`/`$this`-style Laravel
//! request accessors (`->input()`, `->get()`, `->post()`, `->query()`), used
//! by request-field completion and the "Generate validation rules" quickfix
//! (`src/actions/generate_validation_rules_action.rs`).
//!
//! No DB schema access, no type inference — field names come purely from
//! other such calls already present in the surrounding code. The receiver
//! is recognized by naming convention only (a variable literally named
//! `$request`, case-insensitive, or `$this` inside a method whose class has
//! a `rules()` method) rather than by resolving its actual type: precise
//! type resolution would need `CompletionCtx` threaded through with
//! workspace-index access this module doesn't have, and the naming
//! convention this heuristic relies on (`$request` is near-universal in
//! Laravel controller/FormRequest code) already covers the common case.

use std::ops::ControlFlow;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{Expr, ExprKind, Stmt};
use tower_lsp_server::ls_types::Position;

use crate::text::utf16_offset_to_byte;

/// Bare method names recognized as request field-accessors.
pub(crate) const REQUEST_FIELD_METHODS: &[&str] = &["input", "get", "post", "query"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Receiver {
    /// A variable named `request` (any case) — the name itself, as written.
    Variable(String),
    ThisKeyword,
}

/// Typed prefix (from the opening quote up to the cursor) and the matched
/// receiver, when the cursor sits inside a string literal — closed or not —
/// immediately following `->method(` for one of `methods`, where the
/// receiver is a variable named `request` (case-insensitive). Used for
/// completion, where the closing quote may not exist yet.
pub(crate) fn method_call_string_prefix(
    source: &str,
    position: Position,
) -> Option<(Receiver, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let line = *lines.get(position.line as usize)?;
    let byte_col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..byte_col];
    let quote_idx = before.rfind(['\'', '"'])?;
    let before_quote = before[..quote_idx].trim_end();
    let before_paren = before_quote.strip_suffix('(')?.trim_end();

    let method_start = REQUEST_FIELD_METHODS.iter().find_map(|m| {
        let m_len = m.len();
        if before_paren.len() >= m_len
            && before_paren[before_paren.len() - m_len..].eq_ignore_ascii_case(m)
            && word_boundary_before(before_paren, before_paren.len() - m_len)
        {
            Some(before_paren.len() - m_len)
        } else {
            None
        }
    })?;

    // `before_method` is a prefix of `line` starting at byte 0, so its own
    // length is exactly the byte column within `line` where it ends.
    let before_method = before_paren[..method_start].trim_end();
    let arrow_len = if before_method.ends_with("?->") {
        3
    } else if before_method.ends_with("->") {
        2
    } else {
        return None;
    };
    let receiver_text = &before_method[..before_method.len() - arrow_len];

    let var_name: String = receiver_text
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '$')
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    let bare = var_name.trim_start_matches('$');
    if !bare.eq_ignore_ascii_case("request") {
        return None;
    }

    Some((
        Receiver::Variable(bare.to_string()),
        before[quote_idx + 1..].to_string(),
    ))
}

fn word_boundary_before(s: &str, idx: usize) -> bool {
    idx == 0 || !matches!(s.as_bytes()[idx - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
}

/// Field names from `receiver->input('x')`/`get`/`post`/`query` calls
/// anywhere in `stmts`, deduped, first-seen order.
pub(crate) fn harvest_fields(stmts: &[Stmt<'_, '_>], receiver: &Receiver) -> Vec<String> {
    struct Collector<'r> {
        receiver: &'r Receiver,
        out: Vec<String>,
    }
    impl<'arena, 'src> Visitor<'arena, 'src> for Collector<'_> {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            if let ExprKind::MethodCall(mc) = &expr.kind
                && let ExprKind::Identifier(method) = &mc.method.kind
                && REQUEST_FIELD_METHODS
                    .iter()
                    .any(|m| method.eq_ignore_ascii_case(m))
                && receiver_matches(mc.object, self.receiver)
                && let Some(arg) = mc.args.first()
                && arg.name.is_none()
                && let ExprKind::String(s) = &arg.value.kind
                && !self.out.iter().any(|f| f == *s)
            {
                self.out.push((*s).to_string());
            }
            walk_expr(self, expr)
        }
    }
    let mut collector = Collector {
        receiver,
        out: Vec::new(),
    };
    for stmt in stmts {
        let _ = collector.visit_stmt(stmt);
    }
    collector.out
}

fn receiver_matches(object: &Expr<'_, '_>, receiver: &Receiver) -> bool {
    match (&object.kind, receiver) {
        (ExprKind::Variable(v), Receiver::Variable(name)) => v.eq_ignore_ascii_case(name),
        (ExprKind::Variable(v), Receiver::ThisKeyword) => v.as_str() == "this",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::ast::ParsedDoc;

    fn parse(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    #[test]
    fn method_call_string_prefix_matches_request_input() {
        let src = "<?php\n$request->input('em";
        let pos = Position {
            line: 1,
            character: 19,
        };
        let (receiver, prefix) = method_call_string_prefix(src, pos).unwrap();
        assert_eq!(receiver, Receiver::Variable("request".to_string()));
        assert_eq!(prefix, "em");
    }

    #[test]
    fn method_call_string_prefix_rejects_unrelated_variable() {
        let src = "<?php\n$foo->input('em";
        let pos = Position {
            line: 1,
            character: 15,
        };
        assert!(method_call_string_prefix(src, pos).is_none());
    }

    #[test]
    fn method_call_string_prefix_rejects_unrelated_method() {
        let src = "<?php\n$request->validate('em";
        let pos = Position {
            line: 1,
            character: 22,
        };
        assert!(method_call_string_prefix(src, pos).is_none());
    }

    #[test]
    fn harvest_fields_collects_distinct_calls_on_matching_variable() {
        let doc = parse(
            "<?php\nfunction f() {\n    $request->input('email');\n    $request->get('name');\n    $request->input('email');\n    $other->input('ignored');\n}\n",
        );
        let fields = harvest_fields(
            &doc.program().stmts,
            &Receiver::Variable("request".to_string()),
        );
        assert_eq!(fields, vec!["email".to_string(), "name".to_string()]);
    }

    #[test]
    fn harvest_fields_matches_this_keyword() {
        let doc = parse(
            "<?php\nclass R {\n    public function prep() {\n        $this->input('email');\n    }\n}\n",
        );
        let fields = harvest_fields(&doc.program().stmts, &Receiver::ThisKeyword);
        assert_eq!(fields, vec!["email".to_string()]);
    }
}
