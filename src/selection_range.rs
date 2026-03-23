use php_parser_rs::parser::ast::{
    classes::ClassMember,
    control_flow::IfStatementBody,
    loops::{ForeachStatementBody, ForeachStatementIterator, ForStatementBody, WhileStatementBody},
    namespaces::NamespaceStatement,
    traits::TraitMember,
    Statement,
};
use tower_lsp::lsp_types::{Position, Range, SelectionRange};

use crate::diagnostics::span_to_position;

/// Build a selection-range chain for each cursor position.
/// Levels go from innermost to outermost via `parent` links.
pub fn selection_ranges(ast: &[Statement], positions: &[Position]) -> Vec<SelectionRange> {
    let file_range = file_range(ast);
    positions
        .iter()
        .map(|pos| build_chain(ast, *pos, file_range))
        .collect()
}

/// Fallback: the entire file as a single range (line 0 to a large number)
fn file_range(ast: &[Statement]) -> Range {
    // Find the last line from the AST
    let last_line = last_line_in_stmts(ast);
    Range {
        start: Position { line: 0, character: 0 },
        end: Position { line: last_line, character: u32::MAX },
    }
}

fn last_line_in_stmts(stmts: &[Statement]) -> u32 {
    stmts.iter().map(stmt_last_line).max().unwrap_or(0)
}

fn stmt_last_line(stmt: &Statement) -> u32 {
    match stmt {
        Statement::Function(f) => (f.body.right_brace.line as u32).saturating_sub(1),
        Statement::Class(c) => (c.body.right_brace.line as u32).saturating_sub(1),
        Statement::Interface(i) => (i.body.right_brace.line as u32).saturating_sub(1),
        Statement::Trait(t) => (t.body.right_brace.line as u32).saturating_sub(1),
        Statement::Namespace(ns) => match ns {
            NamespaceStatement::Braced(b) => (b.body.end.line as u32).saturating_sub(1),
            NamespaceStatement::Unbraced(u) => last_line_in_stmts(&u.statements),
        },
        _ => 0,
    }
}

/// Build the innermost-to-outermost chain for a cursor position.
fn build_chain(ast: &[Statement], pos: Position, file_range: Range) -> SelectionRange {
    let mut ranges: Vec<Range> = Vec::new();
    collect_ranges_stmts(ast, pos, &mut ranges);

    // Sort from smallest span to largest (innermost first)
    ranges.sort_by_key(|r| {
        let lines = r.end.line.saturating_sub(r.start.line);
        let chars = if r.start.line == r.end.line {
            r.end.character.saturating_sub(r.start.character)
        } else {
            u32::MAX
        };
        (lines, chars)
    });

    // Dedup exact duplicates
    ranges.dedup();

    // Ensure the file-level range is at the outermost position
    if !ranges.contains(&file_range) {
        ranges.push(file_range);
    }

    // Build linked chain from outermost inward, then return innermost
    let mut chain: Option<SelectionRange> = None;
    for range in ranges.into_iter().rev() {
        chain = Some(SelectionRange {
            range,
            parent: chain.map(Box::new),
        });
    }

    chain.unwrap_or(SelectionRange { range: file_range, parent: None })
}

fn contains(range: Range, pos: Position) -> bool {
    pos.line >= range.start.line && pos.line <= range.end.line
}

fn span_range(
    start: &php_parser_rs::lexer::token::Span,
    end: &php_parser_rs::lexer::token::Span,
) -> Range {
    let s = span_to_position(start);
    let e = span_to_position(end);
    Range {
        start: s,
        end: Position { line: e.line, character: e.character + 1 },
    }
}

fn collect_ranges_stmts(stmts: &[Statement], pos: Position, out: &mut Vec<Range>) {
    for stmt in stmts {
        collect_ranges_stmt(stmt, pos, out);
    }
}

