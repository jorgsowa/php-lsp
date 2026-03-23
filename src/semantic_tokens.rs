use php_parser_rs::parser::ast::{
    classes::ClassMember,
    identifiers::Identifier as AstIdentifier,
    namespaces::NamespaceStatement,
    properties::PropertyEntry,
    traits::TraitMember,
    Expression, Statement,
};
use tower_lsp::lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend,
};

use crate::diagnostics::span_to_position;

// Token type indices — order must match `legend()` vec order
#[allow(dead_code)]
const TT_NAMESPACE: u32 = 0;
const TT_CLASS: u32 = 1;
const TT_INTERFACE: u32 = 2;
const TT_FUNCTION: u32 = 3;
const TT_METHOD: u32 = 4;
const TT_PROPERTY: u32 = 5;
#[allow(dead_code)]
const TT_VARIABLE: u32 = 6;
const TT_PARAMETER: u32 = 7;
#[allow(dead_code)]
const TT_TYPE: u32 = 8;

// Modifier bits — order must match `legend()` modifier vec order
const MOD_DECLARATION: u32 = 1 << 0;
const MOD_STATIC: u32 = 1 << 1;
const MOD_ABSTRACT: u32 = 1 << 2;
#[allow(dead_code)]
const MOD_READONLY: u32 = 1 << 3;

/// Raw token: (line_0based, col_0based, length, token_type, modifiers_bitmask)
type RawToken = (u32, u32, u32, u32, u32);

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::NAMESPACE,
            SemanticTokenType::CLASS,
            SemanticTokenType::INTERFACE,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::METHOD,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PARAMETER,
            SemanticTokenType::TYPE,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::STATIC,
            SemanticTokenModifier::ABSTRACT,
            SemanticTokenModifier::READONLY,
        ],
    }
}

pub fn semantic_tokens(ast: &[Statement]) -> Vec<SemanticToken> {
    let mut raw: Vec<RawToken> = Vec::new();
    collect_stmts(ast, &mut raw);
    raw.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    delta_encode(raw)
}

fn push_span(
    out: &mut Vec<RawToken>,
    span: &php_parser_rs::lexer::token::Span,
    len: u32,
    token_type: u32,
    modifiers: u32,
) {
    let pos = span_to_position(span);
    out.push((pos.line, pos.character, len, token_type, modifiers));
}

fn collect_stmts(stmts: &[Statement], out: &mut Vec<RawToken>) {
    for stmt in stmts {
        collect_stmt(stmt, out);
    }
}

