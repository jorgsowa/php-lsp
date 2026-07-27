use std::hash::{Hash, Hasher};

use php_ast::{
    Attribute, ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind, TypeHint,
    TypeHintKind,
};
use tower_lsp::lsp_types::{
    Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensEdit,
    SemanticTokensLegend,
};

use crate::document::ast::{ParsedDoc, SourceView, str_offset};
use crate::lang::docblock::parse_docblock;
use crate::text::utf16_code_units;

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
const TT_ENUM_MEMBER: u32 = 12;

// Modifier bits — order must match `legend()` modifier vec order
const MOD_DECLARATION: u32 = 1 << 0;
const MOD_STATIC: u32 = 1 << 1;
const MOD_ABSTRACT: u32 = 1 << 2;
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
            SemanticTokenType::STRING,
            SemanticTokenType::NUMBER,
            SemanticTokenType::COMMENT,
            SemanticTokenType::ENUM_MEMBER,
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
    // byte_of_position maps lines beyond EOF to byte 0 (`line_starts.get(..)
    // .unwrap_or(0)`); clamp those to the end of the source so an open-ended
    // viewport (end line u32::MAX style) prunes nothing instead of everything.
    let byte_of = |pos: tower_lsp::lsp_types::Position| -> u32 {
        if (pos.line as usize) < doc.line_starts().len() {
            sv.byte_of_position(pos)
        } else {
            doc.source().len() as u32
        }
    };
    let start_byte = byte_of(range.start);
    let end_byte = byte_of(range.end);
    let mut raw: Vec<RawToken> = Vec::new();
    collect_comments(sv, &mut raw);
    collect_stmts_pruned(sv, &doc.program().stmts, start_byte, end_byte, &mut raw);
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
    let offset = str_offset(sv.source(), name).unwrap_or(0);
    push_at(
        out,
        sv,
        offset,
        utf16_code_units(name),
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
    let name_offset = str_offset(sv.source(), name).unwrap_or(0);
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
        extra_len + utf16_code_units(name),
        token_type,
        modifiers,
    );
}

fn push_attributes(out: &mut Vec<RawToken>, sv: SourceView<'_>, attrs: &[Attribute<'_, '_>]) {
    for attr in attrs.iter() {
        let span = attr.name.span();
        let segment = &sv.source()[span.start as usize..span.end as usize];
        let len: u32 = utf16_code_units(segment);
        push_at(out, sv, span.start, len, TT_CLASS, 0);
    }
}

