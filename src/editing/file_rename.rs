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
fn find_use_items(stmts: &[Stmt<'_, '_>], target: &str, out: &mut Vec<(Span, Span)>) {
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
                    if is_group_member(item) {
                        continue;
                    }
                    out.push((stmt.span, item.name.span()));
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    find_use_items(&inner.stmts, target, out);
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
    find_use_items(&doc.program().stmts, clean, &mut matches);

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

/// Edits making each class `use` item in `doc` that imports a moved class,
/// per `moved_to`, import its new FQN; an import made redundant by sharing its
/// new namespace is removed instead. Edits for one statement are built
/// together so its moved members never yield overlapping edits.
///
/// Handles:
/// - `use OldFqn;`
/// - `use \OldFqn;`
/// - `use OldFqn as Alias;`
/// - `use Old\{Fqn, Other};`
pub fn use_edits_in_source(
    doc: &ParsedDoc,
    moved_to: impl Fn(&str) -> Option<String>,
) -> Vec<TextEdit> {
    let source = doc.source();
    let view = doc.view();
    let range = |start: u32, end: u32| Range::new(view.position_of(start), view.position_of(end));
    let stmts = &doc.program().stmts;
    let mut decls = Vec::new();
    class_use_decls(stmts, &mut decls);

    let mut edits = Vec::new();
    for (stmt, decl) in decls {
        let mut removed = vec![false; decl.uses.len()];
        let mut imports = Vec::new();
        for (idx, item) in decl.uses.iter().enumerate() {
            if !matches!(item.kind, None | Some(UseKind::Normal)) {
                continue;
            }
            let Some(new) = moved_to(item.name.to_string_repr().trim_start_matches('\\')) else {
                continue;
            };
            let new = new.trim_start_matches('\\');
            let in_new_ns = namespace_at(stmts, stmt.span.start)
                .name()
                .trim_start_matches('\\')
                .eq_ignore_ascii_case(namespace_of(new));
            let short = crate::text::fqn_short_name(new);
            if in_new_ns
                && item
                    .alias
                    .is_none_or(|a| a.or_error().eq_ignore_ascii_case(short))
            {
                removed[idx] = true;
            } else if !is_group_member(item) {
                let span = fqn_span_without_backslash(source, item.name.span());
                edits.push(TextEdit {
                    range: view.range_of(span),
                    new_text: new.to_string(),
                });
            } else if let Some(rest) = group_member_spelling(source, item, new) {
                edits.push(TextEdit {
                    range: range(item.span.start, item.name.span().end),
                    new_text: rest.to_string(),
                });
            } else {
                removed[idx] = true;
                imports.push(match item.alias {
                    Some(alias) => format!("use {new} as {alias};"),
                    None => format!("use {new};"),
                });
            }
        }
        if imports.is_empty() {
            edits.extend(use_item_removals(doc, stmt, decl, &removed));
            continue;
        }
        let line_start = source[..stmt.span.start as usize]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let indent = &source[line_start..stmt.span.start as usize];
        let indent = &indent[..indent.len() - indent.trim_start_matches([' ', '\t']).len()];
        if removed.iter().all(|&r| r) {
            edits.push(TextEdit {
                range: range(stmt.span.start, stmt.span.end),
                new_text: imports.join(&format!("\n{indent}")),
            });
            continue;
        }
        edits.extend(use_item_removals(doc, stmt, decl, &removed));
        let after = Position::new(view.position_of(stmt.span.end).line + 1, 0);
        edits.push(TextEdit {
            range: Range::new(after, after),
            new_text: imports
                .iter()
                .map(|import| format!("{indent}{import}\n"))
                .collect(),
        });
    }
    edits
}

/// A group member's name span covers only the text inside the braces, not the shared prefix.
fn is_group_member(item: &php_ast::UseItem<'_, '_>) -> bool {
    let span = item.name.span();
    item.name.join_parts().len() > (span.end - span.start) as usize
}

/// The spelling of `new` inside the group of member `item`, when `new` still
/// starts with the group prefix.
fn group_member_spelling<'n>(
    source: &str,
    item: &php_ast::UseItem<'_, '_>,
    new: &'n str,
) -> Option<&'n str> {
    let local = &source[item.span.start as usize..item.name.span().end as usize];
    let fqn = item.name.to_string_repr();
    let prefix = fqn
        .trim_start_matches('\\')
        .strip_suffix(local)
        .unwrap_or_default();
    new.get(..prefix.len())
        .filter(|p| !prefix.is_empty() && p.eq_ignore_ascii_case(prefix))
        .map(|_| &new[prefix.len()..])
}

