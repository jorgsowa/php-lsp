use php_ast::{NamespaceBody, Span, Stmt, StmtKind, UseKind};
use tower_lsp_server::ls_types::{Position, Range, TextEdit};

use crate::document::ast::{ParsedDoc, use_item_alias_and_fqn};
use crate::lang::file_header::{NamespaceSegment, namespace_at, use_insertion};
use crate::navigation::walk::all_class_ref_names_in_stmts;

/// Find every `use` item in `stmts` whose FQN (leading `\` ignored) equals
/// `target`, restricted to `use ClassName` statements (`UseKind::Normal`) —
/// `use function`/`use const` items are never touched by a class file-rename.
/// Returns `(enclosing statement span, item name span)` per match.
///
/// Does not expand group-use (`use App\Model\{User};`) — out of scope,
/// matching the text-scan this replaced. A group member's `Name` span
/// incorrectly extends across the `{` delimiter into the prefix (a parser
/// quirk, not a logical part of the name), so a naive text edit over that
/// span would consume the brace and corrupt the statement; `source` is used
/// to detect and skip this case rather than silently mis-editing it.
fn find_use_items(source: &str, stmts: &[Stmt<'_, '_>], target: &str, out: &mut Vec<(Span, Span)>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) if u.kind == UseKind::Normal => {
                for item in u.uses.iter() {
                    if item.name.to_string_repr().trim_start_matches('\\') != target {
                        continue;
                    }
                    let name_span = item.name.span();
                    let name_text = &source[name_span.start as usize..name_span.end as usize];
                    if name_text.contains(['{', '}']) {
                        continue;
                    }
                    out.push((stmt.span, name_span));
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    find_use_items(source, &inner.stmts, target, out);
                }
            }
            _ => {}
        }
    }
}

/// A name span's text always includes a leading `\` for a fully-qualified
/// name (`\Foo\Bar`) — narrow to just the FQN text so an edit doesn't
/// consume or duplicate the backslash.
fn fqn_span_without_backslash(source: &str, span: Span) -> Span {
    if source.as_bytes().get(span.start as usize) == Some(&b'\\') {
        Span {
            start: span.start + 1,
            end: span.end,
        }
    } else {
        span
    }
}

/// Return `TextEdit`s that delete the entire `use FQN;` line from `doc`.
pub fn delete_use_in_source(doc: &ParsedDoc, fqn: &str) -> Vec<TextEdit> {
    let clean = fqn.trim_start_matches('\\');
    let sv = doc.view();
    let mut matches = Vec::new();
    find_use_items(doc.source(), &doc.program().stmts, clean, &mut matches);

    // A `use A, B;` statement matching on both items would otherwise queue
    // the same line twice.
    let mut seen_lines = std::collections::HashSet::new();
    let mut edits = Vec::new();
    for (stmt_span, _) in matches {
        let line = sv.position_of(stmt_span.start).line;
        if !seen_lines.insert(line) {
            continue;
        }
        // Delete the whole line including its newline.
        edits.push(TextEdit {
            range: Range {
                start: Position { line, character: 0 },
                end: Position {
                    line: line + 1,
                    character: 0,
                },
            },
            new_text: String::new(),
        });
    }
    edits
}

/// Find `use` statements in `doc` that reference `old_fqn` and return
/// `TextEdit`s that replace `old_fqn` with `new_fqn` at each match.
///
/// Handles:
/// - `use OldFqn;`
/// - `use \OldFqn;`
/// - `use OldFqn as Alias;`
pub fn use_edits_in_source(doc: &ParsedDoc, old_fqn: &str, new_fqn: &str) -> Vec<TextEdit> {
    let old = old_fqn.trim_start_matches('\\');
    let new_clean = new_fqn.trim_start_matches('\\');
    let source = doc.source();
    let sv = doc.view();
    let mut matches = Vec::new();
    find_use_items(source, &doc.program().stmts, old, &mut matches);

    matches
        .into_iter()
        .map(|(_, name_span)| {
            let span = fqn_span_without_backslash(source, name_span);
            TextEdit {
                range: Range {
                    start: sv.position_of(span.start),
                    end: sv.position_of(span.end),
                },
                new_text: new_clean.to_string(),
            }
        })
        .collect()
}

