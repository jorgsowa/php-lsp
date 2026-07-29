use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::Arc;

use php_ast::visitor::{Visitor, walk_stmt};
use php_ast::{Stmt, StmtKind, UseKind};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use super::walk::all_class_ref_names_in_stmts;
use crate::document::ast::ParsedDoc;

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

/// Convert a session reference tuple `(file_uri, line, col_start, col_end)` —
/// as produced by `DocumentStore::indexed_references` — into an LSP
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

struct ImportsVisitor {
    only_kind: Option<UseKind>,
    out: HashMap<String, String>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for ImportsVisitor {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        match &stmt.kind {
            StmtKind::Use(u) if self.only_kind.is_none_or(|k| u.kind == k) => {
                for item in u.uses.iter() {
                    let (short, fqn) = crate::document::ast::use_item_alias_and_fqn(item);
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

/// Build the typed `mir_analyzer::Name` for a declaration-site cursor from
/// the classified `(word, kind)` plus resolved owner/target FQNs. Usage-site
/// cursors don't come through here — `FileAnalysis::symbol_at` +
/// `ReferenceKind::to_name` already carry the resolved symbol.
///
/// `target_fqn` is the symbol's own FQN for Function/Class, the owning FQCN
/// for Method/Property, and for Constant either the owning FQCN (class
/// constant, `class_constant = true`) or the constant's FQN (global).
/// A missing FQN falls back to `word` for Function/Class/Constant (global
/// namespace); Method/Property require the owner and return `None` without it.
pub fn build_mir_symbol(
    word: &str,
    kind: Option<SymbolKind>,
    target_fqn: Option<&str>,
    class_constant: bool,
) -> Option<mir_analyzer::Name> {
    let norm = |s: &str| -> Arc<str> { Arc::from(s.trim_start_matches('\\')) };
    match kind {
        Some(SymbolKind::Function) => Some(mir_analyzer::Name::Function(norm(
            target_fqn.unwrap_or(word),
        ))),
        Some(SymbolKind::Class) => {
            Some(mir_analyzer::Name::Class(norm(target_fqn.unwrap_or(word))))
        }
        // An unresolvable owner (call/access on an untyped receiver, outside
        // any class) becomes an empty class: mir answers those from its
        // name-keyed fallback postings.
        Some(SymbolKind::Method) => Some(mir_analyzer::Name::Method {
            class: norm(target_fqn.unwrap_or("")),
            // PHP method dispatch is case-insensitive; normalize here.
            name: Arc::from(word.to_ascii_lowercase()),
        }),
        Some(SymbolKind::Property) => Some(mir_analyzer::Name::Property {
            class: norm(target_fqn.unwrap_or("")),
            name: Arc::from(word),
        }),
        Some(SymbolKind::Constant) => Some(if class_constant {
            mir_analyzer::Name::ClassConstant {
                class: norm(target_fqn?),
                name: Arc::from(word),
            }
        } else {
            mir_analyzer::Name::GlobalConstant(norm(target_fqn.unwrap_or(word)))
        }),
        None => None,
    }
}