/// Each class `use` statement in `stmts` with its declaration.
fn class_use_decls<'a, 'arena, 'src>(
    stmts: &'a [Stmt<'arena, 'src>],
    out: &mut Vec<(&'a Stmt<'arena, 'src>, &'a UseDecl<'arena, 'src>)>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) if u.kind == UseKind::Normal => out.push((stmt, u)),
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    class_use_decls(&inner.stmts, out);
                }
            }
            _ => {}
        }
    }
}

/// Edits removing the items of `decl` flagged in `removed` with their
/// separating commas, or the statement's lines when every item goes.
fn use_item_removals(
    doc: &ParsedDoc,
    stmt: &Stmt<'_, '_>,
    decl: &UseDecl<'_, '_>,
    removed: &[bool],
) -> Vec<TextEdit> {
    let view = doc.view();
    let range = |start: u32, end: u32| Range::new(view.position_of(start), view.position_of(end));
    let Some(last_kept) = removed.iter().rposition(|r| !r) else {
        return if removed.is_empty() {
            Vec::new()
        } else {
            vec![remove_use_stmt(doc, stmt)]
        };
    };
    let items = &decl.uses;
    let mut edits: Vec<TextEdit> = (0..last_kept)
        .filter(|&i| removed[i])
        .map(|i| TextEdit {
            range: range(items[i].span.start, items[i + 1].span.start),
            new_text: String::new(),
        })
        .collect();
    // Trailing removed items go with the comma after the last kept one.
    if last_kept + 1 < items.len() {
        edits.push(TextEdit {
            range: range(items[last_kept].span.end, items[items.len() - 1].span.end),
            new_text: String::new(),
        });
    }
    edits
}

/// Removes the lines of use statement `stmt`.
fn remove_use_stmt(doc: &ParsedDoc, stmt: &Stmt<'_, '_>) -> TextEdit {
    let view = doc.view();
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
    TextEdit {
        range: Range::new(Position::new(first, 0), Position::new(end, 0)),
        new_text: String::new(),
    }
}

pub fn namespace_of(fqn: &str) -> &str {
    fqn.trim_start_matches('\\')
        .rsplit_once('\\')
        .map_or("", |(ns, _)| ns)
}