fn collect_ranges_stmt(stmt: &Statement, pos: Position, out: &mut Vec<Range>) {
    match stmt {
        Statement::Function(f) => {
            let range = span_range(&f.function, &f.body.right_brace);
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            // Name selection range
            let name = f.name.value.to_string();
            let name_start = span_to_position(&f.name.span);
            out.push(Range {
                start: name_start,
                end: Position { line: name_start.line, character: name_start.character + name.len() as u32 },
            });
            // Body range
            let body_range = span_range(&f.body.left_brace, &f.body.right_brace);
            if contains(body_range, pos) {
                out.push(body_range);
                collect_ranges_stmts(&f.body.statements, pos, out);
            }
        }
        Statement::Class(c) => {
            let range = span_range(&c.class, &c.body.right_brace);
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            for member in &c.body.members {
                collect_ranges_class_member(member, pos, out);
            }
        }
        Statement::Interface(i) => {
            let range = span_range(&i.interface, &i.body.right_brace);
            if contains(range, pos) {
                out.push(range);
            }
        }
        Statement::Trait(t) => {
            let range = span_range(&t.r#trait, &t.body.right_brace);
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            for member in &t.body.members {
                collect_ranges_trait_member(member, pos, out);
            }
        }
        Statement::Namespace(ns) => match ns {
            NamespaceStatement::Braced(b) => {
                let range = Range {
                    start: span_to_position(&b.body.start),
                    end: Position {
                        line: span_to_position(&b.body.end).line,
                        character: span_to_position(&b.body.end).character + 1,
                    },
                };
                if contains(range, pos) {
                    out.push(range);
                    collect_ranges_stmts(&b.body.statements, pos, out);
                }
            }
            NamespaceStatement::Unbraced(u) => {
                collect_ranges_stmts(&u.statements, pos, out);
            }
        },
        Statement::If(i) => {
            match &i.body {
                IfStatementBody::Statement { statement, elseifs, r#else } => {
                    collect_ranges_stmt(statement, pos, out);
                    for ei in elseifs {
                        collect_ranges_stmt(&ei.statement, pos, out);
                    }
                    if let Some(e) = r#else {
                        collect_ranges_stmt(&e.statement, pos, out);
                    }
                }
                IfStatementBody::Block { statements, elseifs, r#else, .. } => {
                    collect_ranges_stmts(statements, pos, out);
                    for ei in elseifs {
                        collect_ranges_stmts(&ei.statements, pos, out);
                    }
                    if let Some(e) = r#else {
                        collect_ranges_stmts(&e.statements, pos, out);
                    }
                }
            }
        }
        Statement::While(w) => {
            match &w.body {
                WhileStatementBody::Statement { statement } => collect_ranges_stmt(statement, pos, out),
                WhileStatementBody::Block { statements, .. } => collect_ranges_stmts(statements, pos, out),
            }
        }
        Statement::For(f) => {
            match &f.body {
                ForStatementBody::Statement { statement } => collect_ranges_stmt(statement, pos, out),
                ForStatementBody::Block { statements, .. } => collect_ranges_stmts(statements, pos, out),
            }
        }
        Statement::Foreach(f) => {
            let expr = match &f.iterator {
                ForeachStatementIterator::Value { expression, .. } => expression,
                ForeachStatementIterator::KeyAndValue { expression, .. } => expression,
            };
            let _ = expr;
            match &f.body {
                ForeachStatementBody::Statement { statement } => collect_ranges_stmt(statement, pos, out),
                ForeachStatementBody::Block { statements, .. } => collect_ranges_stmts(statements, pos, out),
            }
        }
        Statement::Try(t) => {
            let range = Range {
                start: span_to_position(&t.start),
                end: Position {
                    line: span_to_position(&t.end).line,
                    character: span_to_position(&t.end).character + 1,
                },
            };
            if contains(range, pos) {
                out.push(range);
                collect_ranges_stmts(&t.body, pos, out);
            }
        }
        Statement::Block(b) => {
            let range = span_range(&b.left_brace, &b.right_brace);
            if contains(range, pos) {
                out.push(range);
                collect_ranges_stmts(&b.statements, pos, out);
            }
        }
        _ => {}
    }
}

fn collect_ranges_class_member(member: &ClassMember, pos: Position, out: &mut Vec<Range>) {
    match member {
        ClassMember::ConcreteMethod(m) => {
            let range = span_range(&m.function, &m.body.right_brace);
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            let body_range = span_range(&m.body.left_brace, &m.body.right_brace);
            if contains(body_range, pos) {
                out.push(body_range);
                collect_ranges_stmts(&m.body.statements, pos, out);
            }
        }
        ClassMember::AbstractMethod(m) => {
            let start = span_to_position(&m.function);
            let end = span_to_position(&m.semicolon);
            let range = Range {
                start,
                end: Position { line: end.line, character: end.character + 1 },
            };
            if contains(range, pos) {
                out.push(range);
            }
        }
        _ => {}
    }
}

