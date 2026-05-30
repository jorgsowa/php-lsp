use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::ast::{ParsedDoc, SourceView, str_offset};
use crate::util::{strip_variable_sigil, utf16_code_units, word_at_position, zero_width_location};
use crate::walk::collect_var_refs_in_scope;

/// Find the definition of the symbol under `position`.
/// Searches the current document first, then `other_docs` for cross-file resolution.
pub fn goto_definition(
    uri: &Url,
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[(Url, Arc<ParsedDoc>)],
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
            return Some(Location {
                uri: uri.clone(),
                range: Range {
                    start: sv.position_of(span.start),
                    end: sv.position_of(span.end),
                },
            });
        }
    }

    if let Some(range) = scan_statements(sv, &doc.program().stmts, &word) {
        return Some(Location {
            uri: uri.clone(),
            range,
        });
    }

    for (other_uri, other_doc) in other_docs {
        let other_sv = other_doc.view();
        if let Some(range) = scan_statements(other_sv, &other_doc.program().stmts, &word) {
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
    scan_statements(sv, &doc.program().stmts, name)
}

fn scan_statements(sv: SourceView<'_>, stmts: &[Stmt<'_, '_>], word: &str) -> Option<Range> {
    // Strip a leading `$` so that `$name` matches property names stored without `$`.
    let bare = strip_variable_sigil(word);
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                return Some(sv.name_range_in_span(&f.name.to_string(), stmt.span));
            }
            StmtKind::Class(c)
                if c.name.as_ref().map(|n| n.to_string()) == Some(word.to_string()) =>
            {
                let name = c.name.expect("match guard ensures Some");
                return Some(sv.name_range_in_span(&name.to_string(), stmt.span));
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range_in_span(&m.name.to_string(), member.span));
                        }
                        ClassMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(sv.name_range_in_span(&cc.name.to_string(), member.span));
                        }
                        ClassMemberKind::Property(p) if p.name == bare => {
                            return Some(sv.name_range_in_span(&p.name.to_string(), member.span));
                        }
                        // Constructor-promoted parameters act as property declarations.
                        ClassMemberKind::Method(m) if m.name == "__construct" => {
                            for p in m.params.iter() {
                                if p.visibility.is_some() && p.name == bare {
                                    return Some(
                                        sv.name_range_in_span(&p.name.to_string(), p.span),
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Interface(i) => {
                if i.name == word {
                    return Some(sv.name_range_in_span(&i.name.to_string(), stmt.span));
                }
                for member in i.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(&m.name.to_string()));
                        }
                        ClassMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(sv.name_range(&cc.name.to_string()));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Trait(t) => {
                if t.name == word {
                    return Some(sv.name_range_in_span(&t.name.to_string(), stmt.span));
                }
                for member in t.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(&m.name.to_string()));
                        }
                        ClassMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(sv.name_range(&cc.name.to_string()));
                        }
                        ClassMemberKind::Property(p) if p.name == bare => {
                            return Some(sv.name_range(&p.name.to_string()));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Enum(e) if e.name == word => {
                return Some(sv.name_range_in_span(&e.name.to_string(), stmt.span));
            }
            StmtKind::Enum(e) => {
                for member in e.body.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(&m.name.to_string()));
                        }
                        EnumMemberKind::Case(c) if c.name == word => {
                            return Some(sv.name_range(&c.name.to_string()));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(range) = scan_statements(sv, &inner.stmts, word)
                {
                    return Some(range);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find a class/function declaration by name in a slice of `FileIndex` entries.
/// Returns the URI and a line-level `Range`.
pub fn find_in_indexes(
    name: &str,
    indexes: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::file_index::FileIndex>,
    )],
) -> Option<Location> {
    let bare = strip_variable_sigil(name);
    for (uri, idx) in indexes {
        // Check top-level functions.
        for f in &idx.functions {
            if f.name.as_ref() == bare || f.name.as_ref() == name {
                return Some(zero_width_location(uri, f.start_line));
            }
        }
        // Check classes / interfaces / traits / enums and their members.
        for cls in &idx.classes {
            if cls.name.as_ref() == bare || cls.name.as_ref() == name {
                return Some(zero_width_location(uri, cls.start_line));
            }
            // Methods.
            for m in &cls.methods {
                if m.name.as_ref() == name {
                    return Some(zero_width_location(uri, m.start_line));
                }
            }
            // Properties (stored without `$`).
            for p in &cls.properties {
                if p.name.as_ref() == bare {
                    return Some(zero_width_location(uri, p.start_line));
                }
            }
            // Class constants.
            for cc in &cls.constants {
                if cc.as_ref() == name {
                    return Some(zero_width_location(uri, cls.start_line));
                }
            }
            // Enum cases.
            for case in &cls.cases {
                if case.as_ref() == name {
                    return Some(zero_width_location(uri, cls.start_line));
                }
            }
        }
    }
    None
}

/// Walk the class hierarchy (extends + traits) in the workspace index to find
/// `method_name` defined in `class_name` or any of its superclasses/traits.
///
/// Returns the first match in PHP's resolution order: class itself → traits →
/// parent → parent's traits, etc. Uses `indexes` so no disk I/O is needed.
pub fn find_method_in_class_hierarchy(
    class_name: &str,
    method_name: &str,
    indexes: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::file_index::FileIndex>,
    )],
) -> Option<Location> {
    let mut queue: Vec<String> = vec![class_name.to_owned()];
    let mut visited = std::collections::HashSet::new();

    while !queue.is_empty() {
        let current = queue.remove(0);
        if !visited.insert(current.clone()) {
            continue;
        }
        for (uri, idx) in indexes {
            for cls in &idx.classes {
                if cls.name.as_ref() != current.as_str()
                    && cls.fqn.as_ref().trim_start_matches('\\') != current.as_str()
                {
                    continue;
                }
                for m in &cls.methods {
                    if m.name.as_ref() == method_name {
                        return Some(zero_width_location(uri, m.start_line));
                    }
                }
                // Traits first (PHP MRO), then parent.
                for trt in &cls.traits {
                    queue.push(trt.as_ref().to_owned());
                }
                if let Some(parent) = &cls.parent {
                    queue.push(parent.as_ref().to_owned());
                }
            }
        }
    }
    None
}

