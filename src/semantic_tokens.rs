use php_ast::{ClassMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend,
};

use crate::ast::{ParsedDoc, offset_to_position, str_offset};

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

pub fn semantic_tokens(source: &str, doc: &ParsedDoc) -> Vec<SemanticToken> {
    let mut raw: Vec<RawToken> = Vec::new();
    collect_stmts(source, &doc.program().stmts, &mut raw);
    raw.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    delta_encode(raw)
}

fn push_at(
    out: &mut Vec<RawToken>,
    source: &str,
    offset: u32,
    len: u32,
    token_type: u32,
    modifiers: u32,
) {
    let pos = offset_to_position(source, offset);
    out.push((pos.line, pos.character, len, token_type, modifiers));
}

fn push_name(out: &mut Vec<RawToken>, source: &str, name: &str, token_type: u32, modifiers: u32) {
    let offset = str_offset(source, name);
    push_at(
        out,
        source,
        offset,
        name.len() as u32,
        token_type,
        modifiers,
    );
}

fn collect_stmts(source: &str, stmts: &[Stmt<'_, '_>], out: &mut Vec<RawToken>) {
    for stmt in stmts {
        collect_stmt(source, stmt, out);
    }
}

fn collect_stmt(source: &str, stmt: &Stmt<'_, '_>, out: &mut Vec<RawToken>) {
    match &stmt.kind {
        StmtKind::Function(f) => {
            push_name(out, source, f.name, TT_FUNCTION, MOD_DECLARATION);
            for p in f.params.iter() {
                push_name(out, source, p.name, TT_PARAMETER, MOD_DECLARATION);
            }
            collect_stmts(source, &f.body, out);
        }
        StmtKind::Class(c) => {
            if let Some(name) = c.name {
                push_name(out, source, name, TT_CLASS, MOD_DECLARATION);
            }
            for member in c.members.iter() {
                collect_class_member(source, member, out);
            }
        }
        StmtKind::Interface(i) => {
            push_name(out, source, i.name, TT_INTERFACE, MOD_DECLARATION);
        }
        StmtKind::Trait(t) => {
            push_name(out, source, t.name, TT_CLASS, MOD_DECLARATION);
            for member in t.members.iter() {
                collect_class_member(source, member, out);
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                collect_stmts(source, inner, out);
            }
        }
        StmtKind::Expression(e) => collect_expr(source, e, out),
        StmtKind::Return(r) => {
            if let Some(v) = r {
                collect_expr(source, v, out);
            }
        }
        StmtKind::Echo(exprs) => {
            for expr in exprs.iter() {
                collect_expr(source, expr, out);
            }
        }
        StmtKind::If(i) => {
            collect_expr(source, &i.condition, out);
            collect_stmt(source, i.then_branch, out);
            for ei in i.elseif_branches.iter() {
                collect_expr(source, &ei.condition, out);
                collect_stmt(source, &ei.body, out);
            }
            if let Some(e) = &i.else_branch {
                collect_stmt(source, e, out);
            }
        }
        StmtKind::While(w) => {
            collect_expr(source, &w.condition, out);
            collect_stmt(source, w.body, out);
        }
        StmtKind::For(f) => {
            for cond in f.condition.iter() {
                collect_expr(source, cond, out);
            }
            collect_stmt(source, f.body, out);
        }
        StmtKind::Foreach(f) => {
            collect_expr(source, &f.expr, out);
            collect_stmt(source, f.body, out);
        }
        StmtKind::TryCatch(t) => {
            collect_stmts(source, &t.body, out);
            for catch in t.catches.iter() {
                collect_stmts(source, &catch.body, out);
            }
            if let Some(finally) = &t.finally {
                collect_stmts(source, finally, out);
            }
        }
        StmtKind::Block(stmts) => collect_stmts(source, stmts, out),
        _ => {}
    }
}

fn collect_class_member(
    source: &str,
    member: &php_ast::ClassMember<'_, '_>,
    out: &mut Vec<RawToken>,
) {
    if let ClassMemberKind::Method(m) = &member.kind {
        let mut mods = MOD_DECLARATION;
        if m.is_static {
            mods |= MOD_STATIC;
        }
        if m.is_abstract {
            mods |= MOD_ABSTRACT;
        }
        push_name(out, source, m.name, TT_METHOD, mods);
        for p in m.params.iter() {
            push_name(out, source, p.name, TT_PARAMETER, MOD_DECLARATION);
        }
        if let Some(body) = &m.body {
            collect_stmts(source, body, out);
        }
    } else if let ClassMemberKind::Property(p) = &member.kind {
        push_name(out, source, p.name, TT_PROPERTY, MOD_DECLARATION);
    }
}