fn collect_stmt(stmt: &Statement, out: &mut Vec<RawToken>) {
    match stmt {
        Statement::Function(f) => {
            let name = f.name.value.to_string();
            push_span(out, &f.name.span, name.len() as u32, TT_FUNCTION, MOD_DECLARATION);
            for p in f.parameters.parameters.iter() {
                let pname = p.name.name.to_string();
                push_span(out, &p.name.span, pname.len() as u32, TT_PARAMETER, MOD_DECLARATION);
            }
            collect_stmts(&f.body.statements, out);
        }
        Statement::Class(c) => {
            let name = c.name.value.to_string();
            push_span(out, &c.name.span, name.len() as u32, TT_CLASS, MOD_DECLARATION);
            for member in &c.body.members {
                collect_class_member(member, out);
            }
        }
        Statement::Interface(i) => {
            let name = i.name.value.to_string();
            push_span(out, &i.name.span, name.len() as u32, TT_INTERFACE, MOD_DECLARATION);
        }
        Statement::Trait(t) => {
            let name = t.name.value.to_string();
            push_span(out, &t.name.span, name.len() as u32, TT_CLASS, MOD_DECLARATION);
            for member in &t.body.members {
                collect_trait_member(member, out);
            }
        }
        Statement::Namespace(ns) => {
            let inner = match ns {
                NamespaceStatement::Unbraced(u) => &u.statements[..],
                NamespaceStatement::Braced(b) => &b.body.statements[..],
            };
            collect_stmts(inner, out);
        }
        Statement::Expression(e) => collect_expr(&e.expression, out),
        Statement::Return(r) => {
            if let Some(v) = &r.value {
                collect_expr(v, out);
            }
        }
        Statement::Echo(e) => {
            for expr in &e.values {
                collect_expr(expr, out);
            }
        }
        Statement::If(i) => {
            use php_parser_rs::parser::ast::control_flow::IfStatementBody;
            collect_expr(&i.condition, out);
            match &i.body {
                IfStatementBody::Statement { statement, elseifs, r#else } => {
                    collect_stmt(statement, out);
                    for ei in elseifs {
                        collect_expr(&ei.condition, out);
                        collect_stmt(&ei.statement, out);
                    }
                    if let Some(e) = r#else {
                        collect_stmt(&e.statement, out);
                    }
                }
                IfStatementBody::Block { statements, elseifs, r#else, .. } => {
                    collect_stmts(statements, out);
                    for ei in elseifs {
                        collect_expr(&ei.condition, out);
                        collect_stmts(&ei.statements, out);
                    }
                    if let Some(e) = r#else {
                        collect_stmts(&e.statements, out);
                    }
                }
            }
        }
        Statement::While(w) => {
            use php_parser_rs::parser::ast::loops::WhileStatementBody;
            collect_expr(&w.condition, out);
            match &w.body {
                WhileStatementBody::Statement { statement } => collect_stmt(statement, out),
                WhileStatementBody::Block { statements, .. } => collect_stmts(statements, out),
            }
        }
        Statement::For(f) => {
            use php_parser_rs::parser::ast::loops::ForStatementBody;
            for cond in &f.iterator.conditions.inner {
                collect_expr(cond, out);
            }
            match &f.body {
                ForStatementBody::Statement { statement } => collect_stmt(statement, out),
                ForStatementBody::Block { statements, .. } => collect_stmts(statements, out),
            }
        }
        Statement::Foreach(f) => {
            use php_parser_rs::parser::ast::loops::{ForeachStatementBody, ForeachStatementIterator};
            let expr = match &f.iterator {
                ForeachStatementIterator::Value { expression, .. } => expression,
                ForeachStatementIterator::KeyAndValue { expression, .. } => expression,
            };
            collect_expr(expr, out);
            match &f.body {
                ForeachStatementBody::Statement { statement } => collect_stmt(statement, out),
                ForeachStatementBody::Block { statements, .. } => collect_stmts(statements, out),
            }
        }
        Statement::Try(t) => {
            collect_stmts(&t.body, out);
            for catch in &t.catches {
                collect_stmts(&catch.body, out);
            }
            if let Some(finally) = &t.finally {
                collect_stmts(&finally.body, out);
            }
        }
        Statement::Block(b) => collect_stmts(&b.statements, out),
        _ => {}
    }
}

fn collect_class_member(member: &ClassMember, out: &mut Vec<RawToken>) {
    match member {
        ClassMember::ConcreteMethod(m) => {
            let mname = m.name.value.to_string();
            let mut mods = MOD_DECLARATION;
            if m.modifiers.has_static() {
                mods |= MOD_STATIC;
            }
            push_span(out, &m.name.span, mname.len() as u32, TT_METHOD, mods);
            for p in m.parameters.parameters.iter() {
                let pname = p.name.name.to_string();
                push_span(out, &p.name.span, pname.len() as u32, TT_PARAMETER, MOD_DECLARATION);
            }
            collect_stmts(&m.body.statements, out);
        }
        ClassMember::AbstractMethod(m) => {
            let mname = m.name.value.to_string();
            let mut mods = MOD_DECLARATION | MOD_ABSTRACT;
            if m.modifiers.has_static() {
                mods |= MOD_STATIC;
            }
            push_span(out, &m.name.span, mname.len() as u32, TT_METHOD, mods);
            for p in m.parameters.parameters.iter() {
                let pname = p.name.name.to_string();
                push_span(out, &p.name.span, pname.len() as u32, TT_PARAMETER, MOD_DECLARATION);
            }
        }
        ClassMember::Property(p) => {
            for entry in &p.entries {
                collect_property_entry(entry, out);
            }
        }
        ClassMember::VariableProperty(p) => {
            for entry in &p.entries {
                collect_property_entry(entry, out);
            }
        }
        _ => {}
    }
}

