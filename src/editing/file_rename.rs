use php_ast::{NamespaceBody, Span, Stmt, StmtKind, UseDecl, UseKind};
use tower_lsp_server::ls_types::{Position, Range, TextEdit};

use crate::document::ast::{ParsedDoc, use_item_alias_and_fqn};
use crate::lang::file_header::{NamespaceSegment, UseInsertion, namespace_at, use_insertion};
use crate::navigation::walk::{all_class_ref_names_in_stmts, function_and_const_names_in_stmts};

/// Find every `use` item in `stmts` whose FQN (leading `\` ignored) equals
/// `target`, restricted to `use ClassName` statements (`UseKind::Normal`) —
/// `use function`/`use const` items are never touched by a class file-rename.
/// Returns `(enclosing statement span, item name span)` per match.
///
/// Skips group-use members (`use App\Model\{User};`), whose `Name` span
/// extends across the `{` into the prefix.
fn find_use_items(source: &str, stmts: &[Stmt<'_, '_>], target: &str, out: &mut Vec<(Span, Span)>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) if u.kind == UseKind::Normal => {
                for item in u.uses.iter() {
                    if !item
                        .name
                        .to_string_repr()
                        .trim_start_matches('\\')
                        .eq_ignore_ascii_case(target)
                    {
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
/// `TextEdit`s that replace `old_fqn` with `new_fqn` at each match. An import
/// made redundant by sharing the namespace of `new_fqn` is removed instead.
///
/// Handles:
/// - `use OldFqn;`
/// - `use \OldFqn;`
/// - `use OldFqn as Alias;`
/// - `use Old\{Fqn, Other};`
pub fn use_edits_in_source(doc: &ParsedDoc, old_fqn: &str, new_fqn: &str) -> Vec<TextEdit> {
    let old = old_fqn.trim_start_matches('\\');
    let new_clean = new_fqn.trim_start_matches('\\');
    let (new_ns, new_short) = (
        namespace_of(new_clean),
        crate::text::fqn_short_name(new_clean),
    );
    let source = doc.source();
    let sv = doc.view();
    let stmts = &doc.program().stmts;
    let mut matches = Vec::new();
    find_use_decl_items(stmts, old, &mut matches);

    let mut edits = Vec::new();
    for (stmt, decl, idx) in matches {
        let item = &decl.uses[idx];
        let in_new_ns = namespace_at(stmts, stmt.span.start)
            .name()
            .trim_start_matches('\\')
            .eq_ignore_ascii_case(new_ns);
        if in_new_ns && item.alias.is_none_or(|a| a.eq_ignore_ascii_case(new_short)) {
            edits.push(remove_use_item(doc, stmt, decl, idx));
        } else if is_group_member(item) {
            edits.extend(group_use_edits(doc, stmt, decl, idx, new_clean));
        } else {
            let span = fqn_span_without_backslash(source, item.name.span());
            edits.push(TextEdit {
                range: sv.range_of(span),
                new_text: new_clean.to_string(),
            });
        }
    }
    edits
}

fn is_group_member(item: &php_ast::UseItem<'_, '_>) -> bool {
    item.span.start > item.name.span().start
}

/// Each class `use` item in `stmts` importing `target`, plain or group member:
/// `(statement, its declaration, item index)`.
fn find_use_decl_items<'a, 'arena, 'src>(
    stmts: &'a [Stmt<'arena, 'src>],
    target: &str,
    out: &mut Vec<(&'a Stmt<'arena, 'src>, &'a UseDecl<'arena, 'src>, usize)>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) if u.kind == UseKind::Normal => {
                for (idx, item) in u.uses.iter().enumerate() {
                    if matches!(item.kind, None | Some(UseKind::Normal))
                        && item
                            .name
                            .to_string_repr()
                            .trim_start_matches('\\')
                            .eq_ignore_ascii_case(target)
                    {
                        out.push((stmt, u, idx));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    find_use_decl_items(&inner.stmts, target, out);
                }
            }
            _ => {}
        }
    }
}

