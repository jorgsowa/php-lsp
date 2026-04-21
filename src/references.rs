use std::collections::HashSet;
use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Span, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::ast::{ParsedDoc, str_offset};
use crate::walk::{
    class_refs_in_stmts, function_refs_in_stmts, method_refs_in_stmts, refs_in_stmts,
    refs_in_stmts_with_use,
};

/// What kind of symbol the cursor is on.  Used to dispatch to the
/// appropriate semantic walker so that, e.g., searching for `get` as a
/// *method* doesn't return free-function calls named `get`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// A free (top-level) function.
    Function,
    /// An instance or static method (`->name`, `?->name`, `::name`).
    Method,
    /// A class, interface, trait, or enum name used as a type.
    Class,
}

/// Find all locations where `word` is referenced across the given documents.
/// If `include_declaration` is true, also includes the declaration site.
/// Pass `kind` to restrict results to a particular symbol category; `None`
/// falls back to the original word-based walker (better some results than none).
pub fn find_references(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
    kind: Option<SymbolKind>,
) -> Vec<Location> {
    find_references_inner(word, all_docs, include_declaration, false, kind)
}

/// Like `find_references` but also includes `use` statement spans.
/// Used by rename so that `use Foo;` statements are also updated.
/// Always uses the general walker (rename must update all occurrence kinds).
pub fn find_references_with_use(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
) -> Vec<Location> {
    find_references_inner(word, all_docs, include_declaration, true, None)
}

/// Fast path: look up pre-computed reference locations from the mir codebase index.
///
/// Handles `Function`, `Class`, and (partially) `Method` kinds.  For `Function` and
/// `Class` the mir analyzer records every call-site / instantiation via
/// `mark_*_referenced_at` and the index is authoritative.
///
/// For `Method`, the index is used as a pre-filter: only files that contain a tracked
/// call site for the method are scanned with the AST walker.  This fast path is
/// activated for two cases where the tracked set is reliably complete or narrows the
/// search scope without missing real references:
///   • `private` methods — PHP semantics guarantee that private methods are only
///     callable from within the class body, so mir always resolves the receiver type.
///   • methods on `final` classes — no subclassing means call sites on the concrete
///     type are unambiguous; the codebase set covers all statically-typed callers.
///
/// Returns `None` for public/protected methods on non-final classes and for `None`
/// kind (caller should use the general AST walker instead).  Also returns `None` when
/// no matching symbol is found in the codebase.
pub fn find_references_codebase(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
    kind: Option<SymbolKind>,
    codebase: &mir_codebase::Codebase,
) -> Option<Vec<Location>> {
    // Build a URI-string → (Url, ParsedDoc) map for O(1) lookup.
    let doc_map: std::collections::HashMap<&str, (&Url, &Arc<ParsedDoc>)> = all_docs
        .iter()
        .map(|(url, doc)| (url.as_str(), (url, doc)))
        .collect();

    let spans_to_location = |file: &str, start: u32, end: u32| -> Option<Location> {
        let (url, doc) = doc_map.get(file)?;
        let sv = doc.view();
        let start_pos = sv.position_of(start);
        let end_pos = sv.position_of(end);
        Some(Location {
            uri: (*url).clone(),
            range: Range {
                start: start_pos,
                end: end_pos,
            },
        })
    };

    match kind {
        Some(SymbolKind::Function) => {
            // Collect all FQNs whose short name (last `\`-segment) matches `word`.
            let fqns: Vec<Arc<str>> = codebase
                .functions
                .iter()
                .filter_map(|e| {
                    let fqn = e.key();
                    let short = fqn.rsplit('\\').next().unwrap_or(fqn.as_ref());
                    if short == word {
                        Some(fqn.clone())
                    } else {
                        None
                    }
                })
                .collect();

            if fqns.is_empty() {
                return None;
            }

            let mut locations: Vec<Location> = Vec::new();
            for fqn in &fqns {
                for (file, start, end) in codebase.get_reference_locations(fqn) {
                    if let Some(loc) = spans_to_location(&file, start, end) {
                        locations.push(loc);
                    }
                }
                if include_declaration
                    && let Some(func) = codebase.functions.get(fqn.as_ref())
                    && let Some(decl) = &func.location
                    && let Some(loc) = spans_to_location(&decl.file, decl.start, decl.end)
                {
                    locations.push(loc);
                }
            }
            if locations.is_empty() {
                None
            } else {
                Some(locations)
            }
        }

        Some(SymbolKind::Class) => {
            // Collect all FQCNs whose short name matches `word` across all type maps.
            let mut fqcns: Vec<Arc<str>> = Vec::new();
            let short_matches =
                |fqcn: &Arc<str>| fqcn.rsplit('\\').next().unwrap_or(fqcn.as_ref()) == word;
            for e in codebase.classes.iter() {
                if short_matches(e.key()) {
                    fqcns.push(e.key().clone());
                }
            }
            for e in codebase.interfaces.iter() {
                if short_matches(e.key()) {
                    fqcns.push(e.key().clone());
                }
            }
            for e in codebase.traits.iter() {
                if short_matches(e.key()) {
                    fqcns.push(e.key().clone());
                }
            }
            for e in codebase.enums.iter() {
                if short_matches(e.key()) {
                    fqcns.push(e.key().clone());
                }
            }

            if fqcns.is_empty() {
                return None;
            }

            let mut locations: Vec<Location> = Vec::new();
            for fqcn in &fqcns {
                for (file, start, end) in codebase.get_reference_locations(fqcn) {
                    if let Some(loc) = spans_to_location(&file, start, end) {
                        locations.push(loc);
                    }
                }
                if include_declaration
                    && let Some(decl) = codebase.get_symbol_location(fqcn)
                    && let Some(loc) = spans_to_location(&decl.file, decl.start, decl.end)
                {
                    locations.push(loc);
                }
            }
            if locations.is_empty() {
                None
            } else {
                Some(locations)
            }
        }

        Some(SymbolKind::Method) => {
            let word_lower = word.to_lowercase();

            // Collect method keys (FQCN::lowercase_name) for types where the
            // codebase index is authoritative or a reliable pre-filter.
            let mut method_keys: Vec<String> = Vec::new();
            let mut candidate_arcs: Vec<Arc<str>> = Vec::new();

            for entry in codebase.classes.iter() {
                let cls = entry.value();
                if let Some(method) = cls.own_methods.get(word_lower.as_str())
                    && (cls.is_final || method.visibility == mir_codebase::Visibility::Private)
                {
                    method_keys.push(format!("{}::{}", entry.key(), word_lower));
                    if include_declaration && let Some(loc) = &method.location {
                        candidate_arcs.push(loc.file.clone());
                    }
                }
            }
            for entry in codebase.enums.iter() {
                let enm = entry.value();
                if let Some(method) = enm.own_methods.get(word_lower.as_str())
                    && method.visibility == mir_codebase::Visibility::Private
                {
                    method_keys.push(format!("{}::{}", entry.key(), word_lower));
                    if include_declaration && let Some(loc) = &method.location {
                        candidate_arcs.push(loc.file.clone());
                    }
                }
            }

            if method_keys.is_empty() {
                // No qualifying class/enum found — fall back to the full AST scan.
                return None;
            }

            // Collect candidate files from the reference index (declaration files
            // already appended above so include_declaration=true works correctly).
            for key in &method_keys {
                for (file, _, _) in codebase.get_reference_locations(key) {
                    candidate_arcs.push(file);
                }
            }
            let candidate_uris: HashSet<&str> = candidate_arcs.iter().map(|a| a.as_ref()).collect();

            // Restrict the AST walk to the candidate files only.
            let candidate_docs: Vec<(Url, Arc<ParsedDoc>)> = all_docs
                .iter()
                .filter(|(url, _)| candidate_uris.contains(url.as_str()))
                .cloned()
                .collect();

            let locations = find_references_inner(
                word,
                &candidate_docs,
                include_declaration,
                false,
                Some(SymbolKind::Method),
            );
            Some(locations)
        }

        // General walker already handles None kind; codebase index adds no value.
        None => None,
    }
}

