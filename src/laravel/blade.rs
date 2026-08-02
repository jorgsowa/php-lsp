//! Blade template (`.blade.php`) support — completion, hover, go-to-definition
//! and document links for Laravel string-key helpers (`route()`, `view()`,
//! `config()`, `asset()`, `env()`, `trans()`/`__()`) written inside `{{ }}`/
//! `{!! !!}` expressions, view-referencing directives (`@include`,
//! `@extends`, `@each`, `@component`), `@livewire(...)`, and Blade/Livewire
//! component tags (`<x-alert>`, `<x-forms.input>`, `<livewire:counter>`).
//!
//! PHP's own parser treats everything in a `.blade.php` file outside real
//! `<?php ?>` tags as inert inline HTML — `{{ route('home') }}` never
//! produces a `FunctionCall` node in the whole-file AST, so none of the
//! AST-based machinery in `string_call`/`mod.rs` ever sees it. Rather than
//! writing a parallel Blade-aware dispatch for every domain, [`scan`] finds
//! each Blade expression/directive-call's raw text, wraps it as a tiny
//! standalone PHP snippet (`<?php (RAW);` or `<?php RAW;`), reparses just
//! that fragment, and delegates to the existing whole-document dispatch
//! functions ([`super::resolve_string_key`], [`super::hover_for_string_key`],
//! [`super::document_links`]) with the cursor position translated into the
//! fragment's own coordinate system. Any `Range` the fragment dispatch
//! returns (document links only — hover/go-to-definition target a *different*
//! file, so their positions need no translation) is translated back via
//! [`frag_position_to_doc_position`].
//!
//! Component/Livewire tags aren't calls at all, so they're resolved directly
//! against [`super::LaravelIndex::views`] (the `components.`/`livewire.`
//! prefix covers anonymous/view-only components) and the new
//! [`super::ComponentIndex`]/[`super::LivewireIndex`] (class-based fallback).
//!
//! No diagnostics are added for Blade files — same deliberate scope
//! boundary as every other domain in this module.

use std::path::Path;

use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, DocumentLink, Hover, Location, Position, Range, Uri,
};

use crate::document::ast::ParsedDoc;

use super::string_call;
use super::{LaravelIndex, hover};

/// Directive names whose first plain-string argument is a dot-separated
/// view name — the same resolution as a bare `view('a.b')` call.
const VIEW_DIRECTIVE_NAMES: &[&str] = &["include", "includeIf", "extends", "each", "component"];
/// Directive names whose first plain-string argument is a Livewire
/// component name.
const LIVEWIRE_DIRECTIVE_NAMES: &[&str] = &["livewire"];

const EXPR_WRAP_PREFIX: &str = "<?php (";
/// Synthetic call name every directive's argument list is wrapped under
/// before reparsing — using the *real* directive name (`include`, `require`,
/// ...) would break for `@include`, since PHP's parser treats `include(...)`
/// as the `include` language construct (`ExprKind::Include`), never a
/// `FunctionCall`, so `string_call`'s call-matching would never see it.
const DIRECTIVE_SYNTHETIC_NAME: &str = "__blade_directive__";

pub(crate) fn is_blade_uri(uri: &Uri) -> bool {
    uri.as_str().ends_with(".blade.php")
}

struct ExprSpan {
    /// Byte range of the raw expression text, `{{`/`{!!`/`}}`/`!!}` delimiters
    /// excluded.
    start: usize,
    end: usize,
}

struct DirectiveCall {
    /// Directive name as written, e.g. `"include"` or `"livewire"`.
    name: String,
    /// Byte range of `name(args)`, `@` excluded — this is what gets wrapped
    /// and reparsed, since it's already valid-looking PHP call syntax once
    /// the `@` is stripped.
    call_start: usize,
    call_end: usize,
}

struct ComponentTag {
    /// Dot-separated tag name, e.g. `"alert"` or `"forms.input"`.
    name: String,
    is_livewire: bool,
    /// Byte range of just the name text (excludes `<x-`/`<livewire:`) — used
    /// both for cursor-containment and as the reported `Range`.
    name_start: usize,
    name_end: usize,
}