fn collect_expr(source: &str, expr: &php_ast::Expr<'_, '_>, out: &mut Vec<RawToken>) {
    match &expr.kind {
        ExprKind::FunctionCall(f) => {
            if let ExprKind::Identifier(name) = &f.name.kind {
                let name_str = name.as_ref();
                push_at(
                    out,
                    source,
                    f.name.span.start,
                    name_str.len() as u32,
                    TT_FUNCTION,
                    0,
                );
            } else {
                collect_expr(source, f.name, out);
            }
            for arg in f.args.iter() {
                collect_expr(source, &arg.value, out);
            }
        }
        ExprKind::MethodCall(m) => {
            collect_expr(source, m.object, out);
            if let ExprKind::Identifier(name) = &m.method.kind {
                let name_str = name.as_ref();
                push_at(
                    out,
                    source,
                    m.method.span.start,
                    name_str.len() as u32,
                    TT_METHOD,
                    0,
                );
            }
            for arg in m.args.iter() {
                collect_expr(source, &arg.value, out);
            }
        }
        ExprKind::NullsafeMethodCall(m) => {
            collect_expr(source, m.object, out);
            if let ExprKind::Identifier(name) = &m.method.kind {
                let name_str = name.as_ref();
                push_at(
                    out,
                    source,
                    m.method.span.start,
                    name_str.len() as u32,
                    TT_METHOD,
                    0,
                );
            }
            for arg in m.args.iter() {
                collect_expr(source, &arg.value, out);
            }
        }
        ExprKind::Assign(a) => {
            collect_expr(source, a.target, out);
            collect_expr(source, a.value, out);
        }
        ExprKind::Ternary(t) => {
            collect_expr(source, t.condition, out);
            if let Some(then_expr) = t.then_expr {
                collect_expr(source, then_expr, out);
            }
            collect_expr(source, t.else_expr, out);
        }
        ExprKind::NullCoalesce(n) => {
            collect_expr(source, n.left, out);
            collect_expr(source, n.right, out);
        }
        ExprKind::Binary(b) => {
            collect_expr(source, b.left, out);
            collect_expr(source, b.right, out);
        }
        ExprKind::Parenthesized(e) => collect_expr(source, e, out),
        _ => {}
    }
}

fn delta_encode(raw: Vec<RawToken>) -> Vec<SemanticToken> {
    let mut result = Vec::with_capacity(raw.len());
    let (mut prev_line, mut prev_start) = (0u32, 0u32);

    for (line, col, len, token_type, modifiers) in raw {
        let delta_line = line - prev_line;
        let delta_start = if delta_line == 0 {
            col - prev_start
        } else {
            col
        };
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

    fn doc(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    #[test]
    fn empty_file_produces_no_tokens() {
        let src = "<?php";
        let d = doc(src);
        assert!(semantic_tokens(src, &d).is_empty());
    }

    #[test]
    fn function_declaration_emits_function_token_with_declaration_modifier() {
        let src = "<?php\nfunction greet() {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_FUNCTION
                    && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected function+declaration token, got {:?}",
            tokens
        );
    }

    #[test]
    fn class_declaration_emits_class_token() {
        let src = "<?php\nclass Foo {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(
                |t| t.token_type == TT_CLASS && t.token_modifiers_bitset & MOD_DECLARATION != 0
            ),
            "expected class+declaration token"
        );
    }

    #[test]
    fn interface_declaration_emits_interface_token() {
        let src = "<?php\ninterface Bar {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_INTERFACE
                    && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected interface+declaration token"
        );
    }

    #[test]
    fn method_declaration_emits_method_token() {
        let src = "<?php\nclass Foo { public function run() {} }";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_METHOD
                    && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected method+declaration token"
        );
    }

    #[test]
    fn abstract_method_has_abstract_modifier() {
        let src = "<?php\nabstract class Base { abstract public function doIt(): void; }";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_METHOD && t.token_modifiers_bitset & MOD_ABSTRACT != 0),
            "expected abstract method token"
        );
    }

    #[test]
    fn static_method_has_static_modifier() {
        let src = "<?php\nclass Foo { public static function build() {} }";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_METHOD && t.token_modifiers_bitset & MOD_STATIC != 0),
            "expected static method token"
        );
    }

    #[test]
    fn parameter_emits_parameter_token() {
        let src = "<?php\nfunction greet(string $name) {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_PARAMETER
                    && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected parameter+declaration token"
        );
    }

    #[test]
    fn function_call_emits_function_token_without_declaration() {
        let src = "<?php\ngreet();";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_FUNCTION
                    && t.token_modifiers_bitset & MOD_DECLARATION == 0),
            "expected function call token (no declaration modifier)"
        );
    }

    #[test]
    fn method_call_emits_method_token_without_declaration() {
        let src = "<?php\n$obj->run();";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_METHOD
                    && t.token_modifiers_bitset & MOD_DECLARATION == 0),
            "expected method call token (no declaration modifier)"
        );
    }

    #[test]
    fn property_emits_property_token() {
        let src = "<?php\nclass Foo { public string $name; }";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_PROPERTY
                    && t.token_modifiers_bitset & MOD_DECLARATION != 0),
            "expected property+declaration token"
        );
    }

    #[test]
    fn tokens_are_delta_encoded_in_order() {
        let src = "<?php\nfunction a() {}\nfunction b() {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        let mut line = 0u32;
        let mut col = 0u32;
        let mut positions = Vec::new();
        for t in &tokens {
            line += t.delta_line;
            col = if t.delta_line == 0 {
                col + t.delta_start
            } else {
                t.delta_start
            };
            positions.push((line, col));
        }
        let sorted = {
            let mut s = positions.clone();
            s.sort();
            s
        };
        assert_eq!(
            positions, sorted,
            "tokens must be in ascending (line, col) order"
        );
    }

    #[test]
    fn namespace_contents_are_tokenized() {
        let src = "<?php\nnamespace App;\nfunction boot() {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
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
