use std::hash::{Hash, Hasher};

use php_ast::{
    Attribute, ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind, TypeHint,
    TypeHintKind,
};
use tower_lsp::lsp_types::{
    Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensEdit,
    SemanticTokensLegend,
};

use crate::ast::{ParsedDoc, SourceView, str_offset};
use crate::docblock::{docblock_before, parse_docblock};

// Token type indices — order must match `legend()` vec order
const _TT_NAMESPACE: u32 = 0;
const TT_CLASS: u32 = 1;
const TT_INTERFACE: u32 = 2;
const TT_FUNCTION: u32 = 3;
const TT_METHOD: u32 = 4;
const TT_PROPERTY: u32 = 5;
const TT_VARIABLE: u32 = 6;
const TT_PARAMETER: u32 = 7;
const TT_TYPE: u32 = 8;
const TT_STRING: u32 = 9;
const TT_NUMBER: u32 = 10;
const TT_COMMENT: u32 = 11;

// Modifier bits — order must match `legend()` modifier vec order
const MOD_DECLARATION: u32 = 1 << 0;
const MOD_STATIC: u32 = 1 << 1;
const MOD_ABSTRACT: u32 = 1 << 2;
const _MOD_READONLY: u32 = 1 << 3;
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
            SemanticTokenType::STRING,
            SemanticTokenType::NUMBER,
            SemanticTokenType::COMMENT,
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

pub fn semantic_tokens(_source: &str, doc: &ParsedDoc) -> Vec<SemanticToken> {
    let sv = doc.view();
    let mut raw: Vec<RawToken> = Vec::new();
    collect_comments(sv, &mut raw);
    collect_stmts(sv, &doc.program().stmts, &mut raw);
    raw.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    delta_encode(raw)
}

/// Return semantic tokens restricted to the given sv.source() range.
/// Useful for editors that only request tokens for the visible viewport.
pub fn semantic_tokens_range(_source: &str, doc: &ParsedDoc, range: Range) -> Vec<SemanticToken> {
    let sv = doc.view();
    let mut raw: Vec<RawToken> = Vec::new();
    collect_comments(sv, &mut raw);
    collect_stmts(sv, &doc.program().stmts, &mut raw);
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
    sv: SourceView<'_>,
    offset: u32,
    len: u32,
    token_type: u32,
    modifiers: u32,
) {
    let pos = sv.position_of(offset);
    out.push((pos.line, pos.character, len, token_type, modifiers));
}

fn push_name(
    out: &mut Vec<RawToken>,
    sv: SourceView<'_>,
    name: &str,
    token_type: u32,
    modifiers: u32,
) {
    let offset = str_offset(sv.source(), name);
    push_at(
        out,
        sv,
        offset,
        name.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
        token_type,
        modifiers,
    );
}

/// Like `push_name` but also includes the leading `$` sigil when one immediately
/// precedes the name in the sv.source().  PHP parameter names in the AST are stored
/// without `$`, but the sigil is part of the syntax and must be highlighted
/// consistently with variable-expression tokens which include it.
fn push_param(
    out: &mut Vec<RawToken>,
    sv: SourceView<'_>,
    name: &str,
    token_type: u32,
    modifiers: u32,
) {
    let name_offset = str_offset(sv.source(), name);
    let (offset, extra_len) =
        if name_offset > 0 && sv.source().as_bytes().get(name_offset as usize - 1) == Some(&b'$') {
            (name_offset - 1, 1u32)
        } else {
            (name_offset, 0u32)
        };
    push_at(
        out,
        sv,
        offset,
        extra_len + name.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
        token_type,
        modifiers,
    );
}