fn _name_range_from_offset(sv: SourceView<'_>, name: &str) -> Range {
    let start_offset = str_offset(sv.source(), name).unwrap_or(0);
    let start = sv.position_of(start_offset);
    Range {
        start,
        end: Position {
            line: start.line,
            character: start.character + utf16_code_units(name),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── find_method_in_class_hierarchy ───────────────────────────────────────

    fn make_index(path: &str, src: &str) -> (Url, std::sync::Arc<crate::file_index::FileIndex>) {
        use crate::file_index::FileIndex;
        let u = Url::parse(&format!("file://{path}")).unwrap();
        let d = ParsedDoc::parse(src.to_string());
        (u, std::sync::Arc::new(FileIndex::extract(&d)))
    }

    #[test]
    fn hierarchy_finds_method_in_class_itself() {
        let (uri, idx) = make_index(
            "/a.php",
            "<?php\nclass Foo { public function bar(): void {} }",
        );
        let indexes = vec![(uri, idx)];
        let loc = find_method_in_class_hierarchy("Foo", "bar", &indexes);
        assert!(loc.is_some(), "expected bar() in Foo");
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn hierarchy_finds_method_in_parent() {
        let (base_uri, base_idx) = make_index(
            "/Base.php",
            "<?php\nclass Base { public function render(): void {} }",
        );
        let (cu, ci) = make_index("/Child.php", "<?php\nclass Child extends Base {}");
        let indexes = vec![(base_uri.clone(), base_idx), (cu, ci)];
        let loc = find_method_in_class_hierarchy("Child", "render", &indexes);
        assert!(loc.is_some(), "expected render() found via parent Base");
        assert_eq!(loc.unwrap().uri, base_uri);
    }

    #[test]
    fn hierarchy_finds_method_in_trait() {
        let (trait_uri, trait_idx) = make_index(
            "/Renderable.php",
            "<?php\ntrait Renderable { public function render(): void {} }",
        );
        let (pu, pi) = make_index("/Page.php", "<?php\nclass Page { use Renderable; }");
        let indexes = vec![(trait_uri.clone(), trait_idx), (pu, pi)];
        let loc = find_method_in_class_hierarchy("Page", "render", &indexes);
        assert!(loc.is_some(), "expected render() found via trait");
        assert_eq!(loc.unwrap().uri, trait_uri);
    }

    #[test]
    fn hierarchy_returns_none_for_missing_method() {
        let (uri, idx) = make_index("/Foo.php", "<?php\nclass Foo {}");
        let indexes = vec![(uri, idx)];
        assert!(find_method_in_class_hierarchy("Foo", "missing", &indexes).is_none());
    }

    #[test]
    fn hierarchy_handles_cycle_without_panic() {
        // Bogus source where A extends B extends A — must not loop forever.
        let (ua, ia) = make_index("/A.php", "<?php\nclass A extends B {}");
        let (ub, ib) = make_index("/B.php", "<?php\nclass B extends A {}");
        let indexes = vec![(ua, ia), (ub, ib)];
        let loc = find_method_in_class_hierarchy("A", "missing", &indexes);
        assert!(loc.is_none());
    }
}
