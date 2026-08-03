//! Validation rule-name completion inside `->validate([...])`/
//! `Validator::make($data, [...])` calls and a `rules()` method's
//! `return [...]`.
//!
//! Gating on the enclosing call is AST-based, unlike every other
//! completion-prefix helper in this module (`middleware_string_prefix`,
//! `string_call::call_string_prefix`), which stay pure text scans because an
//! unterminated string breaks their surrounding expression's parse. Here the
//! *string itself* being typed can be unterminated without taking the
//! enclosing array/call down with it: the lexer swallows everything up to
//! the next quote (or EOF) into one big string token, but every enclosing
//! node (`MethodCall`, `Array`, the method body, the class) still gets built
//! from whatever parsed before that point — see the parser's per-construct
//! `expect(...)` recovery. So the AST walk below can reliably answer "is the
//! cursor inside a recognized rules array" even mid-keystroke; only the
//! *text* of the matched string span is read from source directly, since the
//! decoded `ExprKind::String` content itself may carry swallowed garbage.

use std::ops::ControlFlow;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{ArrayElement, ClassMemberKind, Expr, ExprKind, NamespaceBody, Span, Stmt, StmtKind};
use tower_lsp_server::ls_types::{CompletionItem, CompletionItemKind, Position};

use crate::document::ast::ParsedDoc;

/// Built-in Laravel validation rule names. Bare names only — parameterized
/// suffixes (`max:255`, `in:a,b,c`) aren't completed past the `:`, matching
/// the scope of this feature (see `ROADMAP.md` open work #2).
const RULE_NAMES: &[&str] = &[
    "accepted",
    "accepted_if",
    "active_url",
    "after",
    "after_or_equal",
    "alpha",
    "alpha_dash",
    "alpha_num",
    "array",
    "ascii",
    "bail",
    "before",
    "before_or_equal",
    "between",
    "boolean",
    "confirmed",
    "contains",
    "current_password",
    "date",
    "date_equals",
    "date_format",
    "decimal",
    "declined",
    "declined_if",
    "different",
    "digits",
    "digits_between",
    "dimensions",
    "distinct",
    "doesnt_start_with",
    "doesnt_end_with",
    "email",
    "ends_with",
    "enum",
    "exclude",
    "exclude_if",
    "exclude_unless",
    "exclude_with",
    "exclude_without",
    "exists",
    "extensions",
    "file",
    "filled",
    "gt",
    "gte",
    "hex_color",
    "image",
    "in",
    "in_array",
    "integer",
    "ip",
    "ipv4",
    "ipv6",
    "json",
    "lowercase",
    "lt",
    "lte",
    "mac_address",
    "max",
    "max_digits",
    "mimes",
    "mimetypes",
    "min",
    "min_digits",
    "missing",
    "missing_if",
    "missing_unless",
    "missing_with",
    "missing_with_all",
    "multiple_of",
    "not_in",
    "not_regex",
    "nullable",
    "numeric",
    "present",
    "present_if",
    "present_unless",
    "present_with",
    "present_with_all",
    "prohibited",
    "prohibited_if",
    "prohibited_unless",
    "prohibits",
    "regex",
    "required",
    "required_array_keys",
    "required_if",
    "required_if_accepted",
    "required_unless",
    "required_with",
    "required_with_all",
    "required_without",
    "required_without_all",
    "same",
    "size",
    "sometimes",
    "starts_with",
    "string",
    "timezone",
    "ulid",
    "unique",
    "uploaded",
    "url",
    "uuid",
];

