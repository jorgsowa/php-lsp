use php_parser_rs::parser::ast::{
    classes::ClassMember,
    control_flow::{IfStatementBody, IfStatementElseIf, IfStatementElseIfBlock},
    loops::{ForeachStatementBody, ForeachStatementIterator, ForStatementBody, WhileStatementBody},
    namespaces::NamespaceStatement,
    traits::TraitMember,
    Expression, Statement,
};
use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind};

pub fn folding_ranges(ast: &[Statement]) -> Vec<FoldingRange> {
    let mut ranges = Vec::new();
    fold_stmts(ast, &mut ranges);
    ranges
}

fn fold_stmts(stmts: &[Statement], out: &mut Vec<FoldingRange>) {
    for stmt in stmts {
        fold_stmt(stmt, out);
    }
}

fn fold_stmt(stmt: &Statement, out: &mut Vec<FoldingRange>) {
    match stmt {
        Statement::Function(f) => {
            push(out, f.function.line, f.body.right_brace.line, None);
            fold_stmts(&f.body.statements, out);
        }
        Statement::Class(c) => {
            push(out, c.class.line, c.body.right_brace.line, None);
            for member in &c.body.members {
                fold_class_member(member, out);
            }
        }
        Statement::Interface(i) => {
            push(out, i.interface.line, i.body.right_brace.line, None);
        }
        Statement::Trait(t) => {
            push(out, t.r#trait.line, t.body.right_brace.line, None);
            for member in &t.body.members {
                fold_trait_member(member, out);
            }
        }
        Statement::Namespace(ns) => match ns {
            NamespaceStatement::Braced(b) => {
                // BracedNamespaceBody has start/end, not left_brace/right_brace
                push(out, b.body.start.line, b.body.end.line, None);
                fold_stmts(&b.body.statements, out);
            }
            NamespaceStatement::Unbraced(u) => {
                fold_stmts(&u.statements, out);
            }
        },
        Statement::If(i) => {
            fold_if_body(&i.body, out);
        }
        Statement::While(w) => match &w.body {
            // Regular { } body — the block statement carries the brace spans
            WhileStatementBody::Statement { statement } => fold_stmt(statement, out),
            // Alternative syntax `while (): ... endwhile;`
            WhileStatementBody::Block { statements, colon, endwhile, .. } => {
                push(out, colon.line, endwhile.line, None);
                fold_stmts(statements, out);
            }
        },
        Statement::DoWhile(d) => {
            fold_stmt(&d.body, out);
        }
        Statement::For(f) => match &f.body {
            ForStatementBody::Statement { statement } => fold_stmt(statement, out),
            ForStatementBody::Block { statements, colon, endfor, .. } => {
                push(out, colon.line, endfor.line, None);
                fold_stmts(statements, out);
            }
        },
        Statement::Foreach(f) => {
            let expr = match &f.iterator {
                ForeachStatementIterator::Value { expression, .. } => expression,
                ForeachStatementIterator::KeyAndValue { expression, .. } => expression,
            };
            fold_expr(expr, out);
            match &f.body {
                ForeachStatementBody::Statement { statement } => fold_stmt(statement, out),
                ForeachStatementBody::Block { statements, colon, endforeach, .. } => {
                    push(out, colon.line, endforeach.line, None);
                    fold_stmts(statements, out);
                }
            }
        }
        Statement::Try(t) => {
            // TryStatement has start/end fields
            push(out, t.start.line, t.end.line, None);
            fold_stmts(&t.body, out);
            for catch in &t.catches {
                push(out, catch.start.line, catch.end.line, None);
                fold_stmts(&catch.body, out);
            }
            if let Some(finally) = &t.finally {
                push(out, finally.start.line, finally.end.line, None);
                fold_stmts(&finally.body, out);
            }
        }
        // Curly-brace blocks carry their own brace spans
        Statement::Block(b) => {
            push(out, b.left_brace.line, b.right_brace.line, None);
            fold_stmts(&b.statements, out);
        }
        Statement::Expression(e) => fold_expr(&e.expression, out),
        Statement::Return(r) => {
            if let Some(v) = &r.value {
                fold_expr(v, out);
            }
        }
        _ => {}
    }
}

fn fold_class_member(member: &ClassMember, out: &mut Vec<FoldingRange>) {
    match member {
        ClassMember::ConcreteMethod(m) => {
            push(out, m.function.line, m.body.right_brace.line, None);
            fold_stmts(&m.body.statements, out);
        }
        ClassMember::ConcreteConstructor(c) => {
            push(out, c.function.line, c.body.right_brace.line, None);
            fold_stmts(&c.body.statements, out);
        }
        _ => {}
    }
}

fn fold_trait_member(member: &TraitMember, out: &mut Vec<FoldingRange>) {
    if let TraitMember::ConcreteMethod(m) = member {
        push(out, m.function.line, m.body.right_brace.line, None);
        fold_stmts(&m.body.statements, out);
    }
}

fn fold_if_body(body: &IfStatementBody, out: &mut Vec<FoldingRange>) {
    match body {
        // Regular curly-brace if — the statement is a Block carrying brace spans
        IfStatementBody::Statement { statement, elseifs, r#else } => {
            fold_stmt(statement, out);
            for ei in elseifs {
                fold_elseif(ei, out);
            }
            if let Some(e) = r#else {
                fold_stmt(&e.statement, out);
            }
        }
        // Alternative syntax `if (): ... elseif (): ... else: ... endif;`
        // IfStatementElseIfBlock has no brace spans; recurse into statements only
        IfStatementBody::Block { statements, elseifs, r#else, .. } => {
            fold_stmts(statements, out);
            for ei in elseifs {
                fold_elseif_block(ei, out);
            }
            if let Some(e) = r#else {
                fold_stmts(&e.statements, out);
            }
        }
    }
}

