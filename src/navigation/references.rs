use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::Arc;

use php_ast::visitor::{Visitor, walk_stmt};
use php_ast::{
    ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Span, Stmt, StmtKind, UseKind,
};
use rayon::prelude::*;
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::ast::{ParsedDoc, str_offset_in_range};
use crate::util::{fqn_short_name, utf16_code_units};
use crate::walk::{
    all_class_ref_names_in_stmts, class_refs_in_stmts, constant_refs_in_stmts,
    fqn_new_class_refs_in_stmts, function_refs_in_stmts, global_constant_refs_in_stmts,
    method_refs_in_stmts, new_refs_in_stmts, property_refs_in_stmts, refs_in_stmts,
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
    /// A class / trait property (`->name`, `?->name`, promoted or declared).
    Property,
    /// A class, interface, enum, or trait constant (`Class::CONST`, `self::CONST`).
    Constant,
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
    find_references_inner(word, all_docs, include_declaration, false, kind, None)
}

/// Like [`find_references`] but narrows scanning to docs whose namespace +
/// `use` imports would resolve `word` to `target_fqn`. Used by
/// `textDocument/references` for the AST fallback so it doesn't match
/// same-short-name symbols in unrelated namespaces.
pub fn find_references_with_target(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
    kind: Option<SymbolKind>,
    target_fqn: &str,
) -> Vec<Location> {
    // Default: include `use` statement spans so callers that pass
    // `kind=None` (notably the rename handler) get their use-import edits.
    // For typed kinds we want the kind-specific walker (so a Method search
    // doesn't pick up free functions sharing the name); the general walker
    // would falsely widen those results.
    let include_use = kind.is_none();
    find_references_inner(
        word,
        all_docs,
        include_declaration,
        include_use,
        kind,
        Some(target_fqn),
    )
}

/// Like `find_references` but also includes `use` statement spans.
/// Used by rename so that `use Foo;` statements are also updated.
/// Always uses the general walker (rename must update all occurrence kinds).
pub fn find_references_with_use(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
) -> Vec<Location> {
    find_references_inner(word, all_docs, include_declaration, true, None, None)
}