fn namespace_of(fqn: &str) -> &str {
    fqn.trim_start_matches('\\')
        .rsplit_once('\\')
        .map_or("", |(ns, _)| ns)
}

/// Edits for a class file moved from `old_fqn` to `new_fqn`: its namespace
/// declaration, plus imports for names that resolved against the old namespace.
/// `is_class` tells whether an FQN names a known class-like, for docblock names.
pub fn moved_file_edits(
    doc: &ParsedDoc,
    old_fqn: &str,
    new_fqn: &str,
    is_class: impl Fn(&str) -> bool,
) -> Vec<TextEdit> {
    let (old_ns, new_ns) = (namespace_of(old_fqn), namespace_of(new_fqn));
    if old_ns.eq_ignore_ascii_case(new_ns) {
        return Vec::new();
    }
    let mut edits = namespace_edits_in_source(doc, old_fqn, new_fqn);
    let stmts = &doc.program().stmts;
    let segments: Vec<_> = if stmts
        .iter()
        .any(|s| matches!(s.kind, StmtKind::Namespace(_)))
    {
        stmts
            .iter()
            .filter(|s| matches!(s.kind, StmtKind::Namespace(_)))
            .map(|s| namespace_at(stmts, s.span.start))
            .filter(|seg| {
                seg.name()
                    .trim_start_matches('\\')
                    .eq_ignore_ascii_case(old_ns)
            })
            .collect()
    } else {
        vec![namespace_at(stmts, 0)]
    };
    let docblock_names = docblock_type_names(doc.source());
    for segment in segments {
        let names = old_namespace_names(&segment, &docblock_names, |name| {
            is_class(&qualify(old_ns, name))
        });
        if names.is_empty() {
            continue;
        }
        let insertion = use_insertion(doc, doc.view().position_of(segment.body_start), |_| true);
        let uses: String = names
            .iter()
            .map(|name| format!("{}use {};\n", insertion.indent, qualify(old_ns, name)))
            .collect();
        let pos = insertion.position();
        // Edit dedup keeps one insert per position, so extend an inserted namespace line.
        match edits
            .iter_mut()
            .find(|e| e.range.start == pos && e.range.end == pos)
        {
            Some(ns_insert) => {
                ns_insert.new_text.push('\n');
                ns_insert.new_text.push_str(&uses);
            }
            None => edits.push(TextEdit {
                range: Range::new(pos, pos),
                new_text: uses,
            }),
        }
    }
    edits
}

fn qualify(ns: &str, name: &str) -> String {
    if ns.is_empty() {
        name.to_string()
    } else {
        format!("{ns}\\{name}")
    }
}