fn find_references_inner(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
    include_use: bool,
    kind: Option<SymbolKind>,
) -> Vec<Location> {
    let mut locations = Vec::new();

    for (uri, doc) in all_docs {
        let source = doc.source();
        let stmts = &doc.program().stmts;
        let mut spans = Vec::new();

        if include_use {
            // Rename path: general walker covers call sites, `use` imports, and declarations.
            refs_in_stmts_with_use(source, stmts, word, &mut spans);
            if !include_declaration {
                let mut decl_spans = Vec::new();
                collect_declaration_spans(source, stmts, word, None, &mut decl_spans);
                let decl_set: HashSet<(u32, u32)> =
                    decl_spans.iter().map(|s| (s.start, s.end)).collect();
                spans.retain(|span| !decl_set.contains(&(span.start, span.end)));
            }
        } else {
            match kind {
                Some(SymbolKind::Function) => function_refs_in_stmts(stmts, word, &mut spans),
                Some(SymbolKind::Method) => method_refs_in_stmts(stmts, word, &mut spans),
                Some(SymbolKind::Class) => class_refs_in_stmts(stmts, word, &mut spans),
                // General walker already includes declarations; filter them out if unwanted.
                None => {
                    refs_in_stmts(source, stmts, word, &mut spans);
                    if !include_declaration {
                        let mut decl_spans = Vec::new();
                        collect_declaration_spans(source, stmts, word, None, &mut decl_spans);
                        let decl_set: HashSet<(u32, u32)> =
                            decl_spans.iter().map(|s| (s.start, s.end)).collect();
                        spans.retain(|span| !decl_set.contains(&(span.start, span.end)));
                    }
                }
            }
            // Typed walkers never emit declaration spans, so add them separately when wanted.
            // Pass `kind` so only declarations of the matching category are appended —
            // a Method search must not return a free-function declaration with the same name.
            if include_declaration && kind.is_some() {
                collect_declaration_spans(source, stmts, word, kind, &mut spans);
            }
        }

        let sv = doc.view();
        for span in spans {
            let start = sv.position_of(span.start);
            let end = Position {
                line: start.line,
                character: start.character
                    + word.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
            };
            locations.push(Location {
                uri: uri.clone(),
                range: Range { start, end },
            });
        }
    }

    locations
}

/// Build a span covering exactly the declared name (not the keyword before it).
fn declaration_name_span(source: &str, name: &str) -> Span {
    let start = str_offset(source, name);
    Span {
        start,
        end: start + name.len() as u32,
    }
}