/// Find only `new ClassName(...)` instantiation sites across all docs.
///
/// Used by the `__construct` references handler — `SymbolKind::Class` (the normal
/// class-kind path) is too broad because mir's `ClassReference` key covers type
/// hints, `instanceof`, `extends`, and `implements` in addition to `new` calls.
/// This function walks the AST using `new_refs_in_stmts` which only emits spans
/// for `ExprKind::New` nodes, giving the caller exactly the call sites.
///
/// `class_fqn` is the fully-qualified name (e.g. `"Alpha\\Widget"`) used to
/// filter files where the short name resolves to a different class. Pass `None`
/// for global-namespace classes.
pub fn find_constructor_references(
    short_name: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    class_fqn: Option<&str>,
) -> Vec<Location> {
    all_docs
        .par_iter()
        .flat_map_iter(|(uri, doc)| {
            // Cheap memchr gate before import AST walk.
            if !doc.view().source().contains(short_name)
                && !class_fqn
                    .is_some_and(|f| doc.view().source().contains(f.trim_start_matches('\\')))
            {
                return Vec::new();
            }
            // Namespace filter: skip if the file's imports can't resolve the
            // short name to the target FQN and the FQN doesn't appear literally.
            if let Some(fqn) = class_fqn
                && !doc_can_reference_target(doc, short_name, fqn)
                && !doc.view().source().contains(fqn.trim_start_matches('\\'))
            {
                return Vec::new();
            }
            let mut spans = Vec::new();
            new_refs_in_stmts(&doc.program().stmts, short_name, class_fqn, &mut spans);
            let sv = doc.view();
            spans
                .into_iter()
                .map(|span| {
                    let start = sv.position_of(span.start);
                    let end = sv.position_of(span.end);
                    Location {
                        uri: uri.clone(),
                        range: Range { start, end },
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Convert a session reference tuple `(file_uri, line, col_start, col_end)` —
/// as produced by `DocumentStore::session_references_to` — into an LSP
/// `Location`. Returns `None` when the file URI fails to parse.
pub(crate) fn session_tuple_to_location(
    (file, line, col_start, col_end): (Arc<str>, u32, u32, u32),
) -> Option<Location> {
    let uri = Url::parse(&file).ok()?;
    Some(Location {
        uri,
        range: Range {
            start: Position {
                line,
                character: col_start,
            },
            end: Position {
                line,
                character: col_end,
            },
        },
    })
}

/// Dedup key for a reference location: `(uri, start line, start char, end char)`.
/// Finer than `type_definition`'s `(uri, line)` key — two references on the same
/// line (e.g. chained calls) are distinct results and must both survive.
pub(crate) fn ref_location_key(loc: &Location) -> (String, u32, u32, u32) {
    (
        loc.uri.to_string(),
        loc.range.start.line,
        loc.range.start.character,
        loc.range.end.character,
    )
}

/// De-duplicate reference locations by [`ref_location_key`], preserving
/// first-seen order.
pub(crate) fn dedup_ref_locations(locations: &mut Vec<Location>) {
    let mut seen = HashSet::new();
    locations.retain(|loc| seen.insert(ref_location_key(loc)));
}

// NOTE: a mir-codebase fast path for references (find_references_codebase)
// previously lived here, fully stubbed: every symbol kind fell through to the
// AST walker, and nothing called it. Removed. A real fast path needs mir to
// record ClassReference for type hints / `extends` / `implements` /
// `instanceof` (today it only records `new Foo()` sites), so using the index
// for Class kind would silently drop references.

fn find_references_inner(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
    include_use: bool,
    kind: Option<SymbolKind>,
    target_fqn: Option<&str>,
) -> Vec<Location> {
    // Each document is scanned independently: substring pre-filter, AST walk,
    // then span → position translation. Rayon parallelizes across docs; the
    // per-doc work is CPU-bound and 100% independent, so this scales linearly
    // with cores on large workspaces (Laravel: ~1,600 files).
    // Per-file namespace pre-filter only applies to Function and Class kinds,
    // where the target FQN refers to the symbol itself. For methods the
    // target is the *owning* FQCN, which can't be compared against the
    // method name via namespace resolution.
    let namespace_filter_active =
        matches!(kind, Some(SymbolKind::Function) | Some(SymbolKind::Class));
    all_docs
        .par_iter()
        .flat_map_iter(|(uri, doc)| {
            // Cheap memchr gate before any AST work. doc_can_reference_target
            // walks use-statement nodes and must not run on files that can't
            // possibly match.
            if !doc.view().source().contains(word) {
                return Vec::new();
            }
            if namespace_filter_active
                && let Some(target) = target_fqn
                && !doc_can_reference_target(doc, word, target)
            {
                return Vec::new();
            }
            scan_doc(
                word,
                uri,
                doc,
                include_declaration,
                include_use,
                kind,
                target_fqn,
            )
        })
        .collect()
}

/// Return true when this doc's namespace + `use` imports could plausibly
/// refer to `target_fqn` under the short name `word`.  Used as a pre-filter
/// so the AST walker doesn't emit refs in files whose namespace would resolve
/// `word` to a different FQN.
fn doc_can_reference_target(doc: &ParsedDoc, word: &str, target_fqn: &str) -> bool {
    let target = target_fqn.trim_start_matches('\\');
    let imports = collect_file_imports(doc);
    let resolved = crate::navigation::moniker::resolve_fqn(doc, word, &imports);
    // PHP falls back to the global namespace for unqualified *function* calls
    // when the namespaced version doesn't exist.  We don't know at this point
    // which symbol category the target is, so accept either an exact match
    // or a global-namespace fallback match.
    resolved == target
        || (resolved == word && !target.contains('\\'))
        || (resolved == word && target == format!("\\{word}"))
}

struct ImportsVisitor {
    only_kind: Option<UseKind>,
    out: HashMap<String, String>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for ImportsVisitor {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        match &stmt.kind {
            StmtKind::Use(u) if self.only_kind.is_none_or(|k| u.kind == k) => {
                for item in u.uses.iter() {
                    let fqn = item.name.to_string_repr().into_owned();
                    let short = item
                        .alias
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| fqn_short_name(&fqn).to_string());
                    self.out.insert(short, fqn);
                }
                ControlFlow::Continue(())
            }
            // walk_stmt recurses into NamespaceBody::Braced automatically.
            StmtKind::Namespace(_) => walk_stmt(self, stmt),
            _ => ControlFlow::Continue(()),
        }
    }
}

/// Build a local-name → FQN map from a doc's `use` statements.  Mirrors
/// `Backend::file_imports` but self-contained so the reference walker can
/// run without a persistent codebase. Includes all use kinds (class, function,
/// const) — callers that only want class imports should use `collect_class_imports`.
pub(crate) fn collect_file_imports(doc: &ParsedDoc) -> HashMap<String, String> {
    collect_imports_filtered(doc, None)
}

/// Like `collect_file_imports` but restricted to `use ClassName` statements
/// (`UseKind::Normal`). Use this wherever the import map is fed into class
/// resolution — mixing in `use function` / `use const` entries causes the
/// resolver to map a function/const short name to the wrong FQN when the same
/// short name appears as a type hint or class reference.
///
/// TODO: upstream fix — have mir's FileAnalyzer auto-load via its ClassResolver
/// so lsp no longer needs to pre-collect class dependencies manually.
pub(crate) fn collect_class_imports(doc: &ParsedDoc) -> HashMap<String, String> {
    collect_imports_filtered(doc, Some(UseKind::Normal))
}

fn collect_imports_filtered(
    doc: &ParsedDoc,
    only_kind: Option<UseKind>,
) -> HashMap<String, String> {
    let mut v = ImportsVisitor {
        only_kind,
        out: HashMap::new(),
    };
    for stmt in doc.program().stmts.iter() {
        let _ = v.visit_stmt(stmt);
    }
    v.out
}

/// Collect every FQN class name (e.g. `\App\Model\Entity`) referenced in a
/// `new` expression that has no corresponding `use` import (i.e. written with
/// a leading `\`).  Returns de-duplicated strings with the leading `\` stripped,
/// ready for `session.lazy_load_class`.
pub(crate) fn collect_fqn_new_class_refs(doc: &ParsedDoc) -> Vec<String> {
    fqn_new_class_refs_in_stmts(&doc.program().stmts)
}

/// Collect every class-typed reference in `doc` (extends, implements, new,
/// instanceof, type hints, static calls, catch types), resolved to an FQN via
/// the current namespace and `use` imports. Used to lazy-load same-namespace
/// dependencies that have no explicit `use` statement (and so are missed by
/// `collect_file_imports`) before semantic analysis runs.
///
/// Returns de-duplicated FQNs with any leading `\` stripped.
pub(crate) fn collect_referenced_class_fqns(doc: &ParsedDoc) -> Vec<String> {
    let imports = collect_class_imports(doc);
    let names = all_class_ref_names_in_stmts(&doc.program().stmts);
    let locals = collect_local_type_decl_fqns(doc);
    let mut out: Vec<String> = names
        .into_iter()
        .map(|name| {
            // A leading `\` marks an already-fully-qualified reference like
            // `new \App\Model\Entity()` — strip the slash and use as-is.
            // `resolve_fqn` would otherwise prepend the current namespace.
            if let Some(stripped) = name.strip_prefix('\\') {
                return stripped.to_string();
            }
            let fqn = crate::navigation::moniker::resolve_fqn(doc, &name, &imports);
            fqn.trim_start_matches('\\').to_string()
        })
        // Skip references that resolve to a type declared in this very file —
        // mir already has them via `session.ingest_file`, and asking it to
        // lazy-load them can recurse back through analysis.
        .filter(|fqn| !locals.contains(fqn))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// FQNs of every top-level type declared in `doc` (class, interface, trait,
/// enum), applying the file's `namespace` declaration. Used to suppress
/// self-references in the lazy-load list.
fn collect_local_type_decl_fqns(doc: &ParsedDoc) -> HashSet<String> {
    use php_ast::NamespaceBody;
    let mut out = HashSet::new();
    fn name_of(kind: &StmtKind<'_, '_>) -> Option<String> {
        match kind {
            StmtKind::Class(c) => c.name.as_ref().map(|n| n.to_string()),
            StmtKind::Interface(i) => Some(i.name.to_string()),
            StmtKind::Trait(t) => Some(t.name.to_string()),
            StmtKind::Enum(e) => Some(e.name.to_string()),
            _ => None,
        }
    }
    let mut current_ns: Option<String> = None;
    for stmt in doc.program().stmts.iter() {
        match &stmt.kind {
            StmtKind::Namespace(ns) => {
                let ns_name = ns.name.as_ref().map(|n| n.to_string_repr().to_string());
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        let prefix = ns_name
                            .as_deref()
                            .map(|n| format!("{n}\\"))
                            .unwrap_or_default();
                        for s in inner.stmts.iter() {
                            if let Some(n) = name_of(&s.kind) {
                                out.insert(format!("{prefix}{n}"));
                            }
                        }
                    }
                    NamespaceBody::Simple => {
                        current_ns = ns_name;
                    }
                }
            }
            k => {
                if let Some(n) = name_of(k) {
                    let fqn = match &current_ns {
                        Some(ns) => format!("{ns}\\{n}"),
                        None => n,
                    };
                    out.insert(fqn);
                }
            }
        }
    }
    out
}

fn scan_doc(
    word: &str,
    uri: &Url,
    doc: &Arc<ParsedDoc>,
    include_declaration: bool,
    include_use: bool,
    kind: Option<SymbolKind>,
    target_fqn: Option<&str>,
) -> Vec<Location> {
    let source = doc.source();
    // Substring pre-filter: every walker below pushes a span only when an
    // identifier's bytes equal `word`, so if `word` does not appear in the
    // source it cannot produce any reference. `str::contains` is memchr-fast
    // and skips the full AST traversal for the vast majority of files.
    if !source.contains(word) {
        return Vec::new();
    }
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
            // Property walker emits both access sites *and* declaration spans
            // (used by rename). Strip decls here when the caller doesn't want them.
            Some(SymbolKind::Property) => {
                property_refs_in_stmts(source, stmts, word, &mut spans);
                if !include_declaration {
                    let mut decl_spans = Vec::new();
                    collect_declaration_spans(
                        source,
                        stmts,
                        word,
                        Some(SymbolKind::Property),
                        &mut decl_spans,
                    );
                    let decl_set: HashSet<(u32, u32)> =
                        decl_spans.iter().map(|s| (s.start, s.end)).collect();
                    spans.retain(|span| !decl_set.contains(&(span.start, span.end)));
                }
            }
            // Constant walker emits both declaration spans and access spans.
            Some(SymbolKind::Constant) => {
                // Class constants: target_fqn = owning class short name (no backslash).
                // Global/namespace constants: target_fqn = None (root) or
                //   "Namespace\\ConstName" (namespaced, has backslash). Route to the
                //   bare-identifier walker instead of the `::` class-const walker.
                let is_global = target_fqn.is_none_or(|fqn| fqn.contains('\\'));
                if is_global {
                    global_constant_refs_in_stmts(source, stmts, word, target_fqn, &mut spans);
                } else {
                    // target_fqn = class short name for class constants.
                    constant_refs_in_stmts(source, stmts, word, target_fqn, &mut spans);
                }
                if !include_declaration {
                    let mut decl_spans = Vec::new();
                    collect_declaration_spans(
                        source,
                        stmts,
                        word,
                        Some(SymbolKind::Constant),
                        &mut decl_spans,
                    );
                    let decl_set: HashSet<(u32, u32)> =
                        decl_spans.iter().map(|s| (s.start, s.end)).collect();
                    spans.retain(|span| !decl_set.contains(&(span.start, span.end)));
                }
            }
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
        // Typed walkers (except Property, which already includes decls) don't emit
        // declaration spans, so add them separately when wanted. Pass `kind` so only
        // declarations of the matching category are appended — a Method search must
        // not return a free-function declaration with the same name.
        if include_declaration
            && matches!(
                kind,
                Some(SymbolKind::Function) | Some(SymbolKind::Method) | Some(SymbolKind::Class)
            )
        {
            collect_declaration_spans(source, stmts, word, kind, &mut spans);
        }
    }

    let sv = doc.view();
    let word_utf16_len: u32 = utf16_code_units(word);
    spans
        .into_iter()
        .map(|span| {
            let start = sv.position_of(span.start);
            let end = Position {
                line: start.line,
                character: start.character + word_utf16_len,
            };
            Location {
                uri: uri.clone(),
                range: Range { start, end },
            }
        })
        .collect()
}

/// Build a span covering exactly the declared name (not the keyword before it).
/// Uses the stmt_span to search within the statement's context, avoiding false
/// matches from earlier occurrences of the same name in the file.
fn declaration_name_span(source: &str, name: &str, stmt_span: Span) -> Span {
    let start = str_offset_in_range(source, stmt_span, name).unwrap_or(stmt_span.start);
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
    let want_property = matches!(kind, None | Some(SymbolKind::Property));
    let want_constant = matches!(kind, None | Some(SymbolKind::Constant));

    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if want_free && f.name == word => {
                out.push(declaration_name_span(
                    source,
                    &f.name.to_string(),
                    stmt.span,
                ));
            }
            StmtKind::Class(c) => {
                if want_type
                    && let Some(name) = c.name
                    && name == word
                {
                    out.push(declaration_name_span(source, &name.to_string(), stmt.span));
                }
                if want_method || want_property || want_constant {
                    for member in c.body.members.iter() {
                        match &member.kind {
                            ClassMemberKind::Method(m) if want_method && m.name == word => {
                                // Scope the name search to the member span,
                                // not the whole class — otherwise a class
                                // named the same as one of its members
                                // (`class get { function get() {} }`) resolves
                                // both decls to the class name's position.
                                out.push(declaration_name_span(
                                    source,
                                    &m.name.to_string(),
                                    member.span,
                                ));
                            }
                            ClassMemberKind::Method(m)
                                if want_property && m.name == "__construct" =>
                            {
                                // Promoted constructor params act as property declarations.
                                for p in m.params.iter() {
                                    if p.visibility.is_some() && p.name == word {
                                        out.push(declaration_name_span(
                                            source,
                                            &p.name.to_string(),
                                            p.span,
                                        ));
                                    }
                                }
                            }
                            ClassMemberKind::Property(p) if want_property && p.name == word => {
                                out.push(declaration_name_span(
                                    source,
                                    &p.name.to_string(),
                                    member.span,
                                ));
                            }
                            ClassMemberKind::ClassConst(c) if want_constant && c.name == word => {
                                out.push(declaration_name_span(
                                    source,
                                    &c.name.to_string(),
                                    member.span,
                                ));
                            }
                            _ => {}
                        }
                    }
                }
            }
            StmtKind::Interface(i) => {
                if want_type && i.name == word {
                    out.push(declaration_name_span(
                        source,
                        &i.name.to_string(),
                        stmt.span,
                    ));
                }
                if want_method || want_constant {
                    for member in i.body.members.iter() {
                        match &member.kind {
                            ClassMemberKind::Method(m) if want_method && m.name == word => {
                                out.push(declaration_name_span(
                                    source,
                                    &m.name.to_string(),
                                    member.span,
                                ));
                            }
                            ClassMemberKind::ClassConst(c) if want_constant && c.name == word => {
                                out.push(declaration_name_span(
                                    source,
                                    &c.name.to_string(),
                                    member.span,
                                ));
                            }
                            _ => {}
                        }
                    }
                }
            }
            StmtKind::Trait(t) => {
                if want_type && t.name == word {
                    out.push(declaration_name_span(
                        source,
                        &t.name.to_string(),
                        stmt.span,
                    ));
                }
                if want_method || want_property || want_constant {
                    for member in t.body.members.iter() {
                        match &member.kind {
                            ClassMemberKind::Method(m) if want_method && m.name == word => {
                                out.push(declaration_name_span(
                                    source,
                                    &m.name.to_string(),
                                    member.span,
                                ));
                            }
                            ClassMemberKind::Property(p) if want_property && p.name == word => {
                                out.push(declaration_name_span(
                                    source,
                                    &p.name.to_string(),
                                    member.span,
                                ));
                            }
                            ClassMemberKind::ClassConst(c) if want_constant && c.name == word => {
                                out.push(declaration_name_span(
                                    source,
                                    &c.name.to_string(),
                                    member.span,
                                ));
                            }
                            _ => {}
                        }
                    }
                }
            }
            StmtKind::Enum(e) => {
                if want_type && e.name == word {
                    out.push(declaration_name_span(
                        source,
                        &e.name.to_string(),
                        stmt.span,
                    ));
                }
                for member in e.body.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Method(m) if want_method && m.name == word => {
                            out.push(declaration_name_span(
                                source,
                                &m.name.to_string(),
                                member.span,
                            ));
                        }
                        EnumMemberKind::Case(c) if want_type && c.name == word => {
                            out.push(declaration_name_span(
                                source,
                                &c.name.to_string(),
                                member.span,
                            ));
                        }
                        EnumMemberKind::ClassConst(c) if want_constant && c.name == word => {
                            out.push(declaration_name_span(
                                source,
                                &c.name.to_string(),
                                member.span,
                            ));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Const(items) if want_constant => {
                for item in items.iter() {
                    if item.name == word {
                        let name = item.name.to_string();
                        out.push(declaration_name_span(source, &name, item.span));
                    }
                }
            }
            StmtKind::Expression(expr) if want_constant => {
                // `define('NAME', value)` acts as a global constant declaration.
                if let ExprKind::FunctionCall(f) = &expr.kind
                    && let ExprKind::Identifier(id) = &f.name.kind
                    && id.as_str() == "define"
                    && let Some(first_arg) = f.args.first()
                    && let ExprKind::String(s) = &first_arg.value.kind
                    && *s == word
                {
                    let start = first_arg.value.span.start + 1;
                    out.push(Span {
                        start,
                        end: start + s.len() as u32,
                    });
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_declaration_spans(source, &inner.stmts, word, kind, out);
                }
            }
            _ => {}
        }
    }
}