/// Sorted leading segments of names in `segment` that resolve against its
/// namespace: neither fully qualified, imported, declared in it, nor special.
fn old_namespace_names(
    segment: &NamespaceSegment<'_, '_, '_>,
    docblock_names: &[(u32, &str)],
    is_class: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut bound = std::collections::HashSet::new();
    for stmt in segment.stmts {
        match &stmt.kind {
            StmtKind::Use(u) if u.kind == UseKind::Normal => {
                for item in u.uses.iter() {
                    if matches!(item.kind, None | Some(UseKind::Normal)) {
                        bound.insert(use_item_alias_and_fqn(item).0.to_ascii_lowercase());
                    }
                }
            }
            StmtKind::Class(c) => {
                bound.extend(c.name.and_then(|n| n.as_str()).map(str::to_ascii_lowercase))
            }
            StmtKind::Interface(d) => bound.extend(d.name.as_str().map(str::to_ascii_lowercase)),
            StmtKind::Trait(d) => bound.extend(d.name.as_str().map(str::to_ascii_lowercase)),
            StmtKind::Enum(d) => bound.extend(d.name.as_str().map(str::to_ascii_lowercase)),
            _ => {}
        }
    }
    let end = segment
        .stmts
        .last()
        .map_or(segment.body_start, |s| s.span.end);
    let code_names = all_class_ref_names_in_stmts(segment.stmts);
    let code_names = code_names
        .iter()
        .filter(|name| {
            !name.starts_with('\\')
                && !name
                    .get(..10)
                    .is_some_and(|p| p.eq_ignore_ascii_case("namespace\\"))
        })
        .map(|name| name.split('\\').next().unwrap_or(name));
    let doc_names = docblock_names
        .iter()
        .filter(|(at, name)| {
            (segment.body_start..end).contains(at) && !name.contains('\\') && is_class(name)
        })
        .map(|(_, name)| *name);
    let mut out: Vec<String> = Vec::new();
    for name in code_names.chain(doc_names) {
        let lower = name.to_ascii_lowercase();
        if is_non_class_type(&lower) || !bound.insert(lower) {
            continue;
        }
        out.push(name.to_string());
    }
    out.sort_by_key(|n| n.to_ascii_lowercase());
    out
}

const NON_CLASS_TYPES: &[&str] = &[
    "array", "bool", "boolean", "callable", "double", "false", "float", "int", "integer",
    "iterable", "list", "mixed", "never", "null", "numeric", "object", "parent", "resource",
    "scalar", "self", "static", "string", "true", "void",
];

fn is_non_class_type(lower: &str) -> bool {
    NON_CLASS_TYPES.contains(&lower)
}

const DOCBLOCK_TYPE_TAGS: &[&str] = &[
    "var",
    "param",
    "return",
    "throws",
    "property",
    "property-read",
    "property-write",
    "mixin",
    "extends",
    "implements",
    "use",
    "template-extends",
    "template-implements",
    "template-use",
];

/// Class-like names in the type of each type-bearing docblock tag in `source`,
/// with their byte offsets, in source spelling (`Foo`, `Sub\Foo`, `\Foo`).
fn docblock_type_names(source: &str) -> Vec<(u32, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(open) = source[from..].find("/**") {
        let start = from + open + 3;
        let end = source[start..]
            .find("*/")
            .map_or(source.len(), |i| start + i);
        let mut at = start;
        while let Some(i) = source[at..end].find('@') {
            let tag_start = at + i + 1;
            let tag_len = source[tag_start..end]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .unwrap_or(end - tag_start);
            let tag = &source[tag_start..tag_start + tag_len];
            let tag = tag
                .strip_prefix("phpstan-")
                .or_else(|| tag.strip_prefix("psalm-"))
                .unwrap_or(tag);
            at = tag_start + tag_len;
            if DOCBLOCK_TYPE_TAGS.contains(&tag) {
                at = type_names_at(source, at, end, &mut out);
            }
        }
        from = end;
    }
    out
}

