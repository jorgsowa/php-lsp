use std::collections::HashMap;

use php_parser_rs::lexer::token::Span as TokenSpan;
use php_parser_rs::parser::ast::{
    arguments::Argument,
    classes::ClassMember,
    identifiers::Identifier as AstIdentifier,
    namespaces::NamespaceStatement,
    Expression, Statement,
};
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range};

/// Returns parameter-name inlay hints AND return-type hints for all
/// function/method declarations and calls in `ast`.
pub fn inlay_hints(source: &str, ast: &[Statement], range: Range) -> Vec<InlayHint> {
    let defs = collect_defs(ast);
    let mut hints = Vec::new();
    // Parameter-name hints at call sites
    hints_in_stmts(source, ast, &defs, range, &mut hints);
    // Return-type hints at declarations without explicit return type
    return_type_hints(source, ast, range, &mut hints);
    hints
}

// === Definition collection: function/method name → param names ===

fn collect_defs(stmts: &[Statement]) -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    collect_defs_stmts(stmts, &mut map);
    map
}

fn collect_defs_stmts(stmts: &[Statement], map: &mut HashMap<String, Vec<String>>) {
    for stmt in stmts {
        match stmt {
            Statement::Function(f) => {
                let params = f
                    .parameters
                    .parameters
                    .iter()
                    .map(|p| p.name.name.to_string())
                    .collect();
                map.insert(f.name.value.to_string(), params);
            }
            Statement::Class(c) => {
                for member in &c.body.members {
                    match member {
                        ClassMember::ConcreteMethod(m) => {
                            let params = m
                                .parameters
                                .parameters
                                .iter()
                                .map(|p| p.name.name.to_string())
                                .collect();
                            map.insert(m.name.value.to_string(), params);
                        }
                        _ => {}
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                collect_defs_stmts(inner, map);
            }
            _ => {}
        }
    }
}

// === AST walking ===

fn hints_in_stmts(
    source: &str,
    stmts: &[Statement],
    defs: &HashMap<String, Vec<String>>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    for stmt in stmts {
        hints_in_stmt(source, stmt, defs, range, out);
    }
}

fn hints_in_stmt(
    source: &str,
    stmt: &Statement,
    defs: &HashMap<String, Vec<String>>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    match stmt {
        Statement::Expression(e) => hints_in_expr(source, &e.expression, defs, range, out),
        Statement::Return(r) => {
            if let Some(v) = &r.value {
                hints_in_expr(source, v, defs, range, out);
            }
        }
        Statement::Echo(e) => {
            for expr in &e.values {
                hints_in_expr(source, expr, defs, range, out);
            }
        }
        Statement::Function(f) => {
            hints_in_stmts(source, &f.body.statements, defs, range, out);
        }
        Statement::Class(c) => {
            for member in &c.body.members {
                if let ClassMember::ConcreteMethod(m) = member {
                    hints_in_stmts(source, &m.body.statements, defs, range, out);
                }
            }
        }
        Statement::Namespace(ns) => {
            let inner = match ns {
                NamespaceStatement::Unbraced(u) => &u.statements[..],
                NamespaceStatement::Braced(b) => &b.body.statements[..],
            };
            hints_in_stmts(source, inner, defs, range, out);
        }
        _ => {}
    }
}

fn hints_in_expr(
    source: &str,
    expr: &Expression,
    defs: &HashMap<String, Vec<String>>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    match expr {
        Expression::FunctionCall(f) => {
            if let Some(name) = simple_ident_name(&f.target) {
                if let Some(params) = defs.get(&name) {
                    let positions = arg_start_positions(
                        source,
                        &f.arguments.left_parenthesis,
                        f.arguments.arguments.len(),
                    );
                    for (i, arg) in f.arguments.arguments.iter().enumerate() {
                        if let Argument::Positional(_) = arg {
                            if let (Some(param), Some(&pos)) =
                                (params.get(i), positions.get(i))
                            {
                                if pos_in_range(pos, range) {
                                    out.push(make_hint(pos, param));
                                }
                            }
                        }
                    }
                }
            }
            for arg in &f.arguments.arguments {
                hints_in_expr(source, arg_value(arg), defs, range, out);
            }
        }
        Expression::MethodCall(m) => {
            if let Some(name) = simple_ident_name(&m.method) {
                if let Some(params) = defs.get(&name) {
                    let positions = arg_start_positions(
                        source,
                        &m.arguments.left_parenthesis,
                        m.arguments.arguments.len(),
                    );
                    for (i, arg) in m.arguments.arguments.iter().enumerate() {
                        if let Argument::Positional(_) = arg {
                            if let (Some(param), Some(&pos)) =
                                (params.get(i), positions.get(i))
                            {
                                if pos_in_range(pos, range) {
                                    out.push(make_hint(pos, param));
                                }
                            }
                        }
                    }
                }
            }
            hints_in_expr(source, &m.target, defs, range, out);
            for arg in &m.arguments.arguments {
                hints_in_expr(source, arg_value(arg), defs, range, out);
            }
        }
        Expression::AssignmentOperation(a) => {
            hints_in_expr(source, a.right(), defs, range, out);
        }
        Expression::Parenthesized(p) => {
            hints_in_expr(source, &p.expr, defs, range, out);
        }
        Expression::Ternary(t) => {
            hints_in_expr(source, &t.condition, defs, range, out);
            hints_in_expr(source, &t.then, defs, range, out);
            hints_in_expr(source, &t.r#else, defs, range, out);
        }
        Expression::ShortTernary(t) => {
            hints_in_expr(source, &t.condition, defs, range, out);
            hints_in_expr(source, &t.r#else, defs, range, out);
        }
        _ => {}
    }
}

fn arg_value(arg: &Argument) -> &Expression {
    match arg {
        Argument::Positional(p) => &p.value,
        Argument::Named(n) => &n.value,
    }
}

fn simple_ident_name(expr: &Expression) -> Option<String> {
    match expr {
        Expression::Identifier(AstIdentifier::SimpleIdentifier(si)) => {
            Some(si.value.to_string())
        }
        _ => None,
    }
}

fn make_hint(position: Position, param_name: &str) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!("{}:", param_name.trim_start_matches('$'))),
        kind: Some(InlayHintKind::PARAMETER),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: Some(true),
        data: None,
    }
}

fn pos_in_range(pos: Position, range: Range) -> bool {
    pos.line >= range.start.line && pos.line <= range.end.line
}

// === Source-text scanning for argument positions ===

/// Scans forward from the `(` to find the LSP Position of the first
/// non-whitespace character of each argument. Returns up to `count` positions.
fn arg_start_positions(source: &str, open_paren: &TokenSpan, count: usize) -> Vec<Position> {
    if count == 0 {
        return vec![];
    }

    let paren_byte = match span_to_byte_offset(source, open_paren) {
        Some(b) => b,
        None => return vec![],
    };

    let rest = match source.get(paren_byte..) {
        Some(s) => s,
        None => return vec![],
    };

    let mut positions = Vec::new();
    let mut chars = rest.char_indices();

    // Consume the '('
    match chars.next() {
        Some((_, '(')) => {}
        _ => return vec![],
    }

    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut expect_arg = true;

    for (rel_off, ch) in chars {
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            }
            continue;
        }

        // Record argument start before consuming the character so that
        // quote/paren characters that open an argument are included.
        if expect_arg && depth == 0 && !ch.is_whitespace() {
            expect_arg = false;
            positions.push(byte_to_lsp_position(source, paren_byte + rel_off));
            if positions.len() >= count {
                break;
            }
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth > 0 => depth -= 1,
            ')' if depth == 0 => break,
            ',' if depth == 0 => expect_arg = true,
            _ => {}
        }
    }

    positions
}

