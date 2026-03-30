use std::hash::{Hash, Hasher};

use php_ast::{
    Attribute, ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind,
};
use tower_lsp::lsp_types::{
    Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensEdit,
    SemanticTokensLegend,
};

use crate::ast::{ParsedDoc, offset_to_position, str_offset};
use crate::docblock::{docblock_before, parse_docblock};

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
const MOD_DEPRECATED: u32 = 1 << 4;

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
            SemanticTokenModifier::DEPRECATED,
        ],
    }
}

pub fn semantic_tokens(source: &str, doc: &ParsedDoc) -> Vec<SemanticToken> {
    let mut raw: Vec<RawToken> = Vec::new();
    collect_stmts(source, &doc.program().stmts, &mut raw);
    raw.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    delta_encode(raw)
}

/// Return semantic tokens restricted to the given source range.
/// Useful for editors that only request tokens for the visible viewport.
pub fn semantic_tokens_range(source: &str, doc: &ParsedDoc, range: Range) -> Vec<SemanticToken> {
    let mut raw: Vec<RawToken> = Vec::new();
    collect_stmts(source, &doc.program().stmts, &mut raw);
    raw.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let filtered: Vec<RawToken> = raw
        .into_iter()
        .filter(|(line, col, _len, _, _)| {
            let after_start = *line > range.start.line
                || (*line == range.start.line && *col >= range.start.character);
            let before_end =
                *line < range.end.line || (*line == range.end.line && *col < range.end.character);
            after_start && before_end
        })
        .collect();

    delta_encode(filtered)
}