/// Edits for a class file moved from `old_fqn` to `new_fqn`: its namespace
/// declaration, plus imports for names that resolved against the old namespace.
/// `exists` tells whether an FQN names a known class-like, function or constant;
/// `moved_to` gives the new FQN of a class moved in the same batch.
pub fn moved_file_edits(
    doc: &ParsedDoc,
    old_fqn: &str,
    new_fqn: &str,
    exists: impl Fn(UseKind, &str) -> bool,
    moved_to: impl Fn(&str) -> Option<String>,
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
        let removals = redundant_import_removals(doc, &segment, new_ns);
        let (functions, constants) = function_and_const_names_in_stmts(segment.stmts);
        // `Sub\helper()` resolves through namespace `Sub`, imported like a class.
        let qualified: Vec<&str> = functions
            .iter()
            .map(|n| (UseKind::Function, n))
            .chain(constants.iter().map(|n| (UseKind::Const, n)))
            .filter(|(kind, n)| {
                is_relative_qualified(n) && exists(*kind, &crate::text::qualify_name(old_ns, n))
            })
            .map(|(_, n)| n.as_str())
            .collect();
        let names = old_namespace_names(
            &segment,
            &docblock_names,
            &qualified,
            |name| exists(UseKind::Normal, &crate::text::qualify_name(old_ns, name)),
            |name| {
                moved_to(&crate::text::qualify_name(old_ns, name)).is_some_and(|to| {
                    to.eq_ignore_ascii_case(&crate::text::qualify_name(new_ns, name))
                })
            },
        );
        let mut stmts: Vec<String> = names
            .iter()
            .map(|name| {
                let old = crate::text::qualify_name(old_ns, name);
                format!("use {};", moved_to(&old).unwrap_or(old))
            })
            .collect();
        stmts.extend(old_namespace_function_const_imports(
            &segment, functions, constants, old_ns, &exists,
        ));
        if stmts.is_empty() {
            edits.extend(removals);
            continue;
        }
        let insertion = use_insertion(doc, doc.view().position_of(segment.body_start), |_| true);
        let pos = insertion.position();
        // A removal swallowing the blank line after the imports must stop at the insertion.
        edits.extend(removals.into_iter().map(|mut e| {
            if e.range.start < pos && pos < e.range.end {
                e.range.end = pos;
            }
            e
        }));
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

/// Edits removing class imports in `segment` that name a class in `new_ns`
/// under its own short name, since the file resolves those unqualified there.
fn redundant_import_removals(
    doc: &ParsedDoc,
    segment: &NamespaceSegment<'_, '_, '_>,
    new_ns: &str,
) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    for stmt in segment.stmts {
        let StmtKind::Use(decl) = &stmt.kind else {
            continue;
        };
        let redundant: Vec<bool> = decl
            .uses
            .iter()
            .map(|item| {
                let (alias, fqn) = use_item_alias_and_fqn(item);
                matches!(item.kind.unwrap_or(decl.kind), UseKind::Normal)
                    && namespace_of(&fqn).eq_ignore_ascii_case(new_ns)
                    && alias.eq_ignore_ascii_case(crate::text::fqn_short_name(&fqn))
            })
            .collect();
        edits.extend(use_item_removals(doc, stmt, decl, &redundant));
    }
    edits
}

fn is_namespace_relative(name: &str) -> bool {
    name.get(..10)
        .is_some_and(|p| p.eq_ignore_ascii_case("namespace\\"))
}

/// Qualified, but neither fully qualified nor `namespace\`-relative.
fn is_relative_qualified(name: &str) -> bool {
    name.contains('\\') && !name.starts_with('\\') && !is_namespace_relative(name)
}