/// Removes item `idx` of `decl` with its separating comma, or the whole
/// statement's lines when it is the only item.
fn remove_use_item(
    doc: &ParsedDoc,
    stmt: &Stmt<'_, '_>,
    decl: &UseDecl<'_, '_>,
    idx: usize,
) -> TextEdit {
    let view = doc.view();
    let range = |start: u32, end: u32| Range::new(view.position_of(start), view.position_of(end));
    let item = &decl.uses[idx];
    let range = match (decl.uses.get(idx + 1), idx.checked_sub(1)) {
        (Some(next), _) => range(item.span.start, next.span.start),
        (None, Some(prev)) => range(decl.uses[prev].span.end, item.span.end),
        (None, None) => {
            let first = view.position_of(stmt.span.start).line;
            let mut end = view.position_of(stmt.span.end).line + 1;
            let blank = |n: u32| {
                doc.source()
                    .lines()
                    .nth(n as usize)
                    .is_some_and(|l| l.trim().is_empty())
            };
            // Don't leave a doubled blank line where the import was.
            if first > 0 && blank(first - 1) && blank(end) {
                end += 1;
            }
            Range::new(Position::new(first, 0), Position::new(end, 0))
        }
    };
    TextEdit {
        range,
        new_text: String::new(),
    }
}

/// Edits making group-use member `idx` of `decl` import `new` instead: the
/// member is respelled when `new` shares the group prefix, else it moves to
/// its own `use` after the statement.
fn group_use_edits(
    doc: &ParsedDoc,
    stmt: &Stmt<'_, '_>,
    decl: &UseDecl<'_, '_>,
    idx: usize,
    new: &str,
) -> Vec<TextEdit> {
    let source = doc.source();
    let view = doc.view();
    let range = |start: u32, end: u32| Range::new(view.position_of(start), view.position_of(end));
    let item = &decl.uses[idx];
    let name_end = item.name.span().end;
    let local = &source[item.span.start as usize..name_end as usize];
    let fqn = item.name.to_string_repr();
    let prefix = fqn
        .trim_start_matches('\\')
        .strip_suffix(local)
        .unwrap_or_default();
    if let Some(rest) = new
        .get(..prefix.len())
        .filter(|p| !prefix.is_empty() && p.eq_ignore_ascii_case(prefix))
        .map(|_| &new[prefix.len()..])
    {
        return vec![TextEdit {
            range: range(item.span.start, name_end),
            new_text: rest.to_string(),
        }];
    }
    let import = match item.alias {
        Some(alias) => format!("use {new} as {alias};"),
        None => format!("use {new};"),
    };
    if decl.uses.len() == 1 {
        return vec![TextEdit {
            range: range(stmt.span.start, stmt.span.end),
            new_text: import,
        }];
    }
    let line_start = source[..stmt.span.start as usize]
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let indent = &source[line_start..stmt.span.start as usize];
    let indent = &indent[..indent.len() - indent.trim_start_matches([' ', '\t']).len()];
    let after = Position::new(view.position_of(stmt.span.end).line + 1, 0);
    vec![
        remove_use_item(doc, stmt, decl, idx),
        TextEdit {
            range: Range::new(after, after),
            new_text: format!("{indent}{import}\n"),
        },
    ]
}

fn namespace_of(fqn: &str) -> &str {
    fqn.trim_start_matches('\\')
        .rsplit_once('\\')
        .map_or("", |(ns, _)| ns)
}