// === Return-type inlay hints ===

/// Collect function/method name → return type string from definitions.
fn collect_return_types(stmts: &[Statement], map: &mut HashMap<String, String>) {
    for stmt in stmts {
        match stmt {
            Statement::Function(f) => {
                if let Some(rt) = &f.return_type {
                    map.insert(f.name.value.to_string(), rt.data_type.to_string());
                }
            }
            Statement::Class(c) => {
                for member in &c.body.members {
                    if let ClassMember::ConcreteMethod(m) = member {
                        if let Some(rt) = &m.return_type {
                            map.insert(m.name.value.to_string(), rt.data_type.to_string());
                        }
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                collect_return_types(inner, map);
            }
            _ => {}
        }
    }
}

/// Emit `: ReturnType` hint immediately after function call expressions
/// whose return type is known from a definition in the same AST.
fn return_type_hints(
    source: &str,
    stmts: &[Statement],
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    let mut ret_types = HashMap::new();
    collect_return_types(stmts, &mut ret_types);
    if ret_types.is_empty() {
        return;
    }
    ret_type_hints_in_stmts(source, stmts, &ret_types, range, out);
}

fn ret_type_hints_in_stmts(
    source: &str,
    stmts: &[Statement],
    ret_types: &HashMap<String, String>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    for stmt in stmts {
        match stmt {
            Statement::Expression(e) => ret_type_hints_in_expr(source, &e.expression, ret_types, range, out),
            Statement::Return(r) => {
                if let Some(v) = &r.value {
                    ret_type_hints_in_expr(source, v, ret_types, range, out);
                }
            }
            Statement::Function(f) => ret_type_hints_in_stmts(source, &f.body.statements, ret_types, range, out),
            Statement::Class(c) => {
                for member in &c.body.members {
                    if let ClassMember::ConcreteMethod(m) = member {
                        ret_type_hints_in_stmts(source, &m.body.statements, ret_types, range, out);
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                ret_type_hints_in_stmts(source, inner, ret_types, range, out);
            }
            _ => {}
        }
    }
}

fn ret_type_hints_in_expr(
    source: &str,
    expr: &Expression,
    ret_types: &HashMap<String, String>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    match expr {
        Expression::AssignmentOperation(a) => {
            // For `$x = someCall(...)`, show return type hint after the call
            ret_type_call_hint(source, a.right(), ret_types, range, out);
            // Recurse into the right side for nested assignments
            ret_type_hints_in_expr(source, a.right(), ret_types, range, out);
        }
        _ => {}
    }
}

/// Emit a return-type hint specifically on call expressions that are the
/// immediate RHS of an assignment.  This is the only context where the hint
/// is unambiguously useful (it shows the type of the variable being assigned).
fn ret_type_call_hint(
    source: &str,
    expr: &Expression,
    ret_types: &HashMap<String, String>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    match expr {
        Expression::FunctionCall(f) => {
            if let Some(name) = simple_ident_name(&f.target) {
                if let Some(ret_type) = ret_types.get(&name) {
                    // Skip `void` — not useful to show
                    if ret_type == "void" {
                        return;
                    }
                    let pos = after_span_position(source, &f.arguments.right_parenthesis);
                    if pos_in_range(pos, range) {
                        out.push(make_return_hint(pos, ret_type));
                    }
                }
            }
        }
        Expression::MethodCall(m) => {
            if let Some(name) = simple_ident_name(&m.method) {
                if let Some(ret_type) = ret_types.get(&name) {
                    if ret_type == "void" {
                        return;
                    }
                    let pos = after_span_position(source, &m.arguments.right_parenthesis);
                    if pos_in_range(pos, range) {
                        out.push(make_return_hint(pos, ret_type));
                    }
                }
            }
        }
        _ => {}
    }
}

fn after_span_position(source: &str, span: &TokenSpan) -> Position {
    // The span points to the `)` character itself (1-based).
    // We want the position one character after it.
    let line = span.line.saturating_sub(1) as u32;
    let character = span.column as u32; // column is 1-based, so column = char after )
    let _ = source; // source not needed when we trust the span
    Position { line, character }
}

fn make_return_hint(position: Position, ret_type: &str) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!(": {ret_type}")),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(true),
        padding_right: None,
        data: None,
    }
}

/// Converts a `Span` (1-based line/column) to a byte offset in `source`.
fn span_to_byte_offset(source: &str, span: &TokenSpan) -> Option<usize> {
    let target_line = (span.line as usize).saturating_sub(1);
    let target_col = (span.column as usize).saturating_sub(1);

    let mut byte_off = 0usize;
    for (i, line) in source.lines().enumerate() {
        if i == target_line {
            let result = byte_off + target_col;
            return if result <= source.len() { Some(result) } else { None };
        }
        byte_off += line.len() + 1; // +1 for '\n'
    }
    None
}

/// Converts a byte offset to an LSP `Position` (0-based line + UTF-16 character).
fn byte_to_lsp_position(source: &str, byte_off: usize) -> Position {
    let before = &source[..byte_off.min(source.len())];
    let line = before.bytes().filter(|&b| b == b'\n').count() as u32;
    let last_nl = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character: u32 = before[last_nl..].chars().map(|c| c.len_utf16() as u32).sum();
    Position { line, character }
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

    fn full_range() -> Range {
        Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: u32::MAX, character: u32::MAX },
        }
    }

    fn label_str(hint: &InlayHint) -> &str {
        match &hint.label {
            InlayHintLabel::String(s) => s.as_str(),
            InlayHintLabel::LabelParts(_) => "",
        }
    }

    #[test]
    fn emits_hint_for_single_param_call() {
        let src = "<?php\nfunction greet(string $name): void {}\ngreet('Alice');";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "name:");
    }

    #[test]
    fn emits_hints_for_multiple_params() {
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 2);
        assert_eq!(label_str(&hints[0]), "a:");
        assert_eq!(label_str(&hints[1]), "b:");
    }

    #[test]
    fn no_hints_for_unknown_function() {
        let src = "<?php\nunknownFn(1, 2);";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert!(hints.is_empty());
    }

    #[test]
    fn no_hints_for_zero_param_call() {
        let src = "<?php\nfunction init(): void {}\ninit();";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert!(hints.is_empty());
    }

    #[test]
    fn skips_named_arguments() {
        let src = "<?php\nfunction greet(string $name): void {}\ngreet(name: 'Alice');";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        // Named args already have the label in source — no hint needed
        assert!(hints.is_empty());
    }

    #[test]
    fn hints_for_method_call() {
        let src =
            "<?php\nclass Calc { public function add(int $x, int $y): int { return $x + $y; } }\n$c = new Calc();\n$c->add(3, 4);";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 2);
        assert_eq!(label_str(&hints[0]), "x:");
        assert_eq!(label_str(&hints[1]), "y:");
    }

    #[test]
    fn range_filter_excludes_out_of_range_hints() {
        let src = "<?php\nfunction greet(string $name): void {}\ngreet('Alice');\ngreet('Bob');";
        let ast = parse_ast(src);
        // Only include line 2 (0-based), which is the first call
        let range = Range {
            start: Position { line: 2, character: 0 },
            end: Position { line: 2, character: u32::MAX },
        };
        let hints = inlay_hints(src, &ast, range);
        assert_eq!(hints.len(), 1);
    }

    #[test]
    fn hint_kind_is_parameter() {
        let src = "<?php\nfunction f(int $x): void {}\nf(1);";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints[0].kind, Some(InlayHintKind::PARAMETER));
    }

    // --- Position accuracy ---

    #[test]
    fn hint_position_is_at_argument_start() {
        // greet( is on line 2 (0-based), '  is at character 6
        let src = "<?php\nfunction greet(string $name): void {}\ngreet('Alice');";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].position, Position { line: 2, character: 6 });
    }

    #[test]
    fn hint_positions_for_multiple_args() {
        // add(1, 2) on line 2: '1' at char 4, '2' at char 7
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 2);
        assert_eq!(hints[0].position, Position { line: 2, character: 4 });
        assert_eq!(hints[1].position, Position { line: 2, character: 7 });
    }

    // --- String arguments ---

    #[test]
    fn comma_inside_string_arg_does_not_create_extra_hint() {
        let src = "<?php\nfunction log(string $msg): void {}\nlog('hello, world');";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "msg:");
    }

    #[test]
    fn double_quoted_string_arg_gets_hint() {
        let src = "<?php\nfunction log(string $msg): void {}\nlog(\"hello\");";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "msg:");
    }

    // --- Arity mismatches ---

    #[test]
    fn fewer_args_than_params_emits_hints_for_provided_args_only() {
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1);";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "a:");
    }

    #[test]
    fn more_args_than_params_emits_hints_only_for_known_params() {
        let src = "<?php\nfunction f(int $x): void {}\nf(1, 2, 3);";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        // Only 1 param defined, so only 1 hint
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "x:");
    }

    // --- Nested calls ---

    #[test]
    fn nested_calls_emit_hints_at_both_levels() {
        let src = "<?php\nfunction double(int $n): int { return $n * 2; }\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(double(3), 4);";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        let labels: Vec<&str> = hints.iter().map(|h| label_str(h)).collect();
        assert!(labels.contains(&"a:"), "expected outer param 'a:'");
        assert!(labels.contains(&"b:"), "expected outer param 'b:'");
        assert!(labels.contains(&"n:"), "expected inner param 'n:'");
    }

    // --- Statement contexts ---

    #[test]
    fn hint_inside_return_statement() {
        let src = "<?php\nfunction double(int $n): int { return $n * 2; }\nfunction wrap(): int { return double(5); }";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "n:");
    }

    #[test]
    fn hint_inside_echo_statement() {
        let src = "<?php\nfunction greet(string $name): string { return $name; }\necho greet('Alice');";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "name:");
    }

    #[test]
    fn hint_for_call_inside_assignment() {
        let src = "<?php\nfunction double(int $n): int { return $n * 2; }\n$result = double(7);";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        // Expect parameter hint ("n:") + return-type hint (": int") for the assignment
        let param_hint = hints.iter().find(|h| label_str(h) == "n:");
        assert!(param_hint.is_some(), "expected parameter hint 'n:'");
        let ret_hint = hints.iter().find(|h| label_str(h).starts_with(": "));
        assert!(ret_hint.is_some(), "expected return-type hint");
    }

    #[test]
    fn return_type_hint_for_assignment() {
        let src = "<?php\nfunction make(): string { return 'x'; }\n$s = make();";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        let ret_hint = hints.iter().find(|h| label_str(h) == ": string");
        assert!(ret_hint.is_some(), "expected ': string' return type hint");
    }

    #[test]
    fn no_return_type_hint_for_void() {
        let src = "<?php\nfunction init(): void {}\n$x = init();";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        let ret_hint = hints.iter().find(|h| label_str(h).starts_with(": "));
        assert!(ret_hint.is_none(), "void return type should not produce a hint");
    }

    // --- Namespaced functions ---

    #[test]
    fn hints_for_function_inside_namespace() {
        let src = "<?php\nnamespace App;\nfunction greet(string $name): void {}\ngreet('Alice');";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "name:");
    }

    // --- Multi-line calls ---

    #[test]
    fn hints_for_multi_line_argument_list() {
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(\n    1,\n    2\n);";
        let ast = parse_ast(src);
        let hints = inlay_hints(src, &ast, full_range());
        assert_eq!(hints.len(), 2);
        assert_eq!(label_str(&hints[0]), "a:");
        assert_eq!(label_str(&hints[1]), "b:");
        // Arguments are on lines 3 and 4 (0-based)
        assert_eq!(hints[0].position.line, 3);
        assert_eq!(hints[1].position.line, 4);
    }
}