/// Sorted leading segments of names in `segment` that resolve against its
/// namespace: neither fully qualified, imported, declared in it, nor special.
fn old_namespace_names(
    segment: &NamespaceSegment<'_, '_, '_>,
    docblock_names: &[(u32, &str)],
    qualified_functions_consts: &[&str],
    is_class: impl Fn(&str) -> bool,
    moves_along: impl Fn(&str) -> bool,
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
        .map(String::as_str)
        .filter(|name| !name.starts_with('\\') && !is_namespace_relative(name))
        .chain(qualified_functions_consts.iter().copied())
        .filter(|name| !moves_along(name))
        .map(|name| name.split('\\').next().unwrap_or(name));
    let doc_names = docblock_names
        .iter()
        .filter(|(at, name)| {
            (segment.body_start..end).contains(at)
                && !name.contains('\\')
                && is_class(name)
                && !moves_along(name)
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
    mut functions: Vec<String>,
    mut constants: Vec<String>,
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
                        _ => false,
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
    functions.sort_by_key(|n| n.to_ascii_lowercase());
    constants.sort();
    let mut out = Vec::new();
    for name in functions.iter().filter(|n| !n.contains('\\')) {
        let fqn = crate::text::qualify_name(old_ns, name);
        if bound_functions.insert(name.to_ascii_lowercase()) && exists(UseKind::Function, &fqn) {
            out.push(format!("use function {fqn};"));
        }
    }
    for name in constants.iter().filter(|n| !n.contains('\\')) {
        let fqn = crate::text::qualify_name(old_ns, name);
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
    resolve_class_name_in(segment, &segment.name(), name)
}

/// [`resolve_class_name`] with `segment`'s imports but namespace `ns`.
fn resolve_class_name_in(segment: &NamespaceSegment<'_, '_, '_>, ns: &str, name: &str) -> String {
    if let Some(fq) = name.strip_prefix('\\') {
        return fq.to_string();
    }
    let ns = ns.trim_start_matches('\\');
    if let Some(rest) = name
        .get(..10)
        .filter(|p| p.eq_ignore_ascii_case("namespace\\"))
        .map(|_| &name[10..])
    {
        return crate::text::qualify_name(ns, rest);
    }
    let (first, rest) = name
        .split_once('\\')
        .map_or((name, None), |(f, r)| (f, Some(r)));
    match class_imports(segment).find(|(alias, _)| alias.eq_ignore_ascii_case(first)) {
        Some((_, fqn)) => {
            let fqn = fqn.trim_start_matches('\\');
            rest.map_or_else(|| fqn.to_string(), |r| format!("{fqn}\\{r}"))
        }
        None => crate::text::qualify_name(ns, name),
    }
}

/// `new_fqn` spelled to replace the qualified name `spelled` in `segment`, which
/// will be in namespace `ns`: relative when `spelled` was and that still
/// resolves, else fully qualified.
fn requalify(
    segment: &NamespaceSegment<'_, '_, '_>,
    ns: &str,
    spelled: &str,
    new_fqn: &str,
) -> String {
    let new = new_fqn.trim_start_matches('\\');
    if !spelled.starts_with('\\') {
        let ns = ns.trim_start_matches('\\');
        let relative = if ns.is_empty() {
            Some(new)
        } else {
            new.get(..ns.len())
                .filter(|p| p.eq_ignore_ascii_case(ns) && new[ns.len()..].starts_with('\\'))
                .map(|_| &new[ns.len() + 1..])
        };
        if let Some(relative) =
            relative.filter(|r| resolve_class_name_in(segment, ns, r).eq_ignore_ascii_case(new))
        {
            return relative.to_string();
        }
    }
    format!("\\{new}")
}

/// Rewrites the qualified class reference ending at the short-name range
/// `short` to name `new_fqn`, for `doc` moving to namespace `ns_after` if any;
/// `None` for an unqualified reference or one already naming `new_fqn`.
pub fn qualified_reference_edit(
    doc: &ParsedDoc,
    short: Range,
    new_fqn: &str,
    ns_after: Option<&str>,
) -> Option<TextEdit> {
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
    let segment = namespace_at(&doc.program().stmts, start as u32);
    let ns = ns_after.map_or_else(|| segment.name().to_string(), str::to_string);
    let new_text = requalify(&segment, &ns, qualifier, new_fqn);
    let end = view.byte_of_position(short.end) as usize;
    (new_text != source[start..end]).then(|| TextEdit {
        range: Range::new(view.position_of(start as u32), short.end),
        new_text,
    })
}

/// Edits for docblock type names in `doc` naming `old_fqn`: qualified ones are
/// rewritten to `new_fqn`, unqualified ones get the new short name and, unless
/// `doc` moves to namespace `ns_after` (importing on its own), a same-namespace import.
pub fn docblock_reference_edits(
    doc: &ParsedDoc,
    old_fqn: &str,
    new_fqn: &str,
    ns_after: Option<&str>,
) -> Vec<TextEdit> {
    let old = old_fqn.trim_start_matches('\\');
    let (old_short, new_short) = (
        crate::text::fqn_short_name(old),
        crate::text::fqn_short_name(new_fqn),
    );
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
            let ns = ns_after.map_or_else(|| segment.name().to_string(), str::to_string);
            let new_text = requalify(&segment, &ns, name, new_fqn);
            if new_text != name {
                edits.push(TextEdit { range, new_text });
            }
        } else {
            if ns_after.is_none()
                && let Some(edit) = same_namespace_import_edit(doc, range.start, old_fqn, new_fqn)
            {
                edits.push(edit);
            }
            // An alias keeps its name; only the class's own short name changes.
            if old_short != new_short && name.eq_ignore_ascii_case(old_short) {
                edits.push(TextEdit {
                    range,
                    new_text: new_short.to_string(),
                });
            }
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
                crate::text::qualify_name(ns, spelled)
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
                range: sv.range_of(span),
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