fn deprecated_mod(doc: Option<&php_ast::Comment<'_>>) -> u32 {
    doc.map(|c| {
        if parse_docblock(c.text).is_deprecated() {
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
/// Strip a trailing `\r` so CRLF line endings don't leak into comment lengths.
fn trim_trailing_cr(s: &str) -> &str {
    s.strip_suffix('\r').unwrap_or(s)
}

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
                    let text = trim_trailing_cr(&sv.source()[start..i]);
                    let len_utf16: u32 = utf16_code_units(text);
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
                let text = trim_trailing_cr(&sv.source()[start..i]);
                let len_utf16: u32 = utf16_code_units(text);
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
                let segment = trim_trailing_cr(&sv.source()[line_start..line_end]);
                let len_utf16: u32 = utf16_code_units(segment);
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
        let len_utf16: u32 = utf16_code_units(segment);
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
            let len: u32 = utf16_code_units(segment);
            push_at(out, sv, span.start, len, TT_TYPE, 0);
        }
        TypeHintKind::Keyword(builtin, span) => {
            let text = builtin.as_str();
            let len_utf16: u32 = utf16_code_units(text);
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

/// Walk only the statements / class-like members whose spans overlap
/// `start..=end` (byte offsets). Used by [`semantic_tokens_range`] so a
/// viewport request on a large single-class file doesn't pay the full
/// document walk: out-of-range top-level statements and class/trait members
/// are skipped entirely. In-range nodes are collected fully — the caller's
/// exact per-token filter trims any spill-over, so the final token set is
/// identical to the unpruned walk (every token's span lies within its
/// owning statement/member).
///
/// The `Class` / `Trait` header blocks mirror the corresponding arms in
/// [`collect_stmt`] — keep them in sync when token kinds change there.
fn collect_stmts_pruned(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    start: u32,
    end: u32,
    out: &mut Vec<RawToken>,
) {
    let member_in_range = |member: &php_ast::ClassMember<'_, '_>| {
        member.span.end >= start && member.span.start <= end
    };
    for stmt in stmts {
        if stmt.span.end < start || stmt.span.start > end {
            continue;
        }
        match &stmt.kind {
            StmtKind::Class(c) => {
                push_attributes(out, sv, &c.attributes);
                if let Some(name) = c.name {
                    let mut mods = MOD_DECLARATION | deprecated_mod(c.doc_comment.as_ref());
                    if c.modifiers.is_abstract {
                        mods |= MOD_ABSTRACT;
                    }
                    push_name(out, sv, &name.to_string(), TT_CLASS, mods);
                }
                for member in c.body.members.iter().filter(|m| member_in_range(m)) {
                    collect_class_member(sv, member, out);
                }
            }
            StmtKind::Trait(t) => {
                push_attributes(out, sv, &t.attributes);
                let mods = MOD_DECLARATION | deprecated_mod(t.doc_comment.as_ref());
                push_name(out, sv, &t.name.to_string(), TT_CLASS, mods);
                for member in t.body.members.iter().filter(|m| member_in_range(m)) {
                    collect_class_member(sv, member, out);
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_stmts_pruned(sv, &inner.stmts, start, end, out);
                }
            }
            // Enums are typically small; interfaces have no bodies to prune.
            // Everything else overlapped the range, so collect it fully.
            _ => collect_stmt(sv, stmt, out),
        }
    }
}

fn collect_stmt(sv: SourceView<'_>, stmt: &Stmt<'_, '_>, out: &mut Vec<RawToken>) {
    match &stmt.kind {
        StmtKind::Function(f) => {
            push_attributes(out, sv, &f.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(f.doc_comment.as_ref());
            push_name(out, sv, &f.name.to_string(), TT_FUNCTION, mods);
            for p in f.params.iter() {
                push_attributes(out, sv, &p.attributes);
                if let Some(th) = &p.type_hint {
                    push_type_hint(out, sv, th);
                }
                push_param(out, sv, &p.name.to_string(), TT_PARAMETER, MOD_DECLARATION);
            }
            if let Some(rt) = &f.return_type {
                push_type_hint(out, sv, rt);
            }
            collect_stmts(sv, &f.body.stmts, out);
        }
        StmtKind::Class(c) => {
            push_attributes(out, sv, &c.attributes);
            if let Some(name) = c.name {
                let mut mods = MOD_DECLARATION | deprecated_mod(c.doc_comment.as_ref());
                if c.modifiers.is_abstract {
                    mods |= MOD_ABSTRACT;
                }
                push_name(out, sv, &name.to_string(), TT_CLASS, mods);
            }
            for member in c.body.members.iter() {
                collect_class_member(sv, member, out);
            }
        }
        StmtKind::Interface(i) => {
            push_attributes(out, sv, &i.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(i.doc_comment.as_ref());
            push_name(out, sv, &i.name.to_string(), TT_INTERFACE, mods);
        }
        StmtKind::Trait(t) => {
            push_attributes(out, sv, &t.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(t.doc_comment.as_ref());
            push_name(out, sv, &t.name.to_string(), TT_CLASS, mods);
            for member in t.body.members.iter() {
                collect_class_member(sv, member, out);
            }
        }
        StmtKind::Enum(e) => {
            push_attributes(out, sv, &e.attributes);
            let mods = MOD_DECLARATION | deprecated_mod(e.doc_comment.as_ref());
            push_name(out, sv, &e.name.to_string(), TT_CLASS, mods);
            for member in e.body.members.iter() {
                match &member.kind {
                    EnumMemberKind::Case(c) => {
                        push_attributes(out, sv, &c.attributes);
                        let mmods = MOD_DECLARATION | deprecated_mod(c.doc_comment.as_ref());
                        push_name(out, sv, &c.name.to_string(), TT_ENUM_MEMBER, mmods);
                        if let Some(value) = &c.value {
                            collect_expr(sv, value, out);
                        }
                    }
                    EnumMemberKind::Method(m) => {
                        push_attributes(out, sv, &m.attributes);
                        let mut mmods = MOD_DECLARATION | deprecated_mod(m.doc_comment.as_ref());
                        if m.is_static {
                            mmods |= MOD_STATIC;
                        }
                        push_name(out, sv, &m.name.to_string(), TT_METHOD, mmods);
                        for p in m.params.iter() {
                            if let Some(th) = &p.type_hint {
                                push_type_hint(out, sv, th);
                            }
                            push_param(out, sv, &p.name.to_string(), TT_PARAMETER, MOD_DECLARATION);
                        }
                        if let Some(rt) = &m.return_type {
                            push_type_hint(out, sv, rt);
                        }
                        if let Some(body) = &m.body {
                            collect_stmts(sv, &body.stmts, out);
                        }
                    }
                    EnumMemberKind::ClassConst(k) => {
                        push_attributes(out, sv, &k.attributes);
                        let mmods = MOD_DECLARATION | deprecated_mod(k.doc_comment.as_ref());
                        if let Some(th) = &k.type_hint {
                            push_type_hint(out, sv, th);
                        }
                        push_name(out, sv, &k.name.to_string(), TT_PROPERTY, mmods);
                        collect_expr(sv, &k.value, out);
                    }
                    EnumMemberKind::TraitUse(_) => {
                        // Trait use declarations don't produce tokens
                    }
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                collect_stmts(sv, &inner.stmts, out);
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
            if let Some(key) = &f.key {
                collect_expr(sv, key, out);
            }
            collect_expr(sv, &f.value, out);
            collect_stmt(sv, f.body, out);
        }
        StmtKind::TryCatch(t) => {
            collect_stmts(sv, &t.body.stmts, out);
            for catch in t.catches.iter() {
                collect_stmts(sv, &catch.body.stmts, out);
            }
            if let Some(finally) = &t.finally {
                collect_stmts(sv, &finally.stmts, out);
            }
        }
        StmtKind::Block(stmts) => collect_stmts(sv, &stmts.stmts, out),
        StmtKind::Switch(s) => {
            collect_expr(sv, &s.expr, out);
            for case in s.body.cases.iter() {
                if let Some(v) = &case.value {
                    collect_expr(sv, v, out);
                }
                collect_stmts(sv, &case.body, out);
            }
        }
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
        let mut mods = MOD_DECLARATION | deprecated_mod(m.doc_comment.as_ref());
        if m.is_static {
            mods |= MOD_STATIC;
        }
        if m.is_abstract {
            mods |= MOD_ABSTRACT;
        }
        push_name(out, sv, &m.name.to_string(), TT_METHOD, mods);
        for p in m.params.iter() {
            push_attributes(out, sv, &p.attributes);
            if let Some(th) = &p.type_hint {
                push_type_hint(out, sv, th);
            }
            push_param(out, sv, &p.name.to_string(), TT_PARAMETER, MOD_DECLARATION);
        }
        if let Some(rt) = &m.return_type {
            push_type_hint(out, sv, rt);
        }
        if let Some(body) = &m.body {
            collect_stmts(sv, &body.stmts, out);
        }
    } else if let ClassMemberKind::Property(p) = &member.kind {
        push_attributes(out, sv, &p.attributes);
        if let Some(th) = &p.type_hint {
            push_type_hint(out, sv, th);
        }
        let mut mods = MOD_DECLARATION;
        if p.is_readonly {
            mods |= MOD_READONLY;
        }
        push_param(out, sv, &p.name.to_string(), TT_PROPERTY, mods);
    } else if let ClassMemberKind::ClassConst(k) = &member.kind {
        push_attributes(out, sv, &k.attributes);
        let mmods = MOD_DECLARATION | deprecated_mod(k.doc_comment.as_ref());
        if let Some(th) = &k.type_hint {
            push_type_hint(out, sv, th);
        }
        push_name(out, sv, &k.name.to_string(), TT_PROPERTY, mmods);
        collect_expr(sv, &k.value, out);
    }
}

fn collect_expr(sv: SourceView<'_>, expr: &php_ast::Expr<'_, '_>, out: &mut Vec<RawToken>) {
    match &expr.kind {
        ExprKind::Int(_) | ExprKind::Float(_) => {
            let span_len = expr.span.end - expr.span.start;
            push_at(out, sv, expr.span.start, span_len, TT_NUMBER, 0);
        }
        ExprKind::String(_) | ExprKind::Nowdoc { .. } => {
            let segment = &sv.source()[expr.span.start as usize..expr.span.end as usize];
            let len: u32 = utf16_code_units(segment);
            push_at(out, sv, expr.span.start, len, TT_STRING, 0);
        }
        ExprKind::InterpolatedString(parts) | ExprKind::ShellExec(parts) => {
            // Emit the whole span as a string; embedded variables are not
            // re-coloured here to keep the implementation simple.
            let segment = &sv.source()[expr.span.start as usize..expr.span.end as usize];
            let len: u32 = utf16_code_units(segment);
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
            let len: u32 = utf16_code_units(segment);
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
        ExprKind::FunctionCall(f) => {
            if let ExprKind::Identifier(name) = &f.name.kind {
                let name_str: &str = name;
                push_at(
                    out,
                    sv,
                    f.name.span.start,
                    utf16_code_units(name_str),
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
                    utf16_code_units(name_str),
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
                    utf16_code_units(name_str),
                    TT_METHOD,
                    0,
                );
            }
            for arg in m.args.iter() {
                collect_expr(sv, &arg.value, out);
            }
        }
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
                push_param(out, sv, &p.name.to_string(), TT_PARAMETER, MOD_DECLARATION);
            }
            if let Some(rt) = &c.return_type {
                push_type_hint(out, sv, rt);
            }
            collect_stmts(sv, &c.body.stmts, out);
        }
        ExprKind::ArrowFunction(af) => {
            for p in af.params.iter() {
                if let Some(th) = &p.type_hint {
                    push_type_hint(out, sv, th);
                }
                push_param(out, sv, &p.name.to_string(), TT_PARAMETER, MOD_DECLARATION);
            }
            if let Some(rt) = &af.return_type {
                push_type_hint(out, sv, rt);
            }
            collect_expr(sv, af.body, out);
        }
        ExprKind::Match(m) => {
            collect_expr(sv, m.subject, out);
            for arm in m.arms.iter() {
                if let Some(conds) = &arm.conditions {
                    for c in conds.iter() {
                        collect_expr(sv, c, out);
                    }
                }
                collect_expr(sv, &arm.body, out);
            }
        }
        ExprKind::Variable(_) => {
            let segment = &sv.source()[expr.span.start as usize..expr.span.end as usize];
            let len: u32 = utf16_code_units(segment);
            push_at(out, sv, expr.span.start, len, TT_VARIABLE, 0);
        }
        ExprKind::CloneWith(target, withs) => {
            collect_expr(sv, target, out);
            collect_expr(sv, withs, out);
        }
        ExprKind::PropertyAccess(a) | ExprKind::NullsafePropertyAccess(a) => {
            collect_expr(sv, a.object, out);
            if let ExprKind::Identifier(name) = &a.property.kind {
                let name_str: &str = name;
                push_at(
                    out,
                    sv,
                    a.property.span.start,
                    utf16_code_units(name_str),
                    TT_PROPERTY,
                    0,
                );
            } else {
                collect_expr(sv, a.property, out);
            }
        }
        ExprKind::StaticMethodCall(s) => {
            collect_class_ref(sv, s.class, out);
            if let ExprKind::Identifier(name) = &s.method.kind {
                let name_str: &str = name;
                push_at(
                    out,
                    sv,
                    s.method.span.start,
                    utf16_code_units(name_str),
                    TT_METHOD,
                    MOD_STATIC,
                );
            }
            for arg in s.args.iter() {
                collect_expr(sv, &arg.value, out);
            }
        }
        ExprKind::StaticDynMethodCall(s) => {
            collect_class_ref(sv, s.class, out);
            collect_expr(sv, s.method, out);
            for arg in s.args.iter() {
                collect_expr(sv, &arg.value, out);
            }
        }
        ExprKind::StaticPropertyAccess(a) => {
            collect_class_ref(sv, a.class, out);
            collect_expr(sv, a.member, out);
        }
        ExprKind::ClassConstAccess(a) => {
            collect_class_ref(sv, a.class, out);
            if let ExprKind::Identifier(name) = &a.member.kind {
                let name_str: &str = name;
                push_at(
                    out,
                    sv,
                    a.member.span.start,
                    utf16_code_units(name_str),
                    TT_PROPERTY,
                    MOD_STATIC,
                );
            } else {
                collect_expr(sv, a.member, out);
            }
        }
        ExprKind::ClassConstAccessDynamic { class, member }
        | ExprKind::StaticPropertyAccessDynamic { class, member } => {
            collect_class_ref(sv, class, out);
            collect_expr(sv, member, out);
        }
        ExprKind::CallableCreate(cc) => match &cc.kind {
            php_ast::CallableCreateKind::Function(f) => collect_expr(sv, f, out),
            php_ast::CallableCreateKind::Method { object, method }
            | php_ast::CallableCreateKind::NullsafeMethod { object, method } => {
                collect_expr(sv, object, out);
                if let ExprKind::Identifier(name) = &method.kind {
                    let name_str: &str = name;
                    push_at(
                        out,
                        sv,
                        method.span.start,
                        utf16_code_units(name_str),
                        TT_METHOD,
                        0,
                    );
                }
            }
            php_ast::CallableCreateKind::StaticMethod { class, method } => {
                collect_class_ref(sv, class, out);
                if let ExprKind::Identifier(name) = &method.kind {
                    let name_str: &str = name;
                    push_at(
                        out,
                        sv,
                        method.span.start,
                        utf16_code_units(name_str),
                        TT_METHOD,
                        MOD_STATIC,
                    );
                }
            }
        },
        _ => {}
    }
}

/// Tokenize the `class` side of a static access expression (`Foo::`,
/// `self::`, `$cls::`) — a bare class-like identifier gets `TT_CLASS`;
/// anything else (a variable holding a class name/object) recurses normally.
fn collect_class_ref(sv: SourceView<'_>, class: &php_ast::Expr<'_, '_>, out: &mut Vec<RawToken>) {
    if let ExprKind::Identifier(name) = &class.kind {
        let name_str: &str = name;
        push_at(
            out,
            sv,
            class.span.start,
            utf16_code_units(name_str),
            TT_CLASS,
            0,
        );
    } else {
        collect_expr(sv, class, out);
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

    /// The pruned range walk must produce exactly what the old
    /// collect-everything-then-filter implementation produced: the full token
    /// list filtered to the range. Exercises class members on both sides of
    /// the range boundary, a second top-level class entirely outside it, and
    /// a braced namespace.
    #[test]
    fn range_pruned_walk_matches_filtered_full_walk() {
        use tower_lsp::lsp_types::Position;
        let src = "<?php\nnamespace App {\nclass A {\n    public function one(): int { return 1; }\n    public function two(): string { return 'x'; }\n    public function three(): bool { return true; }\n}\nclass B {\n    public function four(): void {}\n}\n}\n";
        let d = doc(src);
        // Several windows, including degenerate and out-of-bounds ones.
        let windows = [(0u32, 4u32), (3, 5), (4, 9), (7, 11), (0, 99), (8, 8)];
        for (start_line, end_line) in windows {
            let range = Range {
                start: Position {
                    line: start_line,
                    character: 0,
                },
                end: Position {
                    line: end_line,
                    character: 0,
                },
            };
            let pruned = semantic_tokens_range(src, &d, range);
            // Reference: full walk + the same exact filter + re-encode.
            let sv = d.view();
            let mut raw: Vec<RawToken> = Vec::new();
            collect_comments(sv, &mut raw);
            collect_stmts(sv, &d.program().stmts, &mut raw);
            raw.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            let filtered: Vec<RawToken> = raw
                .into_iter()
                .filter(|(line, col, _len, _, _)| {
                    let after_start = *line > range.start.line
                        || (*line == range.start.line && *col >= range.start.character);
                    let before_end = *line < range.end.line
                        || (*line == range.end.line && *col < range.end.character);
                    after_start && before_end
                })
                .collect();
            let reference = delta_encode(filtered);
            assert_eq!(
                pruned, reference,
                "range ({start_line},{end_line}) pruned walk diverged from filtered full walk"
            );
        }
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
    fn legend_has_correct_token_count() {
        let l = legend();
        assert_eq!(l.token_types.len(), 13);
        assert_eq!(l.token_modifiers.len(), 5);
    }

    #[test]
    fn comment_lengths_exclude_crlf_carriage_return() {
        let src = "<?php\r\n// line comment\r\n# hash comment\r\n/* block\r\ncomment */\r\n";
        let d = doc(src);
        let sv = d.view();
        let mut raw: Vec<RawToken> = Vec::new();
        collect_comments(sv, &mut raw);
        let comment_lens: Vec<u32> = raw
            .into_iter()
            .filter(|&(_, _, _, tt, _)| tt == TT_COMMENT)
            .map(|(_, _, len, _, _)| len)
            .collect();
        assert_eq!(
            comment_lens,
            vec![
                "// line comment".len() as u32,
                "# hash comment".len() as u32,
                "/* block".len() as u32,
                "comment */".len() as u32,
            ]
        );
    }
}