/// Scans the type expression after a tag at `from`, up to whitespace outside
/// brackets; returns where it ends.
fn type_names_at<'s>(
    source: &'s str,
    from: usize,
    end: usize,
    out: &mut Vec<(u32, &'s str)>,
) -> usize {
    let bytes = source.as_bytes();
    let mut i = from;
    while i < end && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    let (mut depth, mut quote) = (0u32, None);
    while i < end {
        let b = bytes[i];
        if let Some(q) = quote {
            quote = (b != q).then_some(q);
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' => quote = Some(b),
            b'<' | b'(' | b'{' | b'[' => depth += 1,
            b'>' | b')' | b'}' | b']' => depth = depth.saturating_sub(1),
            b if b.is_ascii_whitespace() && depth == 0 => break,
            b if b.is_ascii_alphabetic() || b == b'_' || b == b'\\' => {
                let start = i;
                while i < end
                    && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'\\' | b'-'))
                {
                    i += 1;
                }
                let name = &source[start..i];
                let after_colon = bytes.get(i) == Some(&b':') && bytes.get(i + 1) != Some(&b':');
                let member = start >= 2 && &source[start - 2..start] == "::";
                let variable = start >= 1 && bytes[start - 1] == b'$';
                if !name.contains('-') && !after_colon && !member && !variable {
                    out.push((start as u32, name));
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    i
}

fn namespace_edits_in_source(doc: &ParsedDoc, old_fqn: &str, new_fqn: &str) -> Vec<TextEdit> {
    let (old_ns, new_ns) = (namespace_of(old_fqn), namespace_of(new_fqn));
    if old_ns.eq_ignore_ascii_case(new_ns) {
        return Vec::new();
    }
    let source = doc.source();
    let sv = doc.view();
    let stmts = &doc.program().stmts;
    let decls: Vec<_> = stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            StmtKind::Namespace(ns) => Some((stmt, ns)),
            _ => None,
        })
        .collect();

    if decls.is_empty() {
        // A global-namespace file gains its declaration after `<?php` and `declare`.
        let line = stmts
            .iter()
            .filter(|s| matches!(s.kind, StmtKind::Declare(_)))
            .map(|s| sv.position_of(s.span.end).line + 1)
            .max()
            .or_else(|| {
                source
                    .find("<?php")
                    .map(|o| sv.position_of(o as u32).line + 1)
            })
            .unwrap_or(0);
        let pos = Position { line, character: 0 };
        return vec![TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text: format!("\nnamespace {new_ns};\n"),
        }];
    }

    decls
        .into_iter()
        .filter_map(|(stmt, ns)| {
            let name = ns.name.as_ref()?;
            if !name
                .to_string_repr()
                .trim_start_matches('\\')
                .eq_ignore_ascii_case(old_ns)
            {
                return None;
            }
            let span = name.span();
            if new_ns.is_empty() && matches!(ns.body, NamespaceBody::Simple) {
                let line = sv.position_of(stmt.span.start).line;
                return Some(TextEdit {
                    range: Range {
                        start: Position { line, character: 0 },
                        end: Position {
                            line: line + 1,
                            character: 0,
                        },
                    },
                    new_text: String::new(),
                });
            }
            Some(TextEdit {
                range: Range {
                    start: sv.position_of(span.start),
                    end: sv.position_of(span.end),
                },
                new_text: new_ns.to_string(),
            })
        })
        .collect()
}

/// An edit inserting `use new_fqn;` when the class reference at `at` resolves
/// only because it shares the namespace of `old_fqn`: unqualified, in that
/// namespace, and not imported.
pub fn same_namespace_import_edit(
    doc: &ParsedDoc,
    at: Position,
    old_fqn: &str,
    new_fqn: &str,
) -> Option<TextEdit> {
    let (old_ns, new_ns) = (namespace_of(old_fqn), namespace_of(new_fqn));
    if old_ns.eq_ignore_ascii_case(new_ns) || new_ns.is_empty() {
        return None;
    }
    let source = doc.source();
    let offset = doc.view().byte_of_position(at);
    if offset > 0 && source.as_bytes().get(offset as usize - 1) == Some(&b'\\') {
        return None;
    }
    let segment = namespace_at(&doc.program().stmts, offset);
    if !segment
        .name()
        .trim_start_matches('\\')
        .eq_ignore_ascii_case(old_ns)
    {
        return None;
    }
    let mut imports = Vec::new();
    find_use_items(
        source,
        segment.stmts,
        old_fqn.trim_start_matches('\\'),
        &mut imports,
    );
    if !imports.is_empty() {
        return None;
    }
    Some(
        use_insertion(doc, at, |_| true)
            .edit(&format!("use {};", new_fqn.trim_start_matches('\\'))),
    )
}