fn collect_property_entry(entry: &PropertyEntry, out: &mut Vec<RawToken>) {
    let var = entry.variable();
    let vname = var.name.to_string();
    push_span(out, &var.span, vname.len() as u32, TT_PROPERTY, MOD_DECLARATION);
}

fn collect_trait_member(member: &TraitMember, out: &mut Vec<RawToken>) {
    match member {
        TraitMember::ConcreteMethod(m) => {
            let mname = m.name.value.to_string();
            let mut mods = MOD_DECLARATION;
            if m.modifiers.has_static() {
                mods |= MOD_STATIC;
            }
            push_span(out, &m.name.span, mname.len() as u32, TT_METHOD, mods);
            for p in m.parameters.parameters.iter() {
                let pname = p.name.name.to_string();
                push_span(out, &p.name.span, pname.len() as u32, TT_PARAMETER, MOD_DECLARATION);
            }
            collect_stmts(&m.body.statements, out);
        }
        TraitMember::AbstractMethod(m) => {
            let mname = m.name.value.to_string();
            push_span(out, &m.name.span, mname.len() as u32, TT_METHOD, MOD_DECLARATION | MOD_ABSTRACT);
            for p in m.parameters.parameters.iter() {
                let pname = p.name.name.to_string();
                push_span(out, &p.name.span, pname.len() as u32, TT_PARAMETER, MOD_DECLARATION);
            }
        }
        _ => {}
    }
}