fn fold_elseif(ei: &IfStatementElseIf, out: &mut Vec<FoldingRange>) {
    fold_stmt(&ei.statement, out);
}

fn fold_elseif_block(ei: &IfStatementElseIfBlock, out: &mut Vec<FoldingRange>) {
    fold_stmts(&ei.statements, out);
}

fn fold_expr(expr: &Expression, out: &mut Vec<FoldingRange>) {
    match expr {
        Expression::Closure(c) => {
            push(out, c.function.line, c.body.right_brace.line, None);
            fold_stmts(&c.body.statements, out);
        }
        Expression::Match(m) => {
            push(out, m.keyword.line, m.right_brace.line, None);
        }
        Expression::AssignmentOperation(a) => {
            fold_expr(a.right(), out);
        }
        Expression::Parenthesized(p) => {
            fold_expr(&p.expr, out);
        }
        _ => {}
    }
}

fn push(out: &mut Vec<FoldingRange>, start: usize, end: usize, kind: Option<FoldingRangeKind>) {
    let start_line = (start as u32).saturating_sub(1);
    let end_line = (end as u32).saturating_sub(1);
    if end_line > start_line {
        out.push(FoldingRange {
            start_line,
            start_character: None,
            end_line,
            end_character: None,
            kind,
            collapsed_text: None,
        });
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

    fn lines(ranges: &[FoldingRange]) -> Vec<(u32, u32)> {
        ranges.iter().map(|r| (r.start_line, r.end_line)).collect()
    }

    #[test]
    fn folds_top_level_function() {
        let src = "<?php\nfunction greet(): void {\n    echo 'hi';\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        assert!(
            ranges.iter().any(|r| r.start_line == 1 && r.end_line == 3),
            "expected function fold (1..3), got {:?}",
            lines(&ranges)
        );
    }

    #[test]
    fn folds_class_and_its_method() {
        let src = "<?php\nclass Foo {\n    public function bar(): void {\n        echo 1;\n    }\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        let ls = lines(&ranges);
        assert!(ls.contains(&(1, 5)), "expected class fold (1..5), got {:?}", ls);
        assert!(ls.contains(&(2, 4)), "expected method fold (2..4), got {:?}", ls);
    }

    #[test]
    fn folds_interface() {
        let src = "<?php\ninterface Countable {\n    public function count(): int;\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        assert!(
            ranges.iter().any(|r| r.start_line == 1),
            "expected interface fold, got {:?}",
            lines(&ranges)
        );
    }

    #[test]
    fn folds_trait_and_its_method() {
        let src = "<?php\ntrait Loggable {\n    public function log(): void {\n        echo 'log';\n    }\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        let ls = lines(&ranges);
        assert!(ls.contains(&(1, 5)), "expected trait fold (1..5), got {:?}", ls);
        assert!(ls.contains(&(2, 4)), "expected method fold (2..4), got {:?}", ls);
    }

    #[test]
    fn folds_braced_namespace() {
        let src = "<?php\nnamespace App {\n    function boot(): void {\n        return;\n    }\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        let ls = lines(&ranges);
        assert!(ls.contains(&(1, 5)), "expected namespace fold (1..5), got {:?}", ls);
        assert!(ls.contains(&(2, 4)), "expected function fold (2..4), got {:?}", ls);
    }

    #[test]
    fn folds_if_block() {
        let src = "<?php\nif (true) {\n    echo 1;\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        assert!(
            ranges.iter().any(|r| r.start_line == 1 && r.end_line == 3),
            "expected if fold (1..3), got {:?}",
            lines(&ranges)
        );
    }

    #[test]
    fn folds_if_else_blocks() {
        let src = "<?php\nif (true) {\n    echo 1;\n} else {\n    echo 2;\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        let ls = lines(&ranges);
        assert!(ls.iter().any(|&(s, _)| s == 1), "expected if branch fold, got {:?}", ls);
        assert!(ls.iter().any(|&(s, _)| s == 3), "expected else branch fold, got {:?}", ls);
    }

    #[test]
    fn folds_while_block() {
        let src = "<?php\nwhile (true) {\n    echo 1;\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        assert!(
            ranges.iter().any(|r| r.start_line == 1 && r.end_line == 3),
            "expected while fold (1..3), got {:?}",
            lines(&ranges)
        );
    }

    #[test]
    fn folds_for_block() {
        let src = "<?php\nfor ($i = 0; $i < 10; $i++) {\n    echo $i;\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        assert!(
            ranges.iter().any(|r| r.start_line == 1 && r.end_line == 3),
            "expected for fold (1..3), got {:?}",
            lines(&ranges)
        );
    }

    #[test]
    fn folds_foreach_block() {
        let src = "<?php\nforeach ($items as $item) {\n    echo $item;\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        assert!(
            ranges.iter().any(|r| r.start_line == 1 && r.end_line == 3),
            "expected foreach fold (1..3), got {:?}",
            lines(&ranges)
        );
    }

    #[test]
    fn folds_try_catch_finally() {
        let src = "<?php\ntry {\n    foo();\n} catch (Exception $e) {\n    bar();\n} finally {\n    baz();\n}";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        assert!(ranges.len() >= 3, "expected try + catch + finally folds, got {:?}", lines(&ranges));
    }

    #[test]
    fn folds_closure() {
        let src = "<?php\n$fn = function() {\n    return 1;\n};";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        assert!(
            ranges.iter().any(|r| r.start_line == 1 && r.end_line == 3),
            "expected closure fold (1..3), got {:?}",
            lines(&ranges)
        );
    }

    #[test]
    fn single_line_construct_produces_no_fold() {
        let src = "<?php\nfunction f(): void { echo 1; }";
        let ast = parse_ast(src);
        let ranges = folding_ranges(&ast);
        assert!(ranges.is_empty(), "single-line function should not fold, got {:?}", ranges);
    }

    #[test]
    fn no_folds_for_empty_file() {
        let src = "<?php";
        let ast = parse_ast(src);
        assert!(folding_ranges(&ast).is_empty());
    }
}