/// Edits for a class file moved from `old_fqn` to `new_fqn`: its namespace
/// declaration, plus imports for names that resolved against the old namespace.
/// `exists` tells whether an FQN names a known class-like, function or constant.
pub fn moved_file_edits(
    doc: &ParsedDoc,
    old_fqn: &str,
    new_fqn: &str,
    exists: impl Fn(UseKind, &str) -> bool,
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
            exists(UseKind::Normal, &qualify(old_ns, name))
        });
        let mut stmts: Vec<String> = names
            .iter()
            .map(|name| format!("use {};", qualify(old_ns, name)))
            .collect();
        stmts.extend(old_namespace_function_const_imports(
            &segment, old_ns, &exists,
        ));
        if stmts.is_empty() {
            continue;
        }
        let insertion = use_insertion(doc, doc.view().position_of(segment.body_start), |_| true);
        let pos = insertion.position();
        // Edit dedup keeps one insert per position, so extend an inserted namespace line.
        match edits
            .iter_mut()
            .find(|e| e.range.start == pos && e.range.end == pos)
        {
            Some(ns_insert) => {
                ns_insert.new_text.push('\n');
                for stmt in &stmts {
                    ns_insert
                        .new_text
                        .push_str(&format!("{}{stmt}\n", insertion.indent));
                }
            }
            None => edits.push(separated_use_edit(doc, &insertion, &stmts)),
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
    let mut bound: std::collections::HashSet<String> = class_imports(segment)
        .map(|(alias, _)| alias.to_ascii_lowercase())
        .collect();
    for stmt in segment.stmts {
        match &stmt.kind {
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

/// `use function`/`use const` statements for unqualified calls and constants
/// in `segment` that resolve to ones declared in `old_ns`.
fn old_namespace_function_const_imports(
    segment: &NamespaceSegment<'_, '_, '_>,
    old_ns: &str,
    exists: &impl Fn(UseKind, &str) -> bool,
) -> Vec<String> {
    if old_ns.is_empty() {
        return Vec::new();
    }
    // Function names are case-insensitive, constant names case-sensitive.
    let mut bound_functions = std::collections::HashSet::new();
    let mut bound_consts = std::collections::HashSet::new();
    for stmt in segment.stmts {
        match &stmt.kind {
            StmtKind::Use(u) => {
                for item in u.uses.iter() {
                    let (alias, _) = use_item_alias_and_fqn(item);
                    match item.kind.unwrap_or(u.kind) {
                        UseKind::Function => bound_functions.insert(alias.to_ascii_lowercase()),
                        UseKind::Const => bound_consts.insert(alias),
                        UseKind::Normal => false,
                    };
                }
            }
            StmtKind::Function(f) => {
                bound_functions.extend(f.name.as_str().map(str::to_ascii_lowercase));
            }
            StmtKind::Const(items) => {
                bound_consts.extend(
                    items
                        .iter()
                        .filter_map(|i| i.name.as_str())
                        .map(String::from),
                );
            }
            _ => {}
        }
    }
    let (mut functions, mut constants) = function_and_const_names_in_stmts(segment.stmts);
    functions.sort_by_key(|n| n.to_ascii_lowercase());
    constants.sort();
    let mut out = Vec::new();
    for name in functions.iter().filter(|n| !n.contains('\\')) {
        let fqn = qualify(old_ns, name);
        if bound_functions.insert(name.to_ascii_lowercase()) && exists(UseKind::Function, &fqn) {
            out.push(format!("use function {fqn};"));
        }
    }
    for name in constants.iter().filter(|n| !n.contains('\\')) {
        let fqn = qualify(old_ns, name);
        if bound_consts.insert(name.clone()) && exists(UseKind::Const, &fqn) {
            out.push(format!("use const {fqn};"));
        }
    }
    out
}

/// `(local name, FQN)` of each class import in `segment`.
fn class_imports<'a>(
    segment: &'a NamespaceSegment<'_, '_, '_>,
) -> impl Iterator<Item = (String, String)> + 'a {
    segment
        .stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            StmtKind::Use(u) if u.kind == UseKind::Normal => Some(u.uses.iter()),
            _ => None,
        })
        .flatten()
        .filter(|item| matches!(item.kind, None | Some(UseKind::Normal)))
        .map(use_item_alias_and_fqn)
}

/// The FQN (no leading `\`) a class name spelled `name` in `segment` refers to.
fn resolve_class_name(segment: &NamespaceSegment<'_, '_, '_>, name: &str) -> String {
    if let Some(fq) = name.strip_prefix('\\') {
        return fq.to_string();
    }
    let ns = segment.name();
    let ns = ns.trim_start_matches('\\');
    if let Some(rest) = name
        .get(..10)
        .filter(|p| p.eq_ignore_ascii_case("namespace\\"))
        .map(|_| &name[10..])
    {
        return qualify(ns, rest);
    }
    let (first, rest) = name
        .split_once('\\')
        .map_or((name, None), |(f, r)| (f, Some(r)));
    match class_imports(segment).find(|(alias, _)| alias.eq_ignore_ascii_case(first)) {
        Some((_, fqn)) => {
            let fqn = fqn.trim_start_matches('\\');
            rest.map_or_else(|| fqn.to_string(), |r| format!("{fqn}\\{r}"))
        }
        None => qualify(ns, name),
    }
}

/// `new_fqn` spelled to replace the qualified name `spelled`: fully qualified,
/// unless `spelled` was a global-namespace name without the leading `\`.
fn requalify(spelled: &str, in_global: bool, new_fqn: &str) -> String {
    let new = new_fqn.trim_start_matches('\\');
    if in_global && !spelled.starts_with('\\') {
        new.to_string()
    } else {
        format!("\\{new}")
    }
}