/// Collect every span where `word` is *declared* within `stmts`.
///
/// When `kind` is `Some`, only declarations of the matching category are collected:
/// - `Function` → free (`StmtKind::Function`) declarations only
/// - `Method`   → method declarations inside classes / traits / enums only
/// - `Class`    → class / interface / trait / enum type declarations only
///
/// `None` collects every declaration kind (used by `is_declaration_span`).
fn collect_declaration_spans(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    word: &str,
    kind: Option<SymbolKind>,
    out: &mut Vec<Span>,
) {
    let want_free = matches!(kind, None | Some(SymbolKind::Function));
    let want_method = matches!(kind, None | Some(SymbolKind::Method));
    let want_type = matches!(kind, None | Some(SymbolKind::Class));

    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                if want_free && f.name == word {
                    out.push(declaration_name_span(source, f.name));
                }
            }
            StmtKind::Class(c) => {
                if want_type
                    && let Some(name) = c.name
                    && name == word
                {
                    out.push(declaration_name_span(source, name));
                }
                if want_method {
                    for member in c.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == word
                        {
                            out.push(declaration_name_span(source, m.name));
                        }
                    }
                }
            }
            StmtKind::Interface(i) => {
                if want_type && i.name == word {
                    out.push(declaration_name_span(source, i.name));
                }
                if want_method {
                    for member in i.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == word
                        {
                            out.push(declaration_name_span(source, m.name));
                        }
                    }
                }
            }
            StmtKind::Trait(t) => {
                if want_type && t.name == word {
                    out.push(declaration_name_span(source, t.name));
                }
                if want_method {
                    for member in t.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == word
                        {
                            out.push(declaration_name_span(source, m.name));
                        }
                    }
                }
            }
            StmtKind::Enum(e) => {
                if want_type && e.name == word {
                    out.push(declaration_name_span(source, e.name));
                }
                for member in e.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Method(m) if want_method && m.name == word => {
                            out.push(declaration_name_span(source, m.name));
                        }
                        EnumMemberKind::Case(c) if want_type && c.name == word => {
                            out.push(declaration_name_span(source, c.name));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_declaration_spans(source, inner, word, kind, out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(path: &str, source: &str) -> (Url, Arc<ParsedDoc>) {
        (uri(path), Arc::new(ParsedDoc::parse(source.to_string())))
    }

    #[test]
    fn finds_function_call_reference() {
        let src = "<?php\nfunction greet() {}\ngreet();\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, false, None);
        assert_eq!(refs.len(), 2, "expected 2 call-site refs, got {:?}", refs);
    }

    #[test]
    fn include_declaration_adds_def_site() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let with_decl = find_references("greet", &docs, true, None);
        let without_decl = find_references("greet", &docs, false, None);
        // Without declaration: only the call site (line 2)
        assert_eq!(
            without_decl.len(),
            1,
            "expected 1 call-site ref without declaration"
        );
        assert_eq!(
            without_decl[0].range.start.line, 2,
            "call site should be on line 2"
        );
        // With declaration: 2 refs total (decl on line 1, call on line 2)
        assert_eq!(
            with_decl.len(),
            2,
            "expected 2 refs with declaration included"
        );
    }

    #[test]
    fn finds_new_expression_reference() {
        let src = "<?php\nclass Foo {}\n$x = new Foo();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Foo", &docs, false, None);
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 reference to Foo in new expr"
        );
        assert_eq!(
            refs[0].range.start.line, 2,
            "new Foo() reference should be on line 2"
        );
    }

    #[test]
    fn finds_reference_in_nested_function_call() {
        let src = "<?php\nfunction greet() {}\necho(greet());";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, false, None);
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 nested function call reference"
        );
        assert_eq!(
            refs[0].range.start.line, 2,
            "nested greet() call should be on line 2"
        );
    }

    #[test]
    fn finds_references_across_multiple_docs() {
        let a = doc("/a.php", "<?php\nfunction helper() {}");
        let b = doc("/b.php", "<?php\nhelper();\nhelper();");
        let refs = find_references("helper", &[a, b], false, None);
        assert_eq!(refs.len(), 2, "expected 2 cross-file references");
        assert!(refs.iter().all(|r| r.uri.path().ends_with("/b.php")));
    }

    #[test]
    fn finds_method_call_reference() {
        let src = "<?php\nclass Calc { public function add() {} }\n$c = new Calc();\n$c->add();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("add", &docs, false, None);
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 method call reference to 'add'"
        );
        assert_eq!(
            refs[0].range.start.line, 3,
            "add() call should be on line 3"
        );
    }

    #[test]
    fn finds_reference_inside_if_body() {
        let src = "<?php\nfunction check() {}\nif (true) { check(); }";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("check", &docs, false, None);
        assert_eq!(refs.len(), 1, "expected exactly 1 reference inside if body");
        assert_eq!(
            refs[0].range.start.line, 2,
            "check() inside if should be on line 2"
        );
    }

    #[test]
    fn finds_use_statement_reference() {
        // Renaming MyClass — the `use MyClass;` statement should be in the results
        // when using find_references_with_use.
        let src = "<?php\nuse MyClass;\n$x = new MyClass();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references_with_use("MyClass", &docs, false);
        // Exactly 2 references: the `use MyClass;` on line 1 and `new MyClass()` on line 2.
        assert_eq!(
            refs.len(),
            2,
            "expected exactly 2 references, got: {:?}",
            refs
        );
        let mut lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            vec![1, 2],
            "references should be on lines 1 (use) and 2 (new)"
        );
    }

    #[test]
    fn find_references_returns_correct_lines() {
        // `helper` is called on lines 1 and 2 (0-based); check exact line numbers.
        let src = "<?php\nhelper();\nhelper();\nfunction helper() {}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("helper", &docs, false, None);
        assert_eq!(refs.len(), 2, "expected exactly 2 call-site references");
        let mut lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        lines.sort_unstable();
        assert_eq!(lines, vec![1, 2], "references should be on lines 1 and 2");
    }

    #[test]
    fn declaration_excluded_when_flag_false() {
        // When include_declaration=false the declaration line must not appear.
        let src = "<?php\nfunction doWork() {}\ndoWork();\ndoWork();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("doWork", &docs, false, None);
        // Declaration is on line 1; call sites are on lines 2 and 3.
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            !lines.contains(&1),
            "declaration line (1) must not appear when include_declaration=false, got: {:?}",
            lines
        );
        assert_eq!(refs.len(), 2, "expected 2 call-site references only");
    }

    #[test]
    fn partial_match_not_included() {
        // Searching for references to `greet` should NOT include occurrences of `greeting`.
        let src = "<?php\nfunction greet() {}\nfunction greeting() {}\ngreet();\ngreeting();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, false, None);
        // Only `greet()` call site should be included, not `greeting()`.
        for r in &refs {
            // Each reference range should span exactly the length of "greet" (5 chars),
            // not longer (which would indicate "greeting" was matched).
            let span_len = r.range.end.character - r.range.start.character;
            assert_eq!(
                span_len, 5,
                "reference span length should equal len('greet')=5, got {} at {:?}",
                span_len, r
            );
        }
        // There should be exactly 1 call-site reference (the greet() call, not greeting()).
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 reference to 'greet' (not 'greeting'), got: {:?}",
            refs
        );
    }

    #[test]
    fn finds_reference_in_class_property_default() {
        // A class constant used as a property default value should be found by find_references.
        let src = "<?php\nclass Foo {\n    public string $status = Status::ACTIVE;\n}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Status", &docs, false, None);
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 reference to Status in property default, got: {:?}",
            refs
        );
        assert_eq!(refs[0].range.start.line, 2, "reference should be on line 2");
    }

    #[test]
    fn class_const_access_span_covers_only_member_name() {
        // Searching for the constant name `ACTIVE` in `Status::ACTIVE` must highlight
        // only `ACTIVE`, not the whole `Status::ACTIVE` expression.
        // Line 0: <?php
        // Line 1: $x = Status::ACTIVE;
        //                       ^ character 13
        let src = "<?php\n$x = Status::ACTIVE;";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("ACTIVE", &docs, false, None);
        assert_eq!(refs.len(), 1, "expected 1 reference, got: {:?}", refs);
        let r = &refs[0].range;
        assert_eq!(r.start.line, 1, "reference must be on line 1");
        // "$x = Status::" is 13 chars; "ACTIVE" starts at character 13.
        // Before the fix this was 5 (the start of "Status"), not 13.
        assert_eq!(
            r.start.character, 13,
            "range must start at 'ACTIVE' (char 13), not at 'Status' (char 5); got {:?}",
            r
        );
    }

    #[test]
    fn class_const_access_no_duplicate_when_name_equals_class() {
        // Edge case: enum case named the same as the enum itself — `Status::Status`.
        // The general walker finds two distinct references:
        //   - the class-side `Status` at character 5  ($x = [S]tatus::Status)
        //   - the member-side `Status` at character 13 ($x = Status::[S]tatus)
        // Before the fix, both pushed a span starting at character 5, producing a duplicate.
        // Line 0: <?php
        // Line 1: $x = Status::Status;
        //              ^    char 5 (class)
        //                       ^ char 13 (member)
        let src = "<?php\n$x = Status::Status;";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Status", &docs, false, None);
        assert_eq!(
            refs.len(),
            2,
            "expected exactly 2 refs (class side + member side), got: {:?}",
            refs
        );
        let mut chars: Vec<u32> = refs.iter().map(|r| r.range.start.character).collect();
        chars.sort_unstable();
        assert_eq!(
            chars,
            vec![5, 13],
            "class-side ref must be at char 5 and member-side at char 13, got: {:?}",
            refs
        );
    }

    #[test]
    fn finds_reference_inside_enum_method_body() {
        // A function call inside an enum method body should be found by find_references.
        let src = "<?php\nfunction helper() {}\nenum Status {\n    public function label(): string { return helper(); }\n}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("helper", &docs, false, None);
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 reference to helper() inside enum method, got: {:?}",
            refs
        );
        assert_eq!(refs[0].range.start.line, 3, "reference should be on line 3");
    }

    #[test]
    fn finds_reference_in_for_init_and_update() {
        // Function calls in `for` init and update expressions should be found.
        let src = "<?php\nfunction tick() {}\nfor (tick(); $i < 10; tick()) {}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("tick", &docs, false, None);
        assert_eq!(
            refs.len(),
            2,
            "expected exactly 2 references to tick() (init + update), got: {:?}",
            refs
        );
        // Both are on line 2.
        assert!(refs.iter().all(|r| r.range.start.line == 2));
    }

    // ── Semantic (kind-aware) tests ───────────────────────────────────────────

    #[test]
    fn function_kind_skips_method_call_with_same_name() {
        // When looking for the free function `get`, method calls `$obj->get()` must be excluded.
        let src = "<?php\nfunction get() {}\nget();\n$obj->get();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("get", &docs, false, Some(SymbolKind::Function));
        // Only the free call `get()` on line 2 should appear; not the method call on line 3.
        assert_eq!(
            refs.len(),
            1,
            "expected 1 free-function ref, got: {:?}",
            refs
        );
        assert_eq!(refs[0].range.start.line, 2);
    }

    #[test]
    fn method_kind_skips_free_function_call_with_same_name() {
        // When looking for the method `add`, the free function call `add()` must be excluded.
        let src = "<?php\nfunction add() {}\nadd();\n$calc->add();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("add", &docs, false, Some(SymbolKind::Method));
        // Only the method call on line 3 should appear.
        assert_eq!(refs.len(), 1, "expected 1 method ref, got: {:?}", refs);
        assert_eq!(refs[0].range.start.line, 3);
    }

    #[test]
    fn class_kind_finds_new_expression() {
        // SymbolKind::Class should find `new Foo()` but not a free function call `Foo()`.
        let src = "<?php\nclass Foo {}\n$x = new Foo();\nFoo();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Foo", &docs, false, Some(SymbolKind::Class));
        // `new Foo()` on line 2 yes; `Foo()` on line 3 should NOT appear as a class ref.
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&2),
            "expected new Foo() on line 2, got: {:?}",
            refs
        );
        assert!(
            !lines.contains(&3),
            "free call Foo() should not appear as class ref, got: {:?}",
            refs
        );
    }

    #[test]
    fn class_kind_finds_extends_and_implements() {
        let src = "<?php\nclass Base {}\ninterface Iface {}\nclass Child extends Base implements Iface {}";
        let docs = vec![doc("/a.php", src)];

        let base_refs = find_references("Base", &docs, false, Some(SymbolKind::Class));
        let lines_base: Vec<u32> = base_refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines_base.contains(&3),
            "expected extends Base on line 3, got: {:?}",
            base_refs
        );

        let iface_refs = find_references("Iface", &docs, false, Some(SymbolKind::Class));
        let lines_iface: Vec<u32> = iface_refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines_iface.contains(&3),
            "expected implements Iface on line 3, got: {:?}",
            iface_refs
        );
    }

    #[test]
    fn class_kind_finds_type_hint() {
        // SymbolKind::Class should find `Foo` as a parameter type hint.
        let src = "<?php\nclass Foo {}\nfunction take(Foo $x): void {}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Foo", &docs, false, Some(SymbolKind::Class));
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&2),
            "expected type hint Foo on line 2, got: {:?}",
            refs
        );
    }

    // ── Declaration span precision tests ────────────────────────────────────────

    #[test]
    fn function_declaration_span_points_to_name_not_keyword() {
        // `include_declaration: true` — the declaration ref must start at `greet`,
        // not at the `function` keyword.
        let src = "<?php\nfunction greet() {}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, true, None);
        assert_eq!(refs.len(), 1, "expected exactly 1 ref (the declaration)");
        // "function " is 9 bytes; "greet" starts at byte 15 (after "<?php\n").
        // As a position, line 1, character 9.
        assert_eq!(
            refs[0].range.start.line, 1,
            "declaration should be on line 1"
        );
        assert_eq!(
            refs[0].range.start.character, 9,
            "declaration should start at the function name, not the 'function' keyword"
        );
        assert_eq!(
            refs[0].range.end.character,
            refs[0].range.start.character
                + "greet".chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
            "range should span exactly the function name"
        );
    }

    #[test]
    fn class_declaration_span_points_to_name_not_keyword() {
        let src = "<?php\nclass MyClass {}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("MyClass", &docs, true, None);
        assert_eq!(refs.len(), 1);
        // "class " is 6 bytes; "MyClass" starts at character 6.
        assert_eq!(refs[0].range.start.line, 1);
        assert_eq!(
            refs[0].range.start.character, 6,
            "declaration should start at 'MyClass', not 'class'"
        );
    }

    #[test]
    fn method_declaration_span_points_to_name_not_keyword() {
        let src = "<?php\nclass C {\n    public function doThing() {}\n}\n(new C())->doThing();";
        let docs = vec![doc("/a.php", src)];
        // include_declaration=true so we get the method declaration too.
        let refs = find_references("doThing", &docs, true, None);
        // Declaration on line 2, call on line 4.
        let decl_ref = refs
            .iter()
            .find(|r| r.range.start.line == 2)
            .expect("no declaration ref on line 2");
        // "    public function " is 20 chars; "doThing" starts at character 20.
        assert_eq!(
            decl_ref.range.start.character, 20,
            "method declaration should start at the method name, not 'public function'"
        );
    }

    #[test]
    fn method_kind_with_include_declaration_does_not_return_free_function() {
        // Regression: kind precision must be preserved even when include_declaration=true.
        // A free function `get` and a method `get` coexist; searching with
        // SymbolKind::Method must NOT return either the free function call or its declaration.
        //
        // Line 0: <?php
        // Line 1: function get() {}          ← free function declaration
        // Line 2: get();                     ← free function call
        // Line 3: class C { public function get() {} }  ← method declaration
        // Line 4: $c->get();                 ← method call
        let src =
            "<?php\nfunction get() {}\nget();\nclass C { public function get() {} }\n$c->get();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("get", &docs, true, Some(SymbolKind::Method));
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&3),
            "method declaration (line 3) must be present, got: {:?}",
            lines
        );
        assert!(
            lines.contains(&4),
            "method call (line 4) must be present, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&1),
            "free function declaration (line 1) must not appear when kind=Method, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&2),
            "free function call (line 2) must not appear when kind=Method, got: {:?}",
            lines
        );
    }

    #[test]
    fn function_kind_with_include_declaration_does_not_return_method_call() {
        // Symmetric: SymbolKind::Function + include_declaration=true must not return method
        // calls or method declarations with the same name.
        //
        // Line 0: <?php
        // Line 1: function add() {}          ← free function declaration
        // Line 2: add();                     ← free function call
        // Line 3: class C { public function add() {} }  ← method declaration
        // Line 4: $c->add();                 ← method call
        let src =
            "<?php\nfunction add() {}\nadd();\nclass C { public function add() {} }\n$c->add();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("add", &docs, true, Some(SymbolKind::Function));
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&1),
            "function declaration (line 1) must be present, got: {:?}",
            lines
        );
        assert!(
            lines.contains(&2),
            "function call (line 2) must be present, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&3),
            "method declaration (line 3) must not appear when kind=Function, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&4),
            "method call (line 4) must not appear when kind=Function, got: {:?}",
            lines
        );
    }

    #[test]
    fn interface_method_declaration_included_when_flag_true() {
        // Regression: collect_declaration_spans must cover interface members, not only
        // classes/traits/enums. When include_declaration=true and kind=Method the
        // abstract method stub inside the interface must appear.
        //
        // Line 0: <?php
        // Line 1: interface I {
        // Line 2:     public function add(): void;   ← interface method declaration
        // Line 3: }
        // Line 4: $obj->add();                        ← call site
        let src = "<?php\ninterface I {\n    public function add(): void;\n}\n$obj->add();";
        let docs = vec![doc("/a.php", src)];

        let refs = find_references("add", &docs, true, Some(SymbolKind::Method));
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&2),
            "interface method declaration (line 2) must appear with include_declaration=true, got: {:?}",
            lines
        );
        assert!(
            lines.contains(&4),
            "call site (line 4) must appear, got: {:?}",
            lines
        );

        // With include_declaration=false only the call site should remain.
        let refs_no_decl = find_references("add", &docs, false, Some(SymbolKind::Method));
        let lines_no_decl: Vec<u32> = refs_no_decl.iter().map(|r| r.range.start.line).collect();
        assert!(
            !lines_no_decl.contains(&2),
            "interface method declaration must be excluded when include_declaration=false, got: {:?}",
            lines_no_decl
        );
    }

    #[test]
    fn declaration_filter_finds_method_inside_same_named_class() {
        // Edge case: a class named `get` contains a method also named `get`.
        // collect_declaration_spans(kind=None) must find BOTH the class declaration
        // and the method declaration so is_declaration_span correctly filters both
        // when include_declaration=false.
        //
        // Line 0: <?php
        // Line 1: class get { public function get() {} }
        // Line 2: $obj->get();
        let src = "<?php\nclass get { public function get() {} }\n$obj->get();";
        let docs = vec![doc("/a.php", src)];

        // With include_declaration=false, neither the class name nor the method
        // declaration should appear — only the call site on line 2.
        let refs = find_references("get", &docs, false, None);
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            !lines.contains(&1),
            "declaration line (1) must not appear when include_declaration=false, got: {:?}",
            lines
        );
        assert!(
            lines.contains(&2),
            "call site (line 2) must be present, got: {:?}",
            lines
        );

        // With include_declaration=true, the class declaration AND method declaration
        // are both on line 1; the call site is on line 2.
        let refs_with = find_references("get", &docs, true, None);
        assert_eq!(
            refs_with.len(),
            3,
            "expected 3 refs (class decl + method decl + call), got: {:?}",
            refs_with
        );
    }

    #[test]
    fn interface_method_declaration_included_with_kind_none() {
        // Regression: the general walker must emit interface method name spans so that
        // kind=None + include_declaration=true returns the declaration, matching the
        // behaviour already present for class and trait methods.
        //
        // Line 0: <?php
        // Line 1: interface I {
        // Line 2:     public function add(): void;   ← declaration
        // Line 3: }
        // Line 4: $obj->add();                        ← call site
        let src = "<?php\ninterface I {\n    public function add(): void;\n}\n$obj->add();";
        let docs = vec![doc("/a.php", src)];

        let refs = find_references("add", &docs, true, None);
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&2),
            "interface method declaration (line 2) must appear with kind=None + include_declaration=true, got: {:?}",
            lines
        );
    }

    #[test]
    fn interface_method_declaration_excluded_with_kind_none_flag_false() {
        // Counterpart to interface_method_declaration_included_with_kind_none.
        // is_declaration_span calls collect_declaration_spans(kind=None), which after
        // the fix now emits interface method name spans. Verify that
        // include_declaration=false correctly suppresses the declaration.
        //
        // Line 0: <?php
        // Line 1: interface I {
        // Line 2:     public function add(): void;   ← declaration — must be absent
        // Line 3: }
        // Line 4: $obj->add();                        ← call site — must be present
        let src = "<?php\ninterface I {\n    public function add(): void;\n}\n$obj->add();";
        let docs = vec![doc("/a.php", src)];

        let refs = find_references("add", &docs, false, None);
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            !lines.contains(&2),
            "interface method declaration (line 2) must be excluded with kind=None + include_declaration=false, got: {:?}",
            lines
        );
        assert!(
            lines.contains(&4),
            "call site (line 4) must be present, got: {:?}",
            lines
        );
    }

    #[test]
    fn function_kind_does_not_include_interface_method_declaration() {
        // kind=Function must not return interface method declarations. The existing
        // function_kind_with_include_declaration_does_not_return_method_call test
        // covers class methods; this covers the interface case specifically.
        //
        // Line 0: <?php
        // Line 1: function add() {}              ← free function declaration
        // Line 2: add();                         ← free function call
        // Line 3: interface I {
        // Line 4:     public function add(): void;  ← interface method — must be absent
        // Line 5: }
        let src =
            "<?php\nfunction add() {}\nadd();\ninterface I {\n    public function add(): void;\n}";
        let docs = vec![doc("/a.php", src)];

        let refs = find_references("add", &docs, true, Some(SymbolKind::Function));
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&1),
            "free function declaration (line 1) must be present, got: {:?}",
            lines
        );
        assert!(
            lines.contains(&2),
            "free function call (line 2) must be present, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&4),
            "interface method declaration (line 4) must not appear with kind=Function, got: {:?}",
            lines
        );
    }

    // ── switch / throw / unset / property-default coverage ──────────────────

    #[test]
    fn finds_function_call_inside_switch_case() {
        // Line 1: function tick() {}
        // Line 2: switch ($x) { case 1: tick(); break; }
        let src = "<?php\nfunction tick() {}\nswitch ($x) {\n    case 1: tick(); break;\n}";
        let docs = vec![doc("/a.php", src)];
        let lines: Vec<u32> = find_references("tick", &docs, false, Some(SymbolKind::Function))
            .iter()
            .map(|r| r.range.start.line)
            .collect();
        assert!(
            lines.contains(&3),
            "tick() call inside switch case (line 3) must be present, got: {:?}",
            lines
        );
    }

    #[test]
    fn finds_method_call_inside_switch_case() {
        // Line 1: switch ($x) { case 1: $obj->process(); break; }
        let src = "<?php\nswitch ($x) {\n    case 1: $obj->process(); break;\n}";
        let docs = vec![doc("/a.php", src)];
        let lines: Vec<u32> = find_references("process", &docs, false, Some(SymbolKind::Method))
            .iter()
            .map(|r| r.range.start.line)
            .collect();
        assert!(
            lines.contains(&2),
            "process() call inside switch case (line 2) must be present, got: {:?}",
            lines
        );
    }

    #[test]
    fn finds_function_call_inside_switch_condition() {
        // Line 1: function classify() {}
        // Line 2: switch (classify()) { default: break; }
        let src = "<?php\nfunction classify() {}\nswitch (classify()) { default: break; }";
        let docs = vec![doc("/a.php", src)];
        let lines: Vec<u32> = find_references("classify", &docs, false, Some(SymbolKind::Function))
            .iter()
            .map(|r| r.range.start.line)
            .collect();
        assert!(
            lines.contains(&2),
            "classify() in switch subject (line 2) must be present, got: {:?}",
            lines
        );
    }

    #[test]
    fn finds_function_call_inside_throw() {
        // Line 1: function makeException() {}
        // Line 2: throw makeException();
        let src = "<?php\nfunction makeException() {}\nthrow makeException();";
        let docs = vec![doc("/a.php", src)];
        let lines: Vec<u32> =
            find_references("makeException", &docs, false, Some(SymbolKind::Function))
                .iter()
                .map(|r| r.range.start.line)
                .collect();
        assert!(
            lines.contains(&2),
            "makeException() inside throw (line 2) must be present, got: {:?}",
            lines
        );
    }

    #[test]
    fn finds_method_call_inside_throw() {
        // Line 1: throw $factory->create();
        let src = "<?php\nthrow $factory->create();";
        let docs = vec![doc("/a.php", src)];
        let lines: Vec<u32> = find_references("create", &docs, false, Some(SymbolKind::Method))
            .iter()
            .map(|r| r.range.start.line)
            .collect();
        assert!(
            lines.contains(&1),
            "create() inside throw (line 1) must be present, got: {:?}",
            lines
        );
    }

    #[test]
    fn finds_method_call_inside_unset() {
        // Line 1: unset($obj->getProp());
        let src = "<?php\nunset($obj->getProp());";
        let docs = vec![doc("/a.php", src)];
        let lines: Vec<u32> = find_references("getProp", &docs, false, Some(SymbolKind::Method))
            .iter()
            .map(|r| r.range.start.line)
            .collect();
        assert!(
            lines.contains(&1),
            "getProp() inside unset (line 1) must be present, got: {:?}",
            lines
        );
    }

    #[test]
    fn finds_static_method_call_in_class_property_default() {
        // Line 1: class Config {
        // Line 2:     public array $data = self::defaults();
        // Line 3:     public static function defaults(): array { return []; }
        // Line 4: }
        let src = "<?php\nclass Config {\n    public array $data = self::defaults();\n    public static function defaults(): array { return []; }\n}";
        let docs = vec![doc("/a.php", src)];
        let lines: Vec<u32> = find_references("defaults", &docs, false, Some(SymbolKind::Method))
            .iter()
            .map(|r| r.range.start.line)
            .collect();
        assert!(
            lines.contains(&2),
            "defaults() in class property default (line 2) must be present, got: {:?}",
            lines
        );
    }

    #[test]
    fn finds_static_method_call_in_trait_property_default() {
        // Line 1: trait T {
        // Line 2:     public int $x = self::init();
        // Line 3:     public static function init(): int { return 0; }
        // Line 4: }
        let src = "<?php\ntrait T {\n    public int $x = self::init();\n    public static function init(): int { return 0; }\n}";
        let docs = vec![doc("/a.php", src)];
        let lines: Vec<u32> = find_references("init", &docs, false, Some(SymbolKind::Method))
            .iter()
            .map(|r| r.range.start.line)
            .collect();
        assert!(
            lines.contains(&2),
            "init() in trait property default (line 2) must be present, got: {:?}",
            lines
        );
    }

    // ── find_references_codebase: Method fast-path ──────────────────────────

    fn make_class(
        fqcn: &str,
        is_final: bool,
        method_name: &str,
        visibility: mir_codebase::Visibility,
    ) -> mir_codebase::ClassStorage {
        use indexmap::IndexMap;
        let method = mir_codebase::MethodStorage {
            name: std::sync::Arc::from(method_name),
            fqcn: std::sync::Arc::from(fqcn),
            params: vec![],
            return_type: None,
            inferred_return_type: None,
            visibility,
            is_static: false,
            is_abstract: false,
            is_final: false,
            is_constructor: false,
            template_params: vec![],
            assertions: vec![],
            throws: vec![],
            is_deprecated: false,
            is_internal: false,
            is_pure: false,
            location: None,
        };
        let mut methods: IndexMap<
            std::sync::Arc<str>,
            std::sync::Arc<mir_codebase::MethodStorage>,
        > = IndexMap::new();
        // own_methods keys are lowercase (PHP method names are case-insensitive).
        methods.insert(
            std::sync::Arc::from(method_name.to_lowercase().as_str()),
            std::sync::Arc::new(method),
        );
        mir_codebase::ClassStorage {
            fqcn: std::sync::Arc::from(fqcn),
            short_name: std::sync::Arc::from(fqcn.rsplit('\\').next().unwrap_or(fqcn)),
            parent: None,
            interfaces: vec![],
            traits: vec![],
            own_methods: methods,
            own_properties: IndexMap::new(),
            own_constants: IndexMap::new(),
            template_params: vec![],
            is_abstract: false,
            is_final,
            is_readonly: false,
            all_parents: vec![],
            is_deprecated: false,
            is_internal: false,
            location: None,
        }
    }

    #[test]
    fn codebase_method_falls_back_for_public_method_on_nonfinal_class() {
        // Public method on a non-final class: no fast path → None → full AST scan.
        let cb = mir_codebase::Codebase::new();
        cb.classes.insert(
            std::sync::Arc::from("Foo"),
            make_class("Foo", false, "process", mir_codebase::Visibility::Public),
        );
        cb.mark_method_referenced_at(
            "Foo",
            "process",
            std::sync::Arc::from("file:///a.php"),
            10,
            17,
        );

        let src = "<?php\nclass Foo { public function process() {} }\n$foo->process();";
        let docs = vec![doc("/a.php", src)];
        let result =
            find_references_codebase("process", &docs, false, Some(SymbolKind::Method), &cb);
        assert!(
            result.is_none(),
            "public method on non-final class must return None (fall back to AST), got: {:?}",
            result
        );
    }

    #[test]
    fn codebase_method_fast_path_private_method_filters_files() {
        // Private method: only files tracked in the codebase index are scanned.
        // File b.php has a same-named call but is not in the codebase index —
        // it must be excluded, proving the fast path is active.
        let cb = mir_codebase::Codebase::new();
        cb.classes.insert(
            std::sync::Arc::from("Foo"),
            make_class("Foo", false, "execute", mir_codebase::Visibility::Private),
        );
        // Only a.php is tracked.
        cb.mark_method_referenced_at(
            "Foo",
            "execute",
            std::sync::Arc::from("file:///a.php"),
            10,
            17,
        );

        // a.php: Foo with private execute + a call to $this->execute() inside the class.
        let src_a = "<?php\nclass Foo {\n    private function execute() {}\n    public function run() { $this->execute(); }\n}";
        // b.php: also calls ->execute() but is NOT in the codebase index.
        let src_b = "<?php\n$other->execute();";

        let docs = vec![doc("/a.php", src_a), doc("/b.php", src_b)];
        let result =
            find_references_codebase("execute", &docs, false, Some(SymbolKind::Method), &cb);

        assert!(
            result.is_some(),
            "private method must activate the fast path"
        );
        let locs = result.unwrap();

        let uris: Vec<&str> = locs.iter().map(|l| l.uri.as_str()).collect();
        assert!(
            uris.iter().all(|u| u.ends_with("/a.php")),
            "all results must be from a.php (b.php was not in the codebase index), got: {:?}",
            locs
        );
        assert!(
            !locs.is_empty(),
            "expected at least the $this->execute() call in a.php, got: {:?}",
            locs
        );
    }

    #[test]
    fn codebase_method_fast_path_final_class_filters_files() {
        // Final class: method is on a final class, so the fast path applies.
        // File b.php is not tracked → excluded.
        let cb = mir_codebase::Codebase::new();
        cb.classes.insert(
            std::sync::Arc::from("Counter"),
            make_class(
                "Counter",
                true, // is_final
                "increment",
                mir_codebase::Visibility::Public,
            ),
        );
        cb.mark_method_referenced_at(
            "Counter",
            "increment",
            std::sync::Arc::from("file:///a.php"),
            10,
            19,
        );

        let src_a = "<?php\nfinal class Counter {\n    public function increment() {}\n}\n$c = new Counter();\n$c->increment();";
        let src_b = "<?php\n$other->increment();";

        let docs = vec![doc("/a.php", src_a), doc("/b.php", src_b)];
        let result =
            find_references_codebase("increment", &docs, false, Some(SymbolKind::Method), &cb);

        assert!(
            result.is_some(),
            "final class method must activate the fast path"
        );
        let locs = result.unwrap();

        let uris: Vec<&str> = locs.iter().map(|l| l.uri.as_str()).collect();
        assert!(
            uris.iter().all(|u| u.ends_with("/a.php")),
            "all results must be from a.php only, got: {:?}",
            locs
        );
    }

    #[test]
    fn codebase_method_fast_path_cross_file_reference() {
        // Realistic cross-file scenario: class defined in class.php, called from
        // caller.php and ignored.php (not tracked).
        // The fast path must include caller.php (tracked) and exclude ignored.php.
        let cb = mir_codebase::Codebase::new();
        cb.classes.insert(
            std::sync::Arc::from("Order"),
            make_class(
                "Order",
                true, // is_final
                "submit",
                mir_codebase::Visibility::Public,
            ),
        );
        // The codebase tracks caller.php as referencing Order::submit.
        cb.mark_method_referenced_at(
            "Order",
            "submit",
            std::sync::Arc::from("file:///caller.php"),
            50,
            56,
        );

        // class.php: defines the final class (no calls here).
        let src_class = "<?php\nfinal class Order {\n    public function submit() {}\n}";
        // caller.php: calls $order->submit() — tracked in codebase.
        let src_caller = "<?php\n$order = new Order();\n$order->submit();";
        // ignored.php: also calls ->submit() on an unknown type — NOT tracked.
        let src_ignored = "<?php\n$unknown->submit();";

        let docs = vec![
            doc("/class.php", src_class),
            doc("/caller.php", src_caller),
            doc("/ignored.php", src_ignored),
        ];

        let result =
            find_references_codebase("submit", &docs, false, Some(SymbolKind::Method), &cb);

        assert!(result.is_some(), "fast path must activate for final class");
        let locs = result.unwrap();

        let uris: Vec<&str> = locs.iter().map(|l| l.uri.as_str()).collect();
        assert!(
            uris.iter().any(|u| u.ends_with("/caller.php")),
            "caller.php (tracked) must appear in results, got: {:?}",
            locs
        );
        assert!(
            !uris.iter().any(|u| u.ends_with("/ignored.php")),
            "ignored.php (not tracked) must be excluded, got: {:?}",
            locs
        );
    }

    #[test]
    fn codebase_method_fast_path_empty_codebase_falls_back() {
        // Empty codebase: no qualifying class found → None → caller falls back to full AST.
        let cb = mir_codebase::Codebase::new();
        let src = "<?php\n$obj->doWork();";
        let docs = vec![doc("/a.php", src)];
        let result =
            find_references_codebase("doWork", &docs, false, Some(SymbolKind::Method), &cb);
        assert!(
            result.is_none(),
            "empty codebase must return None for Method kind, got: {:?}",
            result
        );
    }
}