fn collect_expr(expr: &Expression, out: &mut Vec<RawToken>) {
    match expr {
        Expression::FunctionCall(f) => {
            if let Expression::Identifier(AstIdentifier::SimpleIdentifier(si)) = f.target.as_ref() {
                let name = si.value.to_string();
                push_span(out, &si.span, name.len() as u32, TT_FUNCTION, 0);
            } else {
                collect_expr(&f.target, out);
            }
            collect_args(&f.arguments, out);
        }
        Expression::MethodCall(m) => {
            collect_expr(&m.target, out);
            if let Expression::Identifier(AstIdentifier::SimpleIdentifier(si)) = m.method.as_ref() {
                let name = si.value.to_string();
                push_span(out, &si.span, name.len() as u32, TT_METHOD, 0);
            }
            collect_args(&m.arguments, out);
        }
        Expression::NullsafeMethodCall(m) => {
            collect_expr(&m.target, out);
            if let Expression::Identifier(AstIdentifier::SimpleIdentifier(si)) = m.method.as_ref() {
                let name = si.value.to_string();
                push_span(out, &si.span, name.len() as u32, TT_METHOD, 0);
            }
            collect_args(&m.arguments, out);
        }
        Expression::StaticMethodCall(s) => {
            collect_expr(&s.target, out);
            collect_args(&s.arguments, out);
        }
        Expression::AssignmentOperation(a) => {
            collect_expr(a.left(), out);
            collect_expr(a.right(), out);
        }
        Expression::Ternary(t) => {
            collect_expr(&t.condition, out);
            collect_expr(&t.then, out);
            collect_expr(&t.r#else, out);
        }
        Expression::ShortTernary(t) => {
            collect_expr(&t.condition, out);
            collect_expr(&t.r#else, out);
        }
        Expression::Coalesce(c) => {
            collect_expr(&c.lhs, out);
            collect_expr(&c.rhs, out);
        }
        Expression::Parenthesized(p) => collect_expr(&p.expr, out),
        Expression::Closure(c) => {
            for p in c.parameters.parameters.iter() {
                let pname = p.name.name.to_string();
                push_span(out, &p.name.span, pname.len() as u32, TT_PARAMETER, MOD_DECLARATION);
            }
            collect_stmts(&c.body.statements, out);
        }
        Expression::ArrowFunction(a) => collect_expr(&a.body, out),
        Expression::Concat(c) => {
            collect_expr(&c.left, out);
            collect_expr(&c.right, out);
        }
        _ => {}
    }
}

fn collect_args(args: &php_parser_rs::parser::ast::arguments::ArgumentList, out: &mut Vec<RawToken>) {
    use php_parser_rs::parser::ast::arguments::Argument;
    for arg in &args.arguments {
        match arg {
            Argument::Positional(p) => collect_expr(&p.value, out),
            Argument::Named(n) => collect_expr(&n.value, out),
        }
    }
}

fn delta_encode(raw: Vec<RawToken>) -> Vec<SemanticToken> {
    let mut result = Vec::with_capacity(raw.len());
    let (mut prev_line, mut prev_start) = (0u32, 0u32);

    for (line, col, len, token_type, modifiers) in raw {
        let delta_line = line - prev_line;
        let delta_start = if delta_line == 0 { col - prev_start } else { col };
        result.push(SemanticToken {
            delta_line,
            delta_start,
            length: len,
            token_type,
            token_modifiers_bitset: modifiers,
        });
        prev_line = line;
        prev_start = col;
    }

    result
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

    #[test]
    fn empty_file_produces_no_tokens() {
        let ast = parse_ast("<?php");
        assert!(semantic_tokens(&ast).is_empty());
    }

    #[test]
    fn function_declaration_emits_function_token_with_declaration_modifier() {
        let ast = parse_ast("<?php\nfunction greet() {}");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_FUNCTION && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected function+declaration token, got {:?}", tokens
        );
    }

    #[test]
    fn class_declaration_emits_class_token() {
        let ast = parse_ast("<?php\nclass Foo {}");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_CLASS && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected class+declaration token"
        );
    }

    #[test]
    fn interface_declaration_emits_interface_token() {
        let ast = parse_ast("<?php\ninterface Bar {}");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_INTERFACE && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected interface+declaration token"
        );
    }

    #[test]
    fn method_declaration_emits_method_token() {
        let ast = parse_ast("<?php\nclass Foo { public function run() {} }");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_METHOD && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected method+declaration token"
        );
    }

    #[test]
    fn abstract_method_has_abstract_modifier() {
        let ast = parse_ast("<?php\nabstract class Base { abstract public function doIt(): void; }");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_METHOD && t.token_modifiers_bitset & MOD_ABSTRACT != 0),
            "expected abstract method token"
        );
    }

    #[test]
    fn static_method_has_static_modifier() {
        let ast = parse_ast("<?php\nclass Foo { public static function build() {} }");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_METHOD && t.token_modifiers_bitset & MOD_STATIC != 0),
            "expected static method token"
        );
    }

    #[test]
    fn parameter_emits_parameter_token() {
        let ast = parse_ast("<?php\nfunction greet(string $name) {}");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_PARAMETER && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected parameter+declaration token"
        );
    }

    #[test]
    fn function_call_emits_function_token_without_declaration() {
        let ast = parse_ast("<?php\ngreet();");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_FUNCTION && t.token_modifiers_bitset & MOD_DECLARATION == 0),
            "expected function call token (no declaration modifier)"
        );
    }

    #[test]
    fn method_call_emits_method_token_without_declaration() {
        let ast = parse_ast("<?php\n$obj->run();");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_METHOD && t.token_modifiers_bitset & MOD_DECLARATION == 0),
            "expected method call token (no declaration modifier)"
        );
    }

    #[test]
    fn property_emits_property_token() {
        let ast = parse_ast("<?php\nclass Foo { public string $name; }");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_PROPERTY && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected property+declaration token"
        );
    }

    #[test]
    fn tokens_are_delta_encoded_in_order() {
        let ast = parse_ast("<?php\nfunction a() {}\nfunction b() {}");
        let tokens = semantic_tokens(&ast);
        // Reconstruct absolute positions to verify ordering
        let mut line = 0u32;
        let mut col = 0u32;
        let mut positions = Vec::new();
        for t in &tokens {
            line += t.delta_line;
            col = if t.delta_line == 0 { col + t.delta_start } else { t.delta_start };
            positions.push((line, col));
        }
        let sorted = {
            let mut s = positions.clone();
            s.sort();
            s
        };
        assert_eq!(positions, sorted, "tokens must be in ascending (line, col) order");
    }

    #[test]
    fn namespace_contents_are_tokenized() {
        let ast = parse_ast("<?php\nnamespace App;\nfunction boot() {}");
        let tokens = semantic_tokens(&ast);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_FUNCTION),
            "function inside namespace should produce tokens"
        );
    }

    #[test]
    fn legend_has_correct_token_count() {
        let l = legend();
        assert_eq!(l.token_types.len(), 9);
        assert_eq!(l.token_modifiers.len(), 4);
    }
}