fn collect_ranges_trait_member(member: &TraitMember, pos: Position, out: &mut Vec<Range>) {
    if let TraitMember::ConcreteMethod(m) = member {
        let range = span_range(&m.function, &m.body.right_brace);
        if !contains(range, pos) {
            return;
        }
        out.push(range);
        let body_range = span_range(&m.body.left_brace, &m.body.right_brace);
        if contains(body_range, pos) {
            out.push(body_range);
            collect_ranges_stmts(&m.body.statements, pos, out);
        }
    }
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

    fn chain_ranges(sr: &SelectionRange) -> Vec<Range> {
        let mut ranges = vec![sr.range];
        let mut current = sr.parent.as_deref();
        while let Some(p) = current {
            ranges.push(p.range);
            current = p.parent.as_deref();
        }
        ranges
    }

    #[test]
    fn returns_one_result_per_position() {
        let ast = parse_ast("<?php\nfunction greet() {}");
        let positions = vec![pos(1, 10), pos(0, 0)];
        let result = selection_ranges(&ast, &positions);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn empty_file_returns_file_range() {
        let ast = parse_ast("<?php");
        let result = selection_ranges(&ast, &[pos(0, 0)]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].range.start.line, 0);
    }

    #[test]
    fn cursor_in_function_body_includes_function_range() {
        // line 0: <?php
        // line 1: function greet() {
        // line 2:     echo 'hi';
        // line 3: }
        let src = "<?php\nfunction greet() {\n    echo 'hi';\n}";
        let ast = parse_ast(src);
        let result = selection_ranges(&ast, &[pos(2, 4)]);
        let ranges = chain_ranges(&result[0]);
        // At least one range should span the function (starting at line 1)
        assert!(
            ranges.iter().any(|r| r.start.line == 1),
            "expected a range starting at line 1 (function), got {:?}", ranges
        );
    }

    #[test]
    fn cursor_in_method_body_includes_method_and_class_ranges() {
        // line 0: <?php
        // line 1: class Foo {
        // line 2:     public function bar() {
        // line 3:         echo 1;
        // line 4:     }
        // line 5: }
        let src = "<?php\nclass Foo {\n    public function bar() {\n        echo 1;\n    }\n}";
        let ast = parse_ast(src);
        let result = selection_ranges(&ast, &[pos(3, 8)]);
        let ranges = chain_ranges(&result[0]);
        // Should include a range starting at line 1 (class)
        assert!(
            ranges.iter().any(|r| r.start.line == 1),
            "expected class-level range at line 1, got {:?}", ranges
        );
        // Should include a range starting at line 2 (method)
        assert!(
            ranges.iter().any(|r| r.start.line == 2),
            "expected method-level range at line 2, got {:?}", ranges
        );
    }

    #[test]
    fn cursor_outside_all_nodes_returns_file_range_only() {
        let src = "<?php\n// comment\n";
        let ast = parse_ast(src);
        let result = selection_ranges(&ast, &[pos(1, 0)]);
        // Should still return a valid result (at least file range)
        assert!(!result.is_empty());
        assert_eq!(result[0].range.start.line, 0);
    }

    #[test]
    fn chain_is_ordered_innermost_to_outermost() {
        let src = "<?php\nclass Foo {\n    public function bar() {\n        echo 1;\n    }\n}";
        let ast = parse_ast(src);
        let result = selection_ranges(&ast, &[pos(3, 8)]);
        let ranges = chain_ranges(&result[0]);
        // Each successive range should be >= the previous
        for window in ranges.windows(2) {
            let inner = &window[0];
            let outer = &window[1];
            let inner_lines = inner.end.line - inner.start.line;
            let outer_lines = outer.end.line - outer.start.line;
            assert!(
                outer_lines >= inner_lines,
                "outer range should be >= inner range in size: inner={:?}, outer={:?}", inner, outer
            );
        }
    }

    #[test]
    fn multiple_positions_are_independent() {
        let src = "<?php\nfunction a() {}\nfunction b() {}";
        let ast = parse_ast(src);
        let result = selection_ranges(&ast, &[pos(1, 10), pos(2, 10)]);
        assert_eq!(result.len(), 2);
        // The innermost ranges for each should start on different lines
        assert_ne!(result[0].range, result[1].range);
    }
}