#[derive(Default)]
struct BladeScan {
    exprs: Vec<ExprSpan>,
    directives: Vec<DirectiveCall>,
    tags: Vec<ComponentTag>,
}

/// Single-pass lexer over a Blade template's raw source. Byte-oriented
/// scanning (not `char`-oriented) is safe here: every delimiter this
/// recognizes (`@`, `{`, `<`, quotes) is ASCII, and no UTF-8 continuation
/// byte can equal an ASCII byte, so multi-byte characters are silently
/// skipped one byte at a time without ever being misread as a delimiter or
/// split across a returned span boundary.
fn scan(source: &str) -> BladeScan {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut out = BladeScan::default();
    let mut i = 0usize;
    while i < len {
        match bytes[i] {
            b'@' if bytes.get(i + 1) == Some(&b'{') => {
                // `@{{ ... }}` is Blade's literal-escape syntax (used to emit
                // raw `{{ }}` for e.g. a Vue.js template sharing the file) —
                // step past just the `@` so the following `{{` is recognized
                // as literal text below.
                i += 1;
            }
            b'@' => {
                let name_start = i + 1;
                let mut j = name_start;
                while j < len && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if j == name_start {
                    i += 1;
                    continue;
                }
                let name = &source[name_start..j];
                let mut k = j;
                while k < len && matches!(bytes[k], b' ' | b'\t') {
                    k += 1;
                }
                if is_known_directive(name)
                    && bytes.get(k) == Some(&b'(')
                    && let Some(call_end) = find_matching_paren(source, k)
                {
                    out.directives.push(DirectiveCall {
                        name: name.to_string(),
                        call_start: name_start,
                        call_end,
                    });
                    i = call_end;
                    continue;
                }
                i = j;
            }
            b'{' if source[i..].starts_with("{{--") => {
                let content_start = i + 4;
                i = match find_plain(source, content_start, "--}}") {
                    Some(close) => close + 4,
                    None => len,
                };
            }
            b'{' if source[i..].starts_with("{!!") => {
                let escaped = i > 0 && bytes[i - 1] == b'@';
                let content_start = i + 3;
                match find_unquoted(source, content_start, "!!}") {
                    Some(close) => {
                        if !escaped {
                            out.exprs.push(ExprSpan {
                                start: content_start,
                                end: close,
                            });
                        }
                        i = close + 3;
                    }
                    None => i = len,
                }
            }
            b'{' if source[i..].starts_with("{{") => {
                let escaped = i > 0 && bytes[i - 1] == b'@';
                let content_start = i + 2;
                match find_unquoted(source, content_start, "}}") {
                    Some(close) => {
                        if !escaped {
                            out.exprs.push(ExprSpan {
                                start: content_start,
                                end: close,
                            });
                        }
                        i = close + 2;
                    }
                    None => i = len,
                }
            }
            b'<' if source[i..].starts_with("<x-") => {
                let name_start = i + 3;
                let (name, name_end) = read_tag_name(source, name_start);
                if name.is_empty() {
                    i += 1;
                } else {
                    out.tags.push(ComponentTag {
                        name,
                        is_livewire: false,
                        name_start,
                        name_end,
                    });
                    i = name_end;
                }
            }
            b'<' if source[i..].starts_with("<livewire:") => {
                let name_start = i + "<livewire:".len();
                let (name, name_end) = read_tag_name(source, name_start);
                if name.is_empty() {
                    i += 1;
                } else {
                    out.tags.push(ComponentTag {
                        name,
                        is_livewire: true,
                        name_start,
                        name_end,
                    });
                    i = name_end;
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn is_known_directive(name: &str) -> bool {
    VIEW_DIRECTIVE_NAMES
        .iter()
        .chain(LIVEWIRE_DIRECTIVE_NAMES)
        .any(|n| n.eq_ignore_ascii_case(name))
}

fn read_tag_name(source: &str, start: usize) -> (String, usize) {
    let bytes = source.as_bytes();
    let mut j = start;
    while j < bytes.len()
        && (bytes[j].is_ascii_alphanumeric() || matches!(bytes[j], b'-' | b'.' | b'_'))
    {
        j += 1;
    }
    (source[start..j].to_string(), j)
}

fn find_plain(source: &str, start: usize, needle: &str) -> Option<usize> {
    source.get(start..)?.find(needle).map(|p| p + start)
}

/// Byte offset of the first occurrence of `needle` at or after `start`,
/// skipping over single/double-quoted string contents (backslash-escaped) so
/// a delimiter that happens to appear inside a string literal doesn't end
/// the span early.
fn find_unquoted(source: &str, start: usize, needle: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut i = start;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        if let Some(q) = quote {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match bytes[i] {
            b'\'' | b'"' => {
                quote = Some(bytes[i]);
                i += 1;
            }
            _ => {
                if bytes[i..].starts_with(needle_bytes) {
                    return Some(i);
                }
                i += 1;
            }
        }
    }
    None
}

/// Byte offset just past the `)` matching the `(` at `open_paren`,
/// respecting quoted strings and nested parens (`@include('a', ['b' => fn($x) => $x])`).
fn find_matching_paren(source: &str, open_paren: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = open_paren + 1;
    let mut depth = 1i32;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        if let Some(q) = quote {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match bytes[i] {
            b'\'' | b'"' => quote = Some(bytes[i]),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `Position` of byte offset `byte` within `source` — the inverse of
/// `crate::text::position_to_byte_offset`, needed here since Blade positions
/// are found by byte-offset lexing rather than derived from an existing
/// `Position`.
fn position_at(source: &str, byte: usize) -> Position {
    let byte = byte.min(source.len());
    let prefix = &source[..byte];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
    let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
    let character = crate::text::utf16_code_units(&prefix[line_start..]);
    Position { line, character }
}

/// Translates a document-absolute byte offset into a `Position` within
/// `wrapped` (the reparsed fragment), given that `wrapped[prefix_len..]`
/// is a byte-for-byte copy of the document starting at `doc_offset`.
fn doc_byte_to_frag_position(
    wrapped: &str,
    prefix_len: usize,
    doc_offset: usize,
    doc_byte: usize,
) -> Position {
    let frag_byte = prefix_len + doc_byte.saturating_sub(doc_offset);
    position_at(wrapped, frag_byte)
}

/// Translates a `Position` found within `wrapped` (the reparsed fragment)
/// back into a document-absolute `Position` in `source`.
fn frag_position_to_doc_position(
    source: &str,
    prefix_len: usize,
    doc_offset: usize,
    wrapped: &str,
    frag_pos: Position,
) -> Position {
    let frag_byte = crate::text::position_to_byte_offset(wrapped, frag_pos);
    let doc_byte = doc_offset + frag_byte.saturating_sub(prefix_len);
    position_at(source, doc_byte)
}

fn directive_lookup(laravel: &LaravelIndex, directive: &str, key: &str) -> Option<Location> {
    if LIVEWIRE_DIRECTIVE_NAMES
        .iter()
        .any(|n| n.eq_ignore_ascii_case(directive))
    {
        resolve_livewire(laravel, key)
    } else {
        laravel.views.get(key).cloned()
    }
}

fn resolve_component(laravel: &LaravelIndex, name: &str) -> Option<Location> {
    laravel
        .views
        .get(&format!("components.{name}"))
        .or_else(|| laravel.components.get(name))
        .cloned()
}

fn resolve_livewire(laravel: &LaravelIndex, name: &str) -> Option<Location> {
    laravel
        .livewire
        .get(name)
        .or_else(|| laravel.views.get(&format!("livewire.{name}")))
        .cloned()
}

fn resolve_tag(laravel: &LaravelIndex, name: &str, is_livewire: bool) -> Option<Location> {
    if is_livewire {
        resolve_livewire(laravel, name)
    } else {
        resolve_component(laravel, name)
    }
}

fn resolve_in_expr(
    source: &str,
    expr: &ExprSpan,
    doc_byte: usize,
    laravel: &LaravelIndex,
) -> Option<Location> {
    let raw = &source[expr.start..expr.end];
    let wrapped = format!("{EXPR_WRAP_PREFIX}{raw});");
    let frag_pos = doc_byte_to_frag_position(&wrapped, EXPR_WRAP_PREFIX.len(), expr.start, doc_byte);
    let frag_doc = ParsedDoc::parse(wrapped);
    super::resolve_string_key(&frag_doc, frag_pos, laravel)
}

/// Wraps a directive's argument list (the text between its parens) as a
/// standalone reparseable call to [`DIRECTIVE_SYNTHETIC_NAME`], returning the
/// wrapped source, its prefix length, and the document byte offset where the
/// argument list itself begins (for position translation).
fn wrap_directive(source: &str, call: &DirectiveCall) -> (String, usize, usize) {
    let raw = &source[call.call_start..call.call_end];
    // `scan` only ever records a `DirectiveCall` once it has found the
    // matching `(`/`)` pair, so `raw` is always `name`, optional whitespace,
    // then a balanced `(...)` — the `find`/slicing below can't fail.
    let open = raw.find('(').unwrap_or(raw.len().saturating_sub(1));
    let args = raw.get(open + 1..raw.len().saturating_sub(1)).unwrap_or("");
    let args_doc_start = call.call_start + open + 1;
    let prefix = format!("<?php {DIRECTIVE_SYNTHETIC_NAME}(");
    let prefix_len = prefix.len();
    let wrapped = format!("{prefix}{args});");
    (wrapped, prefix_len, args_doc_start)
}

fn resolve_in_directive(
    source: &str,
    call: &DirectiveCall,
    doc_byte: usize,
    laravel: &LaravelIndex,
) -> Option<Location> {
    let (wrapped, prefix_len, args_doc_start) = wrap_directive(source, call);
    let frag_pos = doc_byte_to_frag_position(&wrapped, prefix_len, args_doc_start, doc_byte);
    let frag_doc = ParsedDoc::parse(wrapped);
    let (key, _) =
        string_call::call_string_arg(&frag_doc, frag_pos, &[DIRECTIVE_SYNTHETIC_NAME])?;
    directive_lookup(laravel, &call.name, &key)
}

/// Go-to-definition for the cursor position inside a Blade template — a
/// component/Livewire tag name, a `{{ }}`/`{!! !!}` expression, or a
/// view/Livewire-referencing directive call. `None` for non-Laravel
/// workspaces, non-Blade files, or when the cursor isn't inside any
/// recognized construct.
pub(crate) fn resolve_definition(
    uri: &Uri,
    source: &str,
    position: Position,
    laravel: &LaravelIndex,
) -> Option<Location> {
    if !laravel.is_laravel || !is_blade_uri(uri) {
        return None;
    }
    let scan = scan(source);
    let doc_byte = crate::text::position_to_byte_offset(source, position);

    for tag in &scan.tags {
        if doc_byte < tag.name_start || doc_byte > tag.name_end {
            continue;
        }
        return resolve_tag(laravel, &tag.name, tag.is_livewire);
    }
    for expr in &scan.exprs {
        if doc_byte < expr.start || doc_byte > expr.end {
            continue;
        }
        if let Some(loc) = resolve_in_expr(source, expr, doc_byte, laravel) {
            return Some(loc);
        }
    }
    for call in &scan.directives {
        if doc_byte < call.call_start || doc_byte > call.call_end {
            continue;
        }
        if let Some(loc) = resolve_in_directive(source, call, doc_byte, laravel) {
            return Some(loc);
        }
    }
    None
}

/// Hover for the cursor position inside a Blade template — same recognized
/// constructs as [`resolve_definition`].
pub(crate) fn hover(
    uri: &Uri,
    source: &str,
    position: Position,
    laravel: &LaravelIndex,
    root: Option<&Path>,
) -> Option<Hover> {
    if !laravel.is_laravel || !is_blade_uri(uri) {
        return None;
    }
    let scan = scan(source);
    let doc_byte = crate::text::position_to_byte_offset(source, position);

    for tag in &scan.tags {
        if doc_byte < tag.name_start || doc_byte > tag.name_end {
            continue;
        }
        let Some(loc) = resolve_tag(laravel, &tag.name, tag.is_livewire) else {
            continue;
        };
        let heading = if tag.is_livewire {
            format!("<livewire:{}>", tag.name)
        } else {
            format!("<x-{}>", tag.name)
        };
        return Some(hover::key_hover(root, &loc, &heading, "php", false));
    }
    for expr in &scan.exprs {
        if doc_byte < expr.start || doc_byte > expr.end {
            continue;
        }
        let raw = &source[expr.start..expr.end];
        let wrapped = format!("{EXPR_WRAP_PREFIX}{raw});");
        let frag_pos =
            doc_byte_to_frag_position(&wrapped, EXPR_WRAP_PREFIX.len(), expr.start, doc_byte);
        let frag_doc = ParsedDoc::parse(wrapped);
        if let Some(h) = super::hover_for_string_key(&frag_doc, frag_pos, laravel, root) {
            return Some(h);
        }
    }
    for call in &scan.directives {
        if doc_byte < call.call_start || doc_byte > call.call_end {
            continue;
        }
        let (wrapped, prefix_len, args_doc_start) = wrap_directive(source, call);
        let frag_pos = doc_byte_to_frag_position(&wrapped, prefix_len, args_doc_start, doc_byte);
        let frag_doc = ParsedDoc::parse(wrapped);
        let Some((key, _)) =
            string_call::call_string_arg(&frag_doc, frag_pos, &[DIRECTIVE_SYNTHETIC_NAME])
        else {
            continue;
        };
        let Some(loc) = directive_lookup(laravel, &call.name, &key) else {
            continue;
        };
        return Some(hover::key_hover(
            root,
            &loc,
            &format!("@{}('{key}')", call.name),
            "php",
            false,
        ));
    }
    None
}

/// Document links for every recognized construct in a Blade template — same
/// coverage as [`resolve_definition`], swept across the whole file.
pub(crate) fn document_links(uri: &Uri, source: &str, laravel: &LaravelIndex) -> Vec<DocumentLink> {
    if !laravel.is_laravel || !is_blade_uri(uri) {
        return Vec::new();
    }
    let scan = scan(source);
    let mut out = Vec::new();

    for tag in &scan.tags {
        let Some(loc) = resolve_tag(laravel, &tag.name, tag.is_livewire) else {
            continue;
        };
        let range = Range {
            start: position_at(source, tag.name_start),
            end: position_at(source, tag.name_end),
        };
        out.push(DocumentLink {
            range,
            target: Some(loc.uri.clone()),
            tooltip: Some(tag.name.clone()),
            data: None,
        });
    }

    for expr in &scan.exprs {
        let raw = &source[expr.start..expr.end];
        let wrapped = format!("{EXPR_WRAP_PREFIX}{raw});");
        let frag_doc = ParsedDoc::parse(wrapped.clone());
        for mut link in super::document_links(&frag_doc, laravel) {
            link.range.start = frag_position_to_doc_position(
                source,
                EXPR_WRAP_PREFIX.len(),
                expr.start,
                &wrapped,
                link.range.start,
            );
            link.range.end = frag_position_to_doc_position(
                source,
                EXPR_WRAP_PREFIX.len(),
                expr.start,
                &wrapped,
                link.range.end,
            );
            out.push(link);
        }
    }

    for call in &scan.directives {
        let (wrapped, prefix_len, args_doc_start) = wrap_directive(source, call);
        let frag_doc = ParsedDoc::parse(wrapped.clone());
        for (key, range) in string_call::find_all_calls(&frag_doc, &[DIRECTIVE_SYNTHETIC_NAME]) {
            let Some(loc) = directive_lookup(laravel, &call.name, &key) else {
                continue;
            };
            let start = frag_position_to_doc_position(
                source,
                prefix_len,
                args_doc_start,
                &wrapped,
                range.start,
            );
            let end = frag_position_to_doc_position(
                source,
                prefix_len,
                args_doc_start,
                &wrapped,
                range.end,
            );
            out.push(DocumentLink {
                range: Range { start, end },
                target: Some(loc.uri.clone()),
                tooltip: Some(key),
                data: None,
            });
        }
    }

    out
}

fn tag_prefix(source: &str, position: Position, marker: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let line = *lines.get(position.line as usize)?;
    let byte_col = crate::text::utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..byte_col];
    let idx = before.rfind(marker)?;
    let after = &before[idx + marker.len()..];
    if after
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    {
        Some(after.to_string())
    } else {
        None
    }
}

fn component_completions(laravel: &LaravelIndex, prefix: &str) -> Vec<CompletionItem> {
    let anon = laravel.views.names().filter_map(|n| n.strip_prefix("components."));
    let mut items: Vec<CompletionItem> = anon
        .chain(laravel.components.names())
        .filter(|n| n.starts_with(prefix))
        .map(|n| CompletionItem {
            label: n.to_string(),
            kind: Some(CompletionItemKind::CLASS),
            insert_text: Some(n.to_string()),
            ..Default::default()
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup_by(|a, b| a.label == b.label);
    items
}

fn livewire_completions(laravel: &LaravelIndex, prefix: &str) -> Vec<CompletionItem> {
    let view_based = laravel.views.names().filter_map(|n| n.strip_prefix("livewire."));
    let mut items: Vec<CompletionItem> = laravel
        .livewire
        .names()
        .chain(view_based)
        .filter(|n| n.starts_with(prefix))
        .map(|n| CompletionItem {
            label: n.to_string(),
            kind: Some(CompletionItemKind::CLASS),
            insert_text: Some(n.to_string()),
            ..Default::default()
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup_by(|a, b| a.label == b.label);
    items
}

/// Completions for the cursor position inside a Blade template — component/
/// Livewire tag names (`<x-al`, `<livewire:cou`) and view/Livewire directive
/// arguments (`@include('lay`, `@livewire('cou`). Bare helper calls inside
/// `{{ }}` (`{{ route('ho` etc.) are already covered by
/// [`super::completions_for_string_key`], which is a pure text scan that
/// doesn't care whether it's running inside a Blade expression or plain PHP —
/// no Blade-specific handling needed for those.
pub(crate) fn completions(
    uri: &Uri,
    source: &str,
    position: Position,
    laravel: Option<&LaravelIndex>,
) -> Option<Vec<CompletionItem>> {
    let laravel = laravel.filter(|l| l.is_laravel)?;
    if !is_blade_uri(uri) {
        return None;
    }
    if let Some(prefix) = string_call::call_string_prefix(source, position, VIEW_DIRECTIVE_NAMES) {
        return Some(super::view_index::view_completions(&laravel.views, &prefix));
    }
    if let Some(prefix) = string_call::call_string_prefix(source, position, LIVEWIRE_DIRECTIVE_NAMES) {
        return Some(livewire_completions(laravel, &prefix));
    }
    if let Some(prefix) = tag_prefix(source, position, "<x-") {
        return Some(component_completions(laravel, &prefix));
    }
    if let Some(prefix) = tag_prefix(source, position, "<livewire:") {
        return Some(livewire_completions(laravel, &prefix));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn laravel_root(tmp: &Path) {
        std::fs::write(tmp.join("artisan"), "#!/usr/bin/env php").unwrap();
    }

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn scan_finds_double_and_raw_echo_expressions() {
        let scan = scan("<h1>{{ route('home') }}</h1>\n<p>{!! $bio !!}</p>\n");
        assert_eq!(scan.exprs.len(), 2);
    }

    #[test]
    fn scan_skips_comment_and_literal_escape() {
        let scan = scan("{{-- a comment with {{ fake }} inside --}}\n@{{ raw }}\n");
        assert!(scan.exprs.is_empty());
    }

    #[test]
    fn scan_finds_known_directive_calls() {
        let scan = scan("@include('layouts.app')\n@extends('layouts.base')\n@foo('x')\n");
        let names: Vec<&str> = scan.directives.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["include", "extends"]);
    }

    #[test]
    fn scan_finds_component_and_livewire_tags() {
        let scan = scan("<x-alert type=\"error\" />\n<x-forms.input />\n<livewire:counter />\n");
        let names: Vec<(&str, bool)> = scan
            .tags
            .iter()
            .map(|t| (t.name.as_str(), t.is_livewire))
            .collect();
        assert_eq!(
            names,
            vec![
                ("alert", false),
                ("forms.input", false),
                ("counter", true),
            ]
        );
    }

    #[test]
    fn resolve_definition_finds_helper_call_inside_double_brace_expr() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        write(
            tmp.path(),
            "resources/views/welcome.blade.php",
            "<h1>Hi</h1>",
        );
        let laravel = LaravelIndex::load(tmp.path());
        let source = "<div>{{ view('welcome') }}</div>\n";
        let uri = Uri::from_file_path(tmp.path().join("resources/views/x.blade.php")).unwrap();
        // Cursor inside "welcome".
        let pos = Position {
            line: 0,
            character: 20,
        };
        let loc = resolve_definition(&uri, source, pos, &laravel).unwrap();
        assert!(loc.uri.as_str().ends_with("welcome.blade.php"));
    }

    #[test]
    fn resolve_definition_finds_include_directive() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        write(
            tmp.path(),
            "resources/views/layouts/app.blade.php",
            "<html></html>",
        );
        let laravel = LaravelIndex::load(tmp.path());
        let source = "@include('layouts.app')\n";
        let uri = Uri::from_file_path(tmp.path().join("resources/views/x.blade.php")).unwrap();
        let pos = Position {
            line: 0,
            character: 15,
        };
        let loc = resolve_definition(&uri, source, pos, &laravel).unwrap();
        assert!(loc.uri.as_str().ends_with("layouts/app.blade.php"));
    }

    #[test]
    fn resolve_definition_finds_anonymous_component_tag() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        write(
            tmp.path(),
            "resources/views/components/alert.blade.php",
            "<div></div>",
        );
        let laravel = LaravelIndex::load(tmp.path());
        let source = "<x-alert type=\"error\" />\n";
        let uri = Uri::from_file_path(tmp.path().join("resources/views/x.blade.php")).unwrap();
        let pos = Position {
            line: 0,
            character: 5,
        };
        let loc = resolve_definition(&uri, source, pos, &laravel).unwrap();
        assert!(loc.uri.as_str().ends_with("components/alert.blade.php"));
    }

    #[test]
    fn resolve_definition_falls_back_to_class_based_component() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        write(
            tmp.path(),
            "app/View/Components/Alert.php",
            "<?php class Alert {}\n",
        );
        let laravel = LaravelIndex::load(tmp.path());
        let source = "<x-alert type=\"error\" />\n";
        let uri = Uri::from_file_path(tmp.path().join("resources/views/x.blade.php")).unwrap();
        let pos = Position {
            line: 0,
            character: 5,
        };
        let loc = resolve_definition(&uri, source, pos, &laravel).unwrap();
        assert!(loc.uri.as_str().ends_with("Alert.php"));
    }

    #[test]
    fn resolve_definition_finds_livewire_tag_and_directive() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        write(
            tmp.path(),
            "app/Livewire/Counter.php",
            "<?php class Counter {}\n",
        );
        let laravel = LaravelIndex::load(tmp.path());
        let uri = Uri::from_file_path(tmp.path().join("resources/views/x.blade.php")).unwrap();

        let source = "<livewire:counter />\n";
        let pos = Position {
            line: 0,
            character: 13,
        };
        let loc = resolve_definition(&uri, source, pos, &laravel).unwrap();
        assert!(loc.uri.as_str().ends_with("Counter.php"));

        let source = "@livewire('counter')\n";
        let pos = Position {
            line: 0,
            character: 13,
        };
        let loc = resolve_definition(&uri, source, pos, &laravel).unwrap();
        assert!(loc.uri.as_str().ends_with("Counter.php"));
    }

    #[test]
    fn resolve_definition_none_for_non_blade_file() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        write(
            tmp.path(),
            "resources/views/welcome.blade.php",
            "<h1>Hi</h1>",
        );
        let laravel = LaravelIndex::load(tmp.path());
        let source = "<div>{{ view('welcome') }}</div>\n";
        let uri = Uri::from_file_path(tmp.path().join("app.php")).unwrap();
        let pos = Position {
            line: 0,
            character: 20,
        };
        assert!(resolve_definition(&uri, source, pos, &laravel).is_none());
    }

    #[test]
    fn hover_shows_component_heading() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        write(
            tmp.path(),
            "resources/views/components/alert.blade.php",
            "<div></div>",
        );
        let laravel = LaravelIndex::load(tmp.path());
        let source = "<x-alert />\n";
        let uri = Uri::from_file_path(tmp.path().join("resources/views/x.blade.php")).unwrap();
        let pos = Position {
            line: 0,
            character: 5,
        };
        let h = hover(&uri, source, pos, &laravel, Some(tmp.path())).unwrap();
        let tower_lsp_server::ls_types::HoverContents::Markup(content) = h.contents else {
            panic!("expected markup contents");
        };
        assert!(content.value.contains("<x-alert>"));
    }

    #[test]
    fn document_links_covers_expr_directive_and_tag() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        write(
            tmp.path(),
            "resources/views/welcome.blade.php",
            "<h1>Hi</h1>",
        );
        write(
            tmp.path(),
            "resources/views/layouts/app.blade.php",
            "<html></html>",
        );
        write(
            tmp.path(),
            "resources/views/components/alert.blade.php",
            "<div></div>",
        );
        let laravel = LaravelIndex::load(tmp.path());
        let source = "{{ view('welcome') }}\n@include('layouts.app')\n<x-alert />\n";
        let uri = Uri::from_file_path(tmp.path().join("resources/views/x.blade.php")).unwrap();
        let links = document_links(&uri, source, &laravel);
        assert_eq!(links.len(), 3);
    }

    #[test]
    fn completions_lists_component_tags_by_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        write(
            tmp.path(),
            "resources/views/components/alert.blade.php",
            "<div></div>",
        );
        let laravel = LaravelIndex::load(tmp.path());
        let uri = Uri::from_file_path(tmp.path().join("resources/views/x.blade.php")).unwrap();
        let source = "<x-al\n";
        let pos = Position {
            line: 0,
            character: 5,
        };
        let items = completions(&uri, source, pos, Some(&laravel)).unwrap();
        assert!(items.iter().any(|i| i.label == "alert"));
    }

    #[test]
    fn completions_lists_include_directive_views_by_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        write(
            tmp.path(),
            "resources/views/layouts/app.blade.php",
            "<html></html>",
        );
        let laravel = LaravelIndex::load(tmp.path());
        let uri = Uri::from_file_path(tmp.path().join("resources/views/x.blade.php")).unwrap();
        let source = "@include('layouts.\n";
        let pos = Position {
            line: 0,
            character: 18,
        };
        let items = completions(&uri, source, pos, Some(&laravel)).unwrap();
        assert!(items.iter().any(|i| i.label == "layouts.app"));
    }

    #[test]
    fn completions_none_for_non_blade_file() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        let laravel = LaravelIndex::load(tmp.path());
        let uri = Uri::from_file_path(tmp.path().join("app.php")).unwrap();
        let source = "<x-al\n";
        let pos = Position {
            line: 0,
            character: 5,
        };
        assert!(completions(&uri, source, pos, Some(&laravel)).is_none());
    }
}