/// Completion items for validation rule names starting with `prefix`.
pub(crate) fn validation_rule_completions(prefix: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = RULE_NAMES
        .iter()
        .filter(|name| name.starts_with(prefix))
        .map(|name| CompletionItem {
            label: (*name).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            insert_text: Some((*name).to_string()),
            ..Default::default()
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

fn is_ident(expr: &Expr<'_, '_>, name: &str) -> bool {
    matches!(&expr.kind, ExprKind::Identifier(n) if n.eq_ignore_ascii_case(name))
}

/// The span (quotes included) of the rule-name string element containing
/// `cursor`, whether it's the sole pipe-delimited string value of a field
/// (`'email' => 'required|em`) or one element of the array form
/// (`'email' => ['required', 'em`). `elements` is the rules array itself
/// (field name => rule spec) on the first call — `require_key` rejects a
/// still-being-typed field name that hasn't reached `=>` yet, which the
/// parser otherwise treats as a bare (unkeyed) positional element
/// indistinguishable from a real one. The recursive call into a nested
/// array-form rule list passes `require_key: false`, since that list's
/// entries are genuinely unkeyed.
fn rule_value_span_at(elements: &[ArrayElement<'_, '_>], cursor: u32, require_key: bool) -> Option<Span> {
    for el in elements {
        if require_key && el.key.is_none() {
            continue;
        }
        if el.value.span.start <= cursor && cursor <= el.value.span.end {
            return match &el.value.kind {
                ExprKind::String(_) => Some(el.value.span),
                ExprKind::Array(nested) => rule_value_span_at(nested, cursor, false),
                _ => None,
            };
        }
    }
    None
}

/// Finds the rules array (mapping field name to rule spec) whose span
/// contains `cursor` — either the first argument to a `->validate([...])`
/// call, or the second argument to `Validator::make($data, [...])` —
/// anywhere in the document, then resolves down to the specific rule-string
/// element under the cursor.
struct RulesCallVisitor {
    cursor: u32,
    found: Option<Span>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for RulesCallVisitor {
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if self.found.is_some() {
            return ControlFlow::Break(());
        }
        let array_arg = match &expr.kind {
            ExprKind::MethodCall(mc) if is_ident(mc.method, "validate") => mc.args.first(),
            ExprKind::StaticMethodCall(s) if is_ident(s.method, "validate") => s.args.first(),
            ExprKind::StaticMethodCall(s)
                if is_ident(s.method, "make") && is_ident(s.class, "Validator") =>
            {
                s.args.get(1)
            }
            _ => None,
        };
        if let Some(arg) = array_arg
            && let Some(arg_value) = &arg.value
            && let ExprKind::Array(elements) = &arg_value.kind
            && arg_value.span.start <= self.cursor
            && self.cursor <= arg_value.span.end
            && let Some(span) = rule_value_span_at(elements, self.cursor, true)
        {
            self.found = Some(span);
            return ControlFlow::Break(());
        }
        walk_expr(self, expr)
    }
}

/// Finds a `return [...]` (including inside nested blocks) whose array span
/// contains `cursor`, resolved down to the specific rule-string element.
fn find_return_array_span<'a>(stmts: &[Stmt<'a, 'a>], cursor: u32) -> Option<Span> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Return(Some(expr)) => {
                if let ExprKind::Array(elements) = &expr.kind
                    && expr.span.start <= cursor
                    && cursor <= expr.span.end
                {
                    return rule_value_span_at(elements, cursor, true);
                }
            }
            StmtKind::Block(inner) => {
                if let Some(span) = find_return_array_span(&inner.stmts, cursor) {
                    return Some(span);
                }
            }
            _ => {}
        }
    }
    None
}

/// Walks every class (recursing into braced namespaces) looking for a method
/// literally named `rules` whose body contains `cursor`, then looks for its
/// `return [...]` array — the third recognized rules-array shape, alongside
/// `validate()`/`Validator::make()` handled by [`RulesCallVisitor`].
fn find_rules_method_return_span<'a>(stmts: &[Stmt<'a, 'a>], cursor: u32) -> Option<Span> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                if !(stmt.span.start <= cursor && cursor <= stmt.span.end) {
                    continue;
                }
                for member in c.body.members.iter() {
                    let ClassMemberKind::Method(m) = &member.kind else {
                        continue;
                    };
                    if m.name != "rules" {
                        continue;
                    }
                    let Some(body) = &m.body else { continue };
                    if !(body.span.start <= cursor && cursor <= body.span.end) {
                        continue;
                    }
                    if let Some(span) = find_return_array_span(&body.stmts, cursor) {
                        return Some(span);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(span) = find_rules_method_return_span(&inner.stmts, cursor)
                {
                    return Some(span);
                }
            }
            _ => {}
        }
    }
    None
}