fn push_attributes(out: &mut Vec<RawToken>, sv: SourceView<'_>, attrs: &[Attribute<'_, '_>]) {
    for attr in attrs.iter() {
        let span = attr.name.span();
        let segment = &sv.source()[span.start as usize..span.end as usize];
        let len: u32 = segment.chars().map(|c| c.len_utf16() as u32).sum();
        push_at(out, sv, span.start, len, TT_CLASS, 0);
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

/// Scan `sv.source()` for PHP comments (single-line `//` and `#`, multi-line `/* */`)
/// and emit `TT_COMMENT` tokens.  Each physical line of a multi-line comment
/// is emitted as a separate token because the LSP protocol requires tokens to
/// fit on a single line.
fn collect_comments(sv: SourceView<'_>, out: &mut Vec<RawToken>) {
    let bytes = sv.source().as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    // Track whether we are inside a string literal so we do not mistake
    // `//` or `/*` inside strings for comments.  We only do a best-effort
    // scan; the AST handles string contents properly.
    while i < len {
        match bytes[i] {
            // Skip double-quoted strings
            b'"' => {
                i += 1;
                while i < len {
                    if bytes[i] == b'\\' {
                        i += 2;
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            // Skip single-quoted strings
            b'\'' => {
                i += 1;
                while i < len {
                    if bytes[i] == b'\\' {
                        i += 2;
                    } else if bytes[i] == b'\'' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'/' if i + 1 < len => {
                if bytes[i + 1] == b'/' {
                    // Single-line comment: `// ...` up to (but not including) newline
                    let start = i;
                    while i < len && bytes[i] != b'\n' {
                        i += 1;
                    }
                    let text = &sv.source()[start..i];
                    let len_utf16: u32 = text.chars().map(|c| c.len_utf16() as u32).sum();
                    push_at(out, sv, start as u32, len_utf16, TT_COMMENT, 0);
                } else if bytes[i + 1] == b'*' {
                    // Multi-line comment: `/* ... */`
                    let start = i;
                    i += 2;
                    while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                        i += 1;
                    }
                    // consume the closing `*/`
                    if i + 1 < len {
                        i += 2;
                    }
                    // Emit per-line so the LSP single-line constraint is met
                    emit_multiline_comment(sv, start, i, out);
                } else {
                    i += 1;
                }
            }
            // Single-line comment starting with `#` (also `#[` is an attribute,
            // but `#` not followed by `[` is a comment in PHP)
            b'#' if i + 1 < len && bytes[i + 1] != b'[' => {
                let start = i;
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                let text = &sv.source()[start..i];
                let len_utf16: u32 = text.chars().map(|c| c.len_utf16() as u32).sum();
                push_at(out, sv, start as u32, len_utf16, TT_COMMENT, 0);
            }
            _ => {
                i += 1;
            }
        }
    }
}

/// Emit a `TT_COMMENT` raw token for each line within a block comment
/// `sv.source()[start..end]`.  Multi-line tokens are not allowed by the LSP spec.
fn emit_multiline_comment(sv: SourceView<'_>, start: usize, end: usize, out: &mut Vec<RawToken>) {
    let text = &sv.source()[start..end];
    let mut line_start = start;
    for (rel, ch) in text.char_indices() {
        if ch == '\n' {
            let line_end = start + rel; // byte index of newline
            if line_end > line_start {
                let segment = &sv.source()[line_start..line_end];
                let len_utf16: u32 = segment.chars().map(|c| c.len_utf16() as u32).sum();
                if len_utf16 > 0 {
                    push_at(out, sv, line_start as u32, len_utf16, TT_COMMENT, 0);
                }
            }
            line_start = start + rel + 1; // byte after '\n'
        }
    }
    // Last (or only) line
    if line_start < end {
        let segment = &sv.source()[line_start..end];
        let len_utf16: u32 = segment.chars().map(|c| c.len_utf16() as u32).sum();
        if len_utf16 > 0 {
            push_at(out, sv, line_start as u32, len_utf16, TT_COMMENT, 0);
        }
    }
}

/// Emit `TT_TYPE` tokens for each atomic component of a type hint.
/// Named types (e.g. `Foo`, `\Bar\Baz`) get one token at the name span.
/// Keyword types (e.g. `int`, `string`, `void`, `null`) get one token at
/// the keyword span.  Union/intersection/nullable are recursed into so
/// every component is covered.
fn push_type_hint(out: &mut Vec<RawToken>, sv: SourceView<'_>, hint: &TypeHint<'_, '_>) {
    match &hint.kind {
        TypeHintKind::Named(name) => {
            let span = name.span();
            let segment = &sv.source()[span.start as usize..span.end as usize];
            let len: u32 = segment.chars().map(|c| c.len_utf16() as u32).sum();
            push_at(out, sv, span.start, len, TT_TYPE, 0);
        }
        TypeHintKind::Keyword(builtin, span) => {
            let text = builtin.as_str();
            let len_utf16: u32 = text.chars().map(|c| c.len_utf16() as u32).sum();
            push_at(out, sv, span.start, len_utf16, TT_TYPE, 0);
        }
        TypeHintKind::Nullable(inner) => {
            push_type_hint(out, sv, inner);
        }
        TypeHintKind::Union(types) | TypeHintKind::Intersection(types) => {
            for t in types.iter() {
                push_type_hint(out, sv, t);
            }
        }
    }
}

fn collect_stmts(sv: SourceView<'_>, stmts: &[Stmt<'_, '_>], out: &mut Vec<RawToken>) {
    for stmt in stmts {
        collect_stmt(sv, stmt, out);
    }
}

fn collect_stmt(sv: SourceView<'_>, stmt: &Stmt<'_, '_>, out: &mut Vec<RawToken>) {
    match &stmt.kind {
        StmtKind::Function(f) => {
            push_attributes(out, sv, &f.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(sv.source(), stmt.span.start);
            push_name(out, sv, f.name, TT_FUNCTION, mods);
            for p in f.params.iter() {
                push_attributes(out, sv, &p.attributes);
                if let Some(th) = &p.type_hint {
                    push_type_hint(out, sv, th);
                }
                push_param(out, sv, p.name, TT_PARAMETER, MOD_DECLARATION);
            }
            if let Some(rt) = &f.return_type {
                push_type_hint(out, sv, rt);
            }
            collect_stmts(sv, &f.body, out);
        }
        StmtKind::Class(c) => {
            push_attributes(out, sv, &c.attributes);
            if let Some(name) = c.name {
                let mods = MOD_DECLARATION | deprecated_mod(sv.source(), stmt.span.start);
                push_name(out, sv, name, TT_CLASS, mods);
            }
            for member in c.members.iter() {
                collect_class_member(sv, member, out);
            }
        }
        StmtKind::Interface(i) => {
            push_attributes(out, sv, &i.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(sv.source(), stmt.span.start);
            push_name(out, sv, i.name, TT_INTERFACE, mods);
        }
        StmtKind::Trait(t) => {
            push_attributes(out, sv, &t.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(sv.source(), stmt.span.start);
            push_name(out, sv, t.name, TT_CLASS, mods);
            for member in t.members.iter() {
                collect_class_member(sv, member, out);
            }
        }
        StmtKind::Enum(e) => {
            push_attributes(out, sv, &e.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(sv.source(), stmt.span.start);
            push_name(out, sv, e.name, TT_CLASS, mods);
            for member in e.members.iter() {
                if let EnumMemberKind::Method(m) = &member.kind {
                    push_attributes(out, sv, &m.attributes);
                    let mut mmods =
                        MOD_DECLARATION | deprecated_mod(sv.source(), member.span.start);
                    if m.is_static {
                        mmods |= MOD_STATIC;
                    }
                    push_name(out, sv, m.name, TT_METHOD, mmods);
                    for p in m.params.iter() {
                        if let Some(th) = &p.type_hint {
                            push_type_hint(out, sv, th);
                        }
                        push_param(out, sv, p.name, TT_PARAMETER, MOD_DECLARATION);
                    }
                    if let Some(rt) = &m.return_type {
                        push_type_hint(out, sv, rt);
                    }
                    if let Some(body) = &m.body {
                        collect_stmts(sv, body, out);
                    }
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                collect_stmts(sv, inner, out);
            }
        }
        StmtKind::Use(_) => {}
        StmtKind::Expression(e) => collect_expr(sv, e, out),
        StmtKind::Return(Some(expr)) => collect_expr(sv, expr, out),
        StmtKind::Return(None) => {}
        StmtKind::Echo(exprs) => {
            for expr in exprs.iter() {
                collect_expr(sv, expr, out);
            }
        }
        StmtKind::If(i) => {
            collect_expr(sv, &i.condition, out);
            collect_stmt(sv, i.then_branch, out);
            for ei in i.elseif_branches.iter() {
                collect_expr(sv, &ei.condition, out);
                collect_stmt(sv, &ei.body, out);
            }
            if let Some(e) = &i.else_branch {
                collect_stmt(sv, e, out);
            }
        }
        StmtKind::While(w) => {
            collect_expr(sv, &w.condition, out);
            collect_stmt(sv, w.body, out);
        }
        StmtKind::For(f) => {
            for e in f.init.iter() {
                collect_expr(sv, e, out);
            }
            for cond in f.condition.iter() {
                collect_expr(sv, cond, out);
            }
            for e in f.update.iter() {
                collect_expr(sv, e, out);
            }
            collect_stmt(sv, f.body, out);
        }
        StmtKind::Foreach(f) => {
            collect_expr(sv, &f.expr, out);
            collect_stmt(sv, f.body, out);
        }
        StmtKind::TryCatch(t) => {
            collect_stmts(sv, &t.body, out);
            for catch in t.catches.iter() {
                collect_stmts(sv, &catch.body, out);
            }
            if let Some(finally) = &t.finally {
                collect_stmts(sv, finally, out);
            }
        }
        StmtKind::Block(stmts) => collect_stmts(sv, stmts, out),
        _ => {}
    }
}

fn collect_class_member(
    sv: SourceView<'_>,
    member: &php_ast::ClassMember<'_, '_>,
    out: &mut Vec<RawToken>,
) {
    if let ClassMemberKind::Method(m) = &member.kind {
        push_attributes(out, sv, &m.attributes);
        let mut mods = MOD_DECLARATION | deprecated_mod(sv.source(), member.span.start);
        if m.is_static {
            mods |= MOD_STATIC;
        }
        if m.is_abstract {
            mods |= MOD_ABSTRACT;
        }
        push_name(out, sv, m.name, TT_METHOD, mods);
        for p in m.params.iter() {
            push_attributes(out, sv, &p.attributes);
            if let Some(th) = &p.type_hint {
                push_type_hint(out, sv, th);
            }
            push_param(out, sv, p.name, TT_PARAMETER, MOD_DECLARATION);
        }
        if let Some(rt) = &m.return_type {
            push_type_hint(out, sv, rt);
        }
        if let Some(body) = &m.body {
            collect_stmts(sv, body, out);
        }
    } else if let ClassMemberKind::Property(p) = &member.kind {
        push_attributes(out, sv, &p.attributes);
        if let Some(th) = &p.type_hint {
            push_type_hint(out, sv, th);
        }
        push_name(out, sv, p.name, TT_PROPERTY, MOD_DECLARATION);
    }
}

fn collect_expr(sv: SourceView<'_>, expr: &php_ast::Expr<'_, '_>, out: &mut Vec<RawToken>) {
    match &expr.kind {
        // ── Literals ──────────────────────────────────────────────────────────
        ExprKind::Int(_) | ExprKind::Float(_) => {
            let span_len = expr.span.end - expr.span.start;
            push_at(out, sv, expr.span.start, span_len, TT_NUMBER, 0);
        }
        ExprKind::String(_) | ExprKind::Nowdoc { .. } => {
            let segment = &sv.source()[expr.span.start as usize..expr.span.end as usize];
            let len: u32 = segment.chars().map(|c| c.len_utf16() as u32).sum();
            push_at(out, sv, expr.span.start, len, TT_STRING, 0);
        }
        ExprKind::InterpolatedString(parts) | ExprKind::ShellExec(parts) => {
            // Emit the whole span as a string; embedded variables are not
            // re-coloured here to keep the implementation simple.
            let segment = &sv.source()[expr.span.start as usize..expr.span.end as usize];
            let len: u32 = segment.chars().map(|c| c.len_utf16() as u32).sum();
            push_at(out, sv, expr.span.start, len, TT_STRING, 0);
            // Still recurse into embedded expressions so method/function calls
            // inside `"... {$obj->method()} ..."` get proper tokens.
            for part in parts.iter() {
                if let php_ast::StringPart::Expr(inner) = part {
                    collect_expr(sv, inner, out);
                }
            }
        }
        ExprKind::Heredoc { parts, .. } => {
            let segment = &sv.source()[expr.span.start as usize..expr.span.end as usize];
            let len: u32 = segment.chars().map(|c| c.len_utf16() as u32).sum();
            push_at(out, sv, expr.span.start, len, TT_STRING, 0);
            for part in parts.iter() {
                if let php_ast::StringPart::Expr(inner) = part {
                    collect_expr(sv, inner, out);
                }
            }
        }
        ExprKind::New(n) => {
            for arg in n.args.iter() {
                collect_expr(sv, &arg.value, out);
            }
        }
        // ── Calls ─────────────────────────────────────────────────────────────
        ExprKind::FunctionCall(f) => {
            if let ExprKind::Identifier(name) = &f.name.kind {
                let name_str: &str = name;
                push_at(
                    out,
                    sv,
                    f.name.span.start,
                    name_str.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
                    TT_FUNCTION,
                    0,
                );
            } else {
                collect_expr(sv, f.name, out);
            }
            for arg in f.args.iter() {
                collect_expr(sv, &arg.value, out);
            }
        }
        ExprKind::MethodCall(m) => {
            collect_expr(sv, m.object, out);
            if let ExprKind::Identifier(name) = &m.method.kind {
                let name_str: &str = name;
                push_at(
                    out,
                    sv,
                    m.method.span.start,
                    name_str.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
                    TT_METHOD,
                    0,
                );
            }
            for arg in m.args.iter() {
                collect_expr(sv, &arg.value, out);
            }
        }
        ExprKind::NullsafeMethodCall(m) => {
            collect_expr(sv, m.object, out);
            if let ExprKind::Identifier(name) = &m.method.kind {
                let name_str: &str = name;
                push_at(
                    out,
                    sv,
                    m.method.span.start,
                    name_str.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
                    TT_METHOD,
                    0,
                );
            }
            for arg in m.args.iter() {
                collect_expr(sv, &arg.value, out);
            }
        }
        // ── Compound expressions ──────────────────────────────────────────────
        ExprKind::Assign(a) => {
            collect_expr(sv, a.target, out);
            collect_expr(sv, a.value, out);
        }
        ExprKind::Ternary(t) => {
            collect_expr(sv, t.condition, out);
            if let Some(then_expr) = t.then_expr {
                collect_expr(sv, then_expr, out);
            }
            collect_expr(sv, t.else_expr, out);
        }
        ExprKind::NullCoalesce(n) => {
            collect_expr(sv, n.left, out);
            collect_expr(sv, n.right, out);
        }
        ExprKind::Binary(b) => {
            collect_expr(sv, b.left, out);
            collect_expr(sv, b.right, out);
        }
        ExprKind::Parenthesized(e) => collect_expr(sv, e, out),
        ExprKind::Array(elements) => {
            for elem in elements.iter() {
                if let Some(key) = &elem.key {
                    collect_expr(sv, key, out);
                }
                collect_expr(sv, &elem.value, out);
            }
        }
        ExprKind::UnaryPrefix(u) => collect_expr(sv, u.operand, out),
        ExprKind::UnaryPostfix(u) => collect_expr(sv, u.operand, out),
        ExprKind::Closure(c) => {
            for p in c.params.iter() {
                if let Some(th) = &p.type_hint {
                    push_type_hint(out, sv, th);
                }
                push_param(out, sv, p.name, TT_PARAMETER, MOD_DECLARATION);
            }
            if let Some(rt) = &c.return_type {
                push_type_hint(out, sv, rt);
            }
            collect_stmts(sv, &c.body, out);
        }
        ExprKind::ArrowFunction(af) => {
            for p in af.params.iter() {
                if let Some(th) = &p.type_hint {
                    push_type_hint(out, sv, th);
                }
                push_param(out, sv, p.name, TT_PARAMETER, MOD_DECLARATION);
            }
            if let Some(rt) = &af.return_type {
                push_type_hint(out, sv, rt);
            }
            collect_expr(sv, af.body, out);
        }
        // ── Variables ─────────────────────────────────────────────────────────
        ExprKind::Variable(_) => {
            let segment = &sv.source()[expr.span.start as usize..expr.span.end as usize];
            let len: u32 = segment.chars().map(|c| c.len_utf16() as u32).sum();
            push_at(out, sv, expr.span.start, len, TT_VARIABLE, 0);
        }
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
    fn parameter_token_includes_dollar_sign() {
        // Parameter tokens must cover `$name` (including the `$`), not just `name`.
        // Variable-expression tokens already include `$`; parameters must be consistent.
        //
        // Source: "<?php\nfunction greet(string $name) {}"
        // Line 1: "function greet(string $name) {}"
        //          0         1         2         3
        //          0123456789012345678901234567890
        //                                ^ char 22 = '$', char 23 = 'n'
        let src = "<?php\nfunction greet(string $name) {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);

        // Decode delta encoding to find the absolute char of the PARAMETER token.
        let mut abs_char: u32 = 0;
        let mut last_line: u32 = 0;
        let mut param_abs_char: Option<u32> = None;
        let mut param_len: Option<u32> = None;
        for t in &tokens {
            if t.delta_line > 0 {
                abs_char = 0;
                last_line += t.delta_line;
            }
            abs_char += t.delta_start;
            if t.token_type == TT_PARAMETER {
                param_abs_char = Some(abs_char);
                param_len = Some(t.length);
                break;
            }
        }
        let _ = last_line; // suppress unused-variable warning

        let abs = param_abs_char.expect("expected a TT_PARAMETER token");
        // `function greet(string ` = 22 chars → `$` at char 22.
        assert_eq!(
            abs, 22,
            "parameter token must start at `$` (char 22), not at the bare identifier (char 23)"
        );
        // `$name` = 5 chars
        assert_eq!(
            param_len.unwrap(),
            5,
            "parameter token length must cover `$name` (5 chars)"
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
        assert_eq!(l.token_types.len(), 12);
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
    fn string_literal_emits_string_token() {
        let src = "<?php\n$x = \"hello\";";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_STRING),
            "expected string token for double-quoted literal, got {:?}",
            tokens
        );
    }

    #[test]
    fn integer_literal_emits_number_token() {
        let src = "<?php\n$x = 42;";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_NUMBER),
            "expected number token for integer literal, got {:?}",
            tokens
        );
    }

    #[test]
    fn float_literal_emits_number_token() {
        let src = "<?php\n$x = 3.14;";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_NUMBER),
            "expected number token for float literal, got {:?}",
            tokens
        );
    }

    #[test]
    fn single_line_comment_emits_comment_token() {
        let src = "<?php\n// this is a comment\n$x = 1;";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_COMMENT),
            "expected comment token for // comment, got {:?}",
            tokens
        );
    }

    #[test]
    fn multiline_comment_emits_comment_tokens() {
        let src = "<?php\n/* block\n   comment */\n$x = 1;";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_COMMENT),
            "expected comment token for /* */ block, got {:?}",
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

    #[test]
    fn variable_expression_emits_variable_token() {
        let src = "<?php\n$x = 1;\n$y = $x + 2;";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        let var_tokens: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == TT_VARIABLE)
            .collect();
        assert!(
            var_tokens.len() >= 2,
            "expected variable tokens for $x and $y, got {} variable tokens: {:?}",
            var_tokens.len(),
            var_tokens
        );
    }

    #[test]
    fn keyword_type_hint_on_param_emits_type_token() {
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        let type_tokens: Vec<_> = tokens.iter().filter(|t| t.token_type == TT_TYPE).collect();
        assert!(
            type_tokens.len() >= 3,
            "expected TYPE tokens for two 'int' params and 'int' return type, got {} type tokens: {:?}",
            type_tokens.len(),
            type_tokens
        );
    }

    #[test]
    fn named_type_hint_on_param_emits_type_token() {
        let src = "<?php\nfunction greet(string $name, DateTime $when): void {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_TYPE),
            "expected TYPE token for named/keyword type hints, got {:?}",
            tokens
        );
    }

    #[test]
    fn property_type_hint_emits_type_token() {
        let src = "<?php\nclass Foo { public int $count; }";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_TYPE),
            "expected TYPE token for property type hint, got {:?}",
            tokens
        );
    }

    #[test]
    fn method_return_type_emits_type_token() {
        let src = "<?php\nclass Foo { public function get(): string { return ''; } }";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        assert!(
            tokens.iter().any(|t| t.token_type == TT_TYPE),
            "expected TYPE token for method return type, got {:?}",
            tokens
        );
    }

    /// `é` (U+00E9) encodes as 2 UTF-8 bytes but 1 UTF-16 code unit.
    /// A named type hint `Héros` has byte-length 6 but UTF-16 length 5.
    /// Verify the token's `length` matches the UTF-16 count, not the byte count.
    #[test]
    fn named_type_hint_with_multibyte_chars_uses_utf16_length() {
        // "Héros": H(1) é(2) r(1) o(1) s(1) = 6 bytes, 5 UTF-16 units
        let src = "<?php\nfunction greet(Héros $h): void {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        let type_token = tokens
            .iter()
            .find(|t| t.token_type == TT_TYPE)
            .expect("expected a TT_TYPE token for the named type hint");
        let name = "Héros";
        let utf16_len: u32 = name.chars().map(|c| c.len_utf16() as u32).sum();
        let byte_len = name.len() as u32;
        assert_ne!(
            utf16_len, byte_len,
            "test requires a name where UTF-16 ≠ byte length"
        );
        assert_eq!(
            type_token.length, utf16_len,
            "token length should be UTF-16 units ({utf16_len}), not byte length ({byte_len})"
        );
    }

    /// Same check for attribute names: `#[Routé]` has a multibyte name.
    /// `Routé`: R(1) o(1) u(1) t(1) é(2) = 6 bytes, 5 UTF-16 units.
    #[test]
    fn attribute_name_with_multibyte_chars_uses_utf16_length() {
        let src = "<?php\n#[Routé(\"/home\")]\nfunction index() {}";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        let attr_token = tokens
            .iter()
            .find(|t| t.token_type == TT_CLASS && t.token_modifiers_bitset == 0)
            .expect("expected a bare TT_CLASS token for the attribute name");
        let name = "Routé";
        let utf16_len: u32 = name.chars().map(|c| c.len_utf16() as u32).sum();
        let byte_len = name.len() as u32;
        assert_ne!(
            utf16_len, byte_len,
            "test requires a name where UTF-16 ≠ byte length"
        );
        assert_eq!(
            attr_token.length, utf16_len,
            "token length should be UTF-16 units ({utf16_len}), not byte length ({byte_len})"
        );
    }

    /// A string literal `"café"` contains `é` (2 UTF-8 bytes, 1 UTF-16 unit).
    /// The TT_STRING token length must use UTF-16 units, not byte count.
    #[test]
    fn string_literal_with_multibyte_chars_uses_utf16_length() {
        // "café": " + c(1) + a(1) + f(1) + é(2) + " = 8 bytes, 6 UTF-16 units
        let src = "<?php\n$s = \"café\";";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        let str_token = tokens
            .iter()
            .find(|t| t.token_type == TT_STRING)
            .expect("expected a TT_STRING token for the string literal");
        let content = "\"café\"";
        let utf16_len: u32 = content.chars().map(|c| c.len_utf16() as u32).sum();
        let byte_len = content.len() as u32;
        assert_ne!(
            utf16_len, byte_len,
            "test requires content where UTF-16 ≠ byte length"
        );
        assert_eq!(
            str_token.length, utf16_len,
            "token length should be UTF-16 units ({utf16_len}), not byte length ({byte_len})"
        );
    }

    /// A variable whose name contains a multibyte character (valid per PHP spec).
    /// The TT_VARIABLE token length must use UTF-16 units, not byte count.
    #[test]
    fn variable_with_multibyte_name_uses_utf16_length() {
        // $café: $(1) + c(1) + a(1) + f(1) + é(2) = 7 bytes, 6 UTF-16 units
        let src = "<?php\n$café = 1;";
        let d = doc(src);
        let tokens = semantic_tokens(src, &d);
        let var_token = tokens
            .iter()
            .find(|t| t.token_type == TT_VARIABLE)
            .expect("expected a TT_VARIABLE token");
        let name = "$café";
        let utf16_len: u32 = name.chars().map(|c| c.len_utf16() as u32).sum();
        let byte_len = name.len() as u32;
        assert_ne!(
            utf16_len, byte_len,
            "test requires a name where UTF-16 ≠ byte length"
        );
        assert_eq!(
            var_token.length, utf16_len,
            "token length should be UTF-16 units ({utf16_len}), not byte length ({byte_len})"
        );
    }
}
