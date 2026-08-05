use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp_server::ls_types::{Location, Position, Range, Uri};

use super::walk::collect_var_refs_in_scope;
use crate::document::ast::{ParsedDoc, SourceView};
use crate::text::{word_at_position, zero_width_location};
use crate::types::resolve::{Container, Declaration, resolve_declaration};

/// Find the definition of the symbol under `position`.
/// Searches the current document first, then `other_docs` for cross-file resolution.
pub fn goto_definition(
    uri: &Uri,
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[(Uri, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Location> {
    let word = word_at_position(source, position)?;

    // For $variable, find the first occurrence in scope (= the definition/assignment).
    let sv = doc.view();
    if word.starts_with('$') {
        let bare = word.trim_start_matches('$');
        let byte_off = sv.byte_of_position(position) as usize;
        let mut spans = Vec::new();
        collect_var_refs_in_scope(&doc.program().stmts, bare, byte_off, &mut spans);
        if let Some((span, _)) = spans.into_iter().min_by_key(|(s, _)| s.start) {
            // Promoted property parameters include a visibility keyword before the
            // type and name (`private Database $db`); keep the full span so the cursor
            // lands on the complete declaration. Regular typed params (`int $x`) have
            // span.start at the type — narrow to just $var_name instead.
            let src_at_start = source.get(span.start as usize..).unwrap_or("");
            let is_promoted = src_at_start.starts_with("private ")
                || src_at_start.starts_with("public ")
                || src_at_start.starts_with("protected ")
                || src_at_start.starts_with("readonly ");
            let range = if is_promoted {
                Range {
                    start: sv.position_of(span.start),
                    end: sv.position_of(span.end),
                }
            } else {
                let name_with_sigil = format!("${bare}");
                let precise_start =
                    crate::document::ast::str_offset_in_range(source, span, &name_with_sigil)
                        .unwrap_or(span.start);
                Range {
                    start: sv.position_of(precise_start),
                    end: sv.position_of(precise_start + name_with_sigil.len() as u32),
                }
            };
            return Some(Location {
                uri: uri.clone(),
                range,
            });
        }
    }

    if let Some(range) = resolve_definition_range(sv, &doc.program().stmts, &word) {
        return Some(Location {
            uri: uri.clone(),
            range,
        });
    }

    for (other_uri, other_doc) in other_docs {
        let other_sv = other_doc.view();
        if let Some(range) = resolve_definition_range(other_sv, &other_doc.program().stmts, &word) {
            return Some(Location {
                uri: other_uri.clone(),
                range,
            });
        }
    }

    None
}

/// Search an AST for a declaration named `name`, returning its selection range.
/// Used by the PSR-4 fallback in the backend after resolving a class to a file.
pub fn find_declaration_range(_source: &str, doc: &ParsedDoc, name: &str) -> Option<Range> {
    let sv = doc.view();
    resolve_definition_range(sv, &doc.program().stmts, name)
}

/// Resolve `word` to a declaration in `stmts` and return its precise name range.
fn resolve_definition_range(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    word: &str,
) -> Option<Range> {
    // Definition resolves every declaration kind *except* enum constants
    // (which the original walker never matched).
    let decl = resolve_declaration(stmts, word, &|d| {
        !matches!(
            d,
            Declaration::ClassConst {
                container: Container::Enum,
                ..
            }
        )
    })?;
    Some(definition_name_range(sv, &decl))
}

fn definition_name_range(sv: SourceView<'_>, decl: &Declaration<'_>) -> Range {
    sv.name_range_in_span(decl.name(), decl.span())
}

/// Walk the class hierarchy (extends + traits) in the workspace index to find
/// `method_name` defined in `class_name` or any of its superclasses/traits.
///
/// Returns the first match in PHP's resolution order: class itself → traits →
/// parent → parent's traits, etc. `class_candidates` resolves a short name to
/// every class sharing it (typically `DocumentStore::class_candidates`,
/// mention-index-narrowed) instead of a linear scan over every workspace
/// class.
///
/// Deliberately does not use `resolve_class_ref`: that picks a single
/// disambiguated candidate, but a hierarchy walk must visit *every* class
/// sharing a short name (workspaces commonly have several, e.g. Laravel's
/// many `Factory`/`Request` classes), since any of them may contribute a
/// matching trait/parent to the search.
pub fn find_method_in_class_hierarchy(
    class_name: &str,
    method_name: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    class_candidates: &dyn Fn(&str) -> Vec<crate::db::workspace_index::ClassRef>,
) -> Option<Location> {
    let mut queue: std::collections::VecDeque<String> =
        std::collections::VecDeque::from([class_name.to_owned()]);
    let mut visited = std::collections::HashSet::new();

    while let Some(current) = queue.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }
        let short = crate::text::fqn_short_name(&current);
        let candidates = class_candidates(short);
        for cr in &candidates {
            let Some((uri, cls)) = wi.at(*cr) else {
                continue;
            };
            if cls.name.as_ref() != current.as_str()
                && cls.fqn.as_ref().trim_start_matches('\\') != current.as_str()
            {
                continue;
            }
            for m in &cls.methods {
                if m.name.as_ref() == method_name {
                    return Some(precise_method_location(
                        uri,
                        m.start_line,
                        m.name_char,
                        m.name.len(),
                    ));
                }
            }
            // `@method` docblock declarations — navigates to the tag line.
            for dm in &cls.doc_methods {
                if dm.name.as_ref() == method_name {
                    return Some(zero_width_location(uri, dm.start_line));
                }
            }
            // Trait alias: `use Trait { original as alias }` — redirect the
            // search to the original method name in the aliased trait.
            for alias in &cls.trait_method_aliases {
                if alias.alias.as_ref() == method_name {
                    let orig = alias.original.as_ref();
                    let search_in: Vec<&str> = match &alias.trait_name {
                        Some(t) => vec![t.as_ref()],
                        None => cls.traits.iter().map(|t| t.as_ref()).collect(),
                    };
                    for trt_name in search_in {
                        if let Some(loc) = find_method_in_class_hierarchy(
                            trt_name,
                            orig,
                            wi,
                            class_candidates,
                        ) {
                            return Some(loc);
                        }
                    }
                }
            }
            // Traits first (PHP MRO), then `@mixin` targets, then parent.
            for trt in &cls.traits {
                queue.push_back(trt.as_ref().to_owned());
            }
            for mx in &cls.mixins {
                queue.push_back(mx.as_ref().to_owned());
            }
            if let Some(parent) = &cls.parent {
                queue.push_back(parent.as_ref().to_owned());
            }
        }
    }
    None
}

fn precise_method_location(uri: &Uri, line: u32, name_char: u32, name_len: usize) -> Location {
    let start = Position {
        line,
        character: name_char,
    };
    let end = Position {
        line,
        character: name_char + name_len as u32,
    };
    Location {
        uri: uri.clone(),
        range: Range { start, end },
    }
}

/// Find the name range of method `method_name` declared directly on
/// `class_name` (class or trait) in `doc`. Does NOT walk the class hierarchy.
/// Used by the mir-backed goto-definition path to precisely locate the
/// winning trait method after insteadof conflict resolution.
pub fn find_method_range_in_class(
    doc: &ParsedDoc,
    class_name: &str,
    method_name: &str,
) -> Option<Range> {
    let sv = doc.view();
    find_method_range_impl(sv, &doc.program().stmts, class_name, method_name)
}

fn find_method_range_impl(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    method_name: &str,
) -> Option<Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name.as_ref().and_then(|n| n.as_str()) == Some(class_name) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return Some(sv.name_range_in_span(method_name, member.span));
                    }
                }
            }
            StmtKind::Trait(t) if t.name == class_name => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return Some(sv.name_range_in_span(method_name, member.span));
                    }
                }
            }
            StmtKind::Enum(e) if e.name == class_name => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return Some(sv.name_range_in_span(method_name, member.span));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(block) = &ns.body
                    && let Some(r) =
                        find_method_range_impl(sv, &block.stmts, class_name, method_name)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}