/// Typed prefix (from the last `|`, or the opening quote if there is none,
/// up to the cursor), when the cursor sits inside a rule-name string that's
/// part of a recognized rules array. `None` when the cursor isn't in any of
/// the three recognized shapes at all — the caller should fall through to
/// normal completion.
pub(crate) fn validation_rule_prefix(doc: &ParsedDoc, source: &str, position: Position) -> Option<String> {
    let sv = doc.view();
    let cursor = sv.byte_of_position(position);

    let mut visitor = RulesCallVisitor { cursor, found: None };
    for stmt in doc.program().stmts.iter() {
        if matches!(visitor.visit_stmt(stmt), ControlFlow::Break(())) {
            break;
        }
    }
    let span = visitor
        .found
        .or_else(|| find_rules_method_return_span(&doc.program().stmts, cursor))?;

    let start = span.start as usize + 1; // skip the opening quote
    let end = cursor as usize;
    if end < start || end > source.len() {
        return None;
    }
    let raw = &source[start..end];
    Some(raw.rsplit('|').next().unwrap_or(raw).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    #[test]
    fn validation_rule_completions_filters_by_prefix_and_sorts() {
        let items = validation_rule_completions("requ");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "required",
                "required_array_keys",
                "required_if",
                "required_if_accepted",
                "required_unless",
                "required_with",
                "required_with_all",
                "required_without",
                "required_without_all",
            ]
        );
    }

    #[test]
    fn validation_rule_completions_empty_for_unknown_prefix() {
        assert!(validation_rule_completions("zzz").is_empty());
    }

    #[test]
    fn prefix_matches_pipe_form_first_rule_in_request_validate() {
        let src = "<?php\nclass C {\n    public function store($request) {\n        $request->validate(['email' => 'requ\n";
        let doc = parse(src);
        let pos = Position {
            line: 3,
            character: 44,
        };
        assert_eq!(validation_rule_prefix(&doc, src, pos).as_deref(), Some("requ"));
    }

    #[test]
    fn prefix_matches_pipe_form_second_rule() {
        let src = "<?php\nclass C {\n    public function store($request) {\n        $request->validate(['email' => 'required|em\n";
        let doc = parse(src);
        let pos = Position {
            line: 3,
            character: 51,
        };
        assert_eq!(validation_rule_prefix(&doc, src, pos).as_deref(), Some("em"));
    }

    #[test]
    fn prefix_matches_array_form_element() {
        let src = "<?php\nclass C {\n    public function store($request) {\n        $request->validate(['email' => ['required', 'em\n";
        let doc = parse(src);
        let pos = Position {
            line: 3,
            character: 55,
        };
        assert_eq!(validation_rule_prefix(&doc, src, pos).as_deref(), Some("em"));
    }

    #[test]
    fn prefix_matches_validator_make_second_argument() {
        let src = "<?php\nValidator::make($data, ['email' => 'requ\n";
        let doc = parse(src);
        let pos = Position {
            line: 1,
            character: 40,
        };
        assert_eq!(validation_rule_prefix(&doc, src, pos).as_deref(), Some("requ"));
    }

    #[test]
    fn prefix_matches_rules_method_return_array() {
        let src = "<?php\nclass StoreUserRequest {\n    public function rules() {\n        return ['email' => 'requ\n";
        let doc = parse(src);
        let pos = Position {
            line: 3,
            character: 32,
        };
        assert_eq!(validation_rule_prefix(&doc, src, pos).as_deref(), Some("requ"));
    }

    #[test]
    fn prefix_none_for_field_name_position() {
        // Cursor is inside the *key* string, not a rule value.
        let src = "<?php\n$request->validate(['ema";
        let doc = parse(src);
        let pos = Position {
            line: 1,
            character: 24,
        };
        assert!(validation_rule_prefix(&doc, src, pos).is_none());
    }

    #[test]
    fn prefix_none_for_unrelated_array() {
        let src = "<?php\n$x = ['email' => 'requ";
        let doc = parse(src);
        let pos = Position {
            line: 1,
            character: 22,
        };
        assert!(validation_rule_prefix(&doc, src, pos).is_none());
    }

    #[test]
    fn prefix_none_for_validate_call_on_unrelated_class() {
        let src = "<?php\nFoo::make($data, ['email' => 'requ";
        let doc = parse(src);
        let pos = Position {
            line: 1,
            character: 34,
        };
        assert!(validation_rule_prefix(&doc, src, pos).is_none());
    }
}