/// Stable hash of a token list, used as a `result_id` for delta requests.
/// Identical token sequences always produce the same string.
pub fn token_hash(tokens: &[SemanticToken]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for t in tokens {
        t.delta_line.hash(&mut hasher);
        t.delta_start.hash(&mut hasher);
        t.length.hash(&mut hasher);
        t.token_type.hash(&mut hasher);
        t.token_modifiers_bitset.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

/// Compute the minimal single-span edit that transforms `old` into `new`.
/// Returns an empty vec when the sequences are identical.
pub fn compute_token_delta(
    old: &[SemanticToken],
    new: &[SemanticToken],
) -> Vec<SemanticTokensEdit> {
    let eq = |a: &SemanticToken, b: &SemanticToken| {
        a.delta_line == b.delta_line
            && a.delta_start == b.delta_start
            && a.length == b.length
            && a.token_type == b.token_type
            && a.token_modifiers_bitset == b.token_modifiers_bitset
    };

    // First differing token index
    let first = old
        .iter()
        .zip(new.iter())
        .position(|(a, b)| !eq(a, b))
        .unwrap_or(old.len().min(new.len()));

    if first == old.len() && first == new.len() {
        return vec![];
    }

    // Trim common suffix (working from the ends of each slice past `first`)
    let trim = old[first..]
        .iter()
        .rev()
        .zip(new[first..].iter().rev())
        .take_while(|(a, b)| eq(a, b))
        .count();

    let old_end = old.len() - trim; // exclusive index in `old`
    let new_end = new.len() - trim; // exclusive index in `new`

    // Indices in the *flat* u32 array (5 u32s per SemanticToken)
    let start = (first * 5) as u32;
    let delete_count = ((old_end - first) * 5) as u32;
    let insert: Vec<SemanticToken> = new[first..new_end].to_vec();

    vec![SemanticTokensEdit {
        start,
        delete_count,
        data: if insert.is_empty() {
            None
        } else {
            Some(insert)
        },
    }]
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
        name.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
        token_type,
        modifiers,
    );
}

fn push_attributes(out: &mut Vec<RawToken>, source: &str, attrs: &[Attribute<'_, '_>]) {
    for attr in attrs.iter() {
        let span = attr.name.span();
        let len = span.end - span.start;
        push_at(out, source, span.start, len, TT_CLASS, 0);
    }
}

fn deprecated_mod(source: &str, node_start: u32) -> u32 {
    docblock_before(source, node_start)
        .map(|raw| {
            if parse_docblock(&raw).is_deprecated() {
                MOD_DEPRECATED
            } else {
                0
            }
        })
        .unwrap_or(0)
}

fn collect_stmts(source: &str, stmts: &[Stmt<'_, '_>], out: &mut Vec<RawToken>) {
    for stmt in stmts {
        collect_stmt(source, stmt, out);
    }
}

fn collect_stmt(source: &str, stmt: &Stmt<'_, '_>, out: &mut Vec<RawToken>) {
    match &stmt.kind {
        StmtKind::Function(f) => {
            push_attributes(out, source, &f.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(source, stmt.span.start);
            push_name(out, source, f.name, TT_FUNCTION, mods);
            for p in f.params.iter() {
                push_attributes(out, source, &p.attributes);
                push_name(out, source, p.name, TT_PARAMETER, MOD_DECLARATION);
            }
            collect_stmts(source, &f.body, out);
        }
        StmtKind::Class(c) => {
            push_attributes(out, source, &c.attributes);
            if let Some(name) = c.name {
                let mods = MOD_DECLARATION | deprecated_mod(source, stmt.span.start);
                push_name(out, source, name, TT_CLASS, mods);
            }
            for member in c.members.iter() {
                collect_class_member(source, member, out);
            }
        }
        StmtKind::Interface(i) => {
            push_attributes(out, source, &i.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(source, stmt.span.start);
            push_name(out, source, i.name, TT_INTERFACE, mods);
        }
        StmtKind::Trait(t) => {
            push_attributes(out, source, &t.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(source, stmt.span.start);
            push_name(out, source, t.name, TT_CLASS, mods);
            for member in t.members.iter() {
                collect_class_member(source, member, out);
            }
        }
        StmtKind::Enum(e) => {
            push_attributes(out, source, &e.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(source, stmt.span.start);
            push_name(out, source, e.name, TT_CLASS, mods);
            for member in e.members.iter() {
                if let EnumMemberKind::Method(m) = &member.kind {
                    push_attributes(out, source, &m.attributes);
                    let mut mmods = MOD_DECLARATION | deprecated_mod(source, member.span.start);
                    if m.is_static {
                        mmods |= MOD_STATIC;
                    }
                    push_name(out, source, m.name, TT_METHOD, mmods);
                    for p in m.params.iter() {
                        push_name(out, source, p.name, TT_PARAMETER, MOD_DECLARATION);
                    }
                    if let Some(body) = &m.body {
                        collect_stmts(source, body, out);
                    }
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                collect_stmts(source, inner, out);
            }
        }
        StmtKind::Expression(e) => collect_expr(source, e, out),
        StmtKind::Return(Some(v)) => collect_expr(source, v, out),
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
            for e in f.init.iter() {
                collect_expr(source, e, out);
            }
            for cond in f.condition.iter() {
                collect_expr(source, cond, out);
            }
            for e in f.update.iter() {
                collect_expr(source, e, out);
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
        push_attributes(out, source, &m.attributes);
        let mut mods = MOD_DECLARATION | deprecated_mod(source, member.span.start);
        if m.is_static {
            mods |= MOD_STATIC;
        }
        if m.is_abstract {
            mods |= MOD_ABSTRACT;
        }
        push_name(out, source, m.name, TT_METHOD, mods);
        for p in m.params.iter() {
            push_attributes(out, source, &p.attributes);
            push_name(out, source, p.name, TT_PARAMETER, MOD_DECLARATION);
        }
        if let Some(body) = &m.body {
            collect_stmts(source, body, out);
        }
    } else if let ClassMemberKind::Property(p) = &member.kind {
        push_attributes(out, source, &p.attributes);
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
                    name_str.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
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
                    name_str.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
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
                    name_str.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
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
        assert_eq!(l.token_modifiers.len(), 5);
    }

    #[test]
    fn deprecated_function_has_deprecated_modifier() {
        let src = "<?php\n/** @deprecated Use newFn() instead */\nfunction oldFn() {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_FUNCTION
                    && t.token_modifiers_bitset & MOD_DEPRECATED != 0),
            "expected deprecated modifier on function, got {:?}",
            tokens
        );
    }

    #[test]
    fn deprecated_method_has_deprecated_modifier() {
        let src =
            "<?php\nclass Foo {\n    /** @deprecated */\n    public function oldMethod() {}\n}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(
                |t| t.token_type == TT_METHOD && t.token_modifiers_bitset & MOD_DEPRECATED != 0
            ),
            "expected deprecated modifier on method, got {:?}",
            tokens
        );
    }

    #[test]
    fn non_deprecated_function_has_no_deprecated_modifier() {
        let src = "<?php\n/** Just a regular function */\nfunction goodFn() {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .all(|t| t.token_modifiers_bitset & MOD_DEPRECATED == 0),
            "expected no deprecated modifier on non-deprecated function"
        );
    }

    #[test]
    fn enum_declaration_emits_class_token() {
        let src = "<?php\nenum Suit { case Hearts; }";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(
                |t| t.token_type == TT_CLASS && t.token_modifiers_bitset & MOD_DECLARATION != 0
            ),
            "expected class+declaration token for enum"
        );
    }

    #[test]
    fn attribute_name_emits_class_token() {
        let src = "<?php\n#[Route(\"/home\")]\nfunction index() {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens
                .iter()
                .any(|t| t.token_type == TT_CLASS && t.token_modifiers_bitset == 0),
            "expected bare class token for attribute name, got {:?}",
            tokens
        );
    }

    #[test]
    fn for_loop_init_and_update_expressions_are_tokenized() {
        let src = "<?php\nfunction init(): int { return 0; }\nfunction step(): void {}\nfor ($i = init(); $i < 10; step()) {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        // Should produce function+declaration tokens for init() and step() plus
        // function reference tokens for the calls in the for loop.
        let func_tokens: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TT_FUNCTION)
            .collect();
        assert!(
            func_tokens.len() >= 4,
            "expected tokens for init decl, step decl, init() call, step() call; got {} function tokens",
            func_tokens.len()
        );
    }
}