/// Rewrites the qualified class reference ending at the short-name range
/// `short` to name `new_fqn`; `None` for an unqualified reference.
pub fn qualified_reference_edit(doc: &ParsedDoc, short: Range, new_fqn: &str) -> Option<TextEdit> {
    let source = doc.source();
    let view = doc.view();
    let end = view.byte_of_position(short.start) as usize;
    let start = source[..end]
        .trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '\\')
        .len();
    let qualifier = &source[start..end];
    if !qualifier.ends_with('\\') {
        return None;
    }
    let in_global = namespace_at(&doc.program().stmts, start as u32)
        .name()
        .is_empty();
    Some(TextEdit {
        range: Range::new(view.position_of(start as u32), short.end),
        new_text: requalify(qualifier, in_global, new_fqn),
    })
}

/// Edits for docblock type names in `doc` naming `old_fqn`: qualified ones are
/// rewritten to `new_fqn`, unqualified ones get a same-namespace import when
/// `import_unqualified`.
pub fn docblock_reference_edits(
    doc: &ParsedDoc,
    old_fqn: &str,
    new_fqn: &str,
    import_unqualified: bool,
) -> Vec<TextEdit> {
    let old = old_fqn.trim_start_matches('\\');
    let view = doc.view();
    let stmts = &doc.program().stmts;
    let mut edits = Vec::new();
    for (at, name) in docblock_type_names(doc.source()) {
        let segment = namespace_at(stmts, at);
        if !resolve_class_name(&segment, name).eq_ignore_ascii_case(old) {
            continue;
        }
        let range = Range::new(
            view.position_of(at),
            view.position_of(at + name.len() as u32),
        );
        if name.contains('\\') {
            edits.push(TextEdit {
                range,
                new_text: requalify(name, segment.name().is_empty(), new_fqn),
            });
        } else if import_unqualified
            && let Some(edit) = same_namespace_import_edit(doc, range.start, old_fqn, new_fqn)
        {
            edits.push(edit);
        }
    }
    edits
}

/// Case variants of class `fqn` spelled in `source`, as FQNs; mir keys class references by spelling.
pub fn class_case_variants(source: &str, fqn: &str, out: &mut std::collections::HashSet<String>) {
    let fqn = fqn.trim_start_matches('\\');
    let (ns, short) = (namespace_of(fqn), crate::text::fqn_short_name(fqn));
    let is_name_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let lower = source.to_ascii_lowercase();
    let mut find = |needle: &str, qualified: bool| {
        let needle = needle.to_ascii_lowercase();
        for (at, _) in lower.match_indices(&needle) {
            let end = at + needle.len();
            let bytes = source.as_bytes();
            let before = at.checked_sub(1).map(|i| bytes[i]);
            let bounded = before.is_none_or(|b| !is_name_byte(b) && (qualified || b != b'\\'))
                && bytes
                    .get(end)
                    .is_none_or(|&b| !is_name_byte(b) && b != b'\\');
            if !bounded {
                continue;
            }
            let spelled = &source[at..end];
            let variant = if qualified {
                spelled.to_string()
            } else {
                qualify(ns, spelled)
            };
            if variant != fqn {
                out.insert(variant);
            }
        }
    };
    if !ns.is_empty() {
        find(fqn, true);
    }
    find(short, false);
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
    let stmt = format!("use {};", new_fqn.trim_start_matches('\\'));
    Some(separated_use_edit(
        doc,
        &use_insertion(doc, at, |_| true),
        &[stmt],
    ))
}

/// Inserts `stmts` at `insertion`, blank-line separated from a preceding
/// `namespace X;` or `declare` and from following code.
fn separated_use_edit(doc: &ParsedDoc, insertion: &UseInsertion, stmts: &[String]) -> TextEdit {
    let line = |n: u32| doc.source().lines().nth(n as usize).map(str::trim);
    let opens_block = insertion
        .line
        .checked_sub(1)
        .and_then(line)
        .is_some_and(|l| l.ends_with('{'));
    let mut text = String::new();
    if !insertion.after_use && !opens_block {
        text.push('\n');
    }
    for stmt in stmts {
        text.push_str(&format!("{}{stmt}\n", insertion.indent));
    }
    let next = line(insertion.line);
    if next.is_some_and(|l| !l.is_empty() && !l.starts_with("use ") && l != "}") {
        text.push('\n');
    }
    let pos = insertion.position();
    TextEdit {
        range: Range::new(pos, pos),
        new_text: text,
    }
}
