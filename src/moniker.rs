use php_ast::{NamespaceBody, StmtKind};
use tower_lsp::lsp_types::*;

use std::collections::HashMap;

use crate::ast::ParsedDoc;
use crate::util::word_at;

/// Return a moniker for the symbol at `position`.
///
/// Scheme: `"php"`.
/// Identifier: the PSR-4 fully-qualified name (`Ns\\ClassName`,
/// `Ns\\ClassName::method`, or bare `functionName`).
/// Uniqueness: `workspace` (symbols declared in the current file or resolved
/// via the workspace index).
pub fn moniker_at(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    file_imports: &HashMap<String, String>,
) -> Option<Moniker> {
    let word = word_at(source, position)?;
    if word.is_empty() || word.starts_with('$') {
        return None;
    }

    let identifier = resolve_fqn(doc, &word, file_imports);

    Some(Moniker {
        scheme: "php".to_string(),
        identifier,
        unique: UniquenessLevel::Project,
        kind: Some(MonikerKind::Export),
    })
}

/// Walk the top-level statements of `doc` looking for a declaration of `name`
/// and return its fully-qualified name including the namespace prefix.
/// When the name is not declared in this file, checks `use` statements so that
/// imported names resolve to their FQN (e.g. `Mailer` → `App\\Services\\Mailer`).
/// Falls back to returning `name` as-is.
pub(crate) fn resolve_fqn(
    doc: &ParsedDoc,
    name: &str,
    file_imports: &HashMap<String, String>,
) -> String {
    // Strip a leading `\` from a fully-qualified reference.
    let bare = name.trim_start_matches('\\');

    // Track the current namespace prefix across top-level statements so that
    // the declaration-form `namespace App;` (NamespaceBody::Simple) applies
    // to every subsequent class/function until the next namespace statement.
    let mut current_ns: Option<String> = None;

    fn matches_top(kind: &StmtKind<'_, '_>, name: &str) -> bool {
        match kind {
            StmtKind::Class(c) => c.name == Some(name),
            StmtKind::Interface(i) => i.name == name,
            StmtKind::Trait(t) => t.name == name,
            StmtKind::Enum(e) => e.name == name,
            StmtKind::Function(f) => f.name == name,
            _ => false,
        }
    }

    for stmt in doc.program().stmts.iter() {
        match &stmt.kind {
            StmtKind::Namespace(ns) => {
                let ns_name = ns.name.as_ref().map(|n| n.to_string_repr().to_string());
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        let ns_prefix = ns_name
                            .as_ref()
                            .map(|n| format!("{n}\\"))
                            .unwrap_or_default();
                        for s in inner.iter() {
                            if matches_top(&s.kind, bare) {
                                return format!("{ns_prefix}{bare}");
                            }
                        }
                    }
                    NamespaceBody::Simple => {
                        // Set the "active namespace" for all following top-level stmts.
                        current_ns = ns_name;
                    }
                }
            }
            k if matches_top(k, bare) => {
                return match &current_ns {
                    Some(ns) => format!("{ns}\\{bare}"),
                    None => bare.to_string(),
                };
            }
            _ => {}
        }
    }

    // Not a local declaration — resolve via `use` statements.
    if let Some(fqn) = file_imports.get(bare) {
        return fqn.clone();
    }

    // No local declaration and no `use` import. When the file declares a
    // namespace (Simple form), unqualified references still resolve to that
    // namespace (PHP falls back to global only for *functions*; for classes
    // the namespace-prefixed FQCN is authoritative).
    if let Some(ns) = current_ns {
        return format!("{ns}\\{bare}");
    }

    bare.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn empty() -> HashMap<String, String> {
        HashMap::new()
    }

    #[test]
    fn bare_class_name() {
        let src = "<?php\nclass Foo {}";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(1, 7), &empty()).unwrap();
        assert_eq!(m.scheme, "php");
        assert_eq!(m.identifier, "Foo");
        assert_eq!(m.unique, UniquenessLevel::Project);
        assert_eq!(m.kind, Some(MonikerKind::Export));
    }

    #[test]
    fn namespaced_class() {
        let src = "<?php\nnamespace App\\Services {\n    class FooService {}\n}";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(2, 10), &empty()).unwrap();
        assert_eq!(m.identifier, "App\\Services\\FooService");
    }

    #[test]
    fn unknown_word_returns_bare_name() {
        let src = "<?php\n$x = doSomething();";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(1, 6), &empty()).unwrap();
        assert_eq!(m.identifier, "doSomething");
    }

    #[test]
    fn empty_position_returns_none() {
        let src = "<?php\n   ";
        let d = doc(src);
        assert!(moniker_at(src, &d, pos(1, 1), &empty()).is_none());
    }

    #[test]
    fn variable_returns_none() {
        let src = "<?php\n$foo = 1;";
        let d = doc(src);
        assert!(moniker_at(src, &d, pos(1, 1), &empty()).is_none());
    }

    #[test]
    fn imported_name_resolves_via_use_statement() {
        let src = "<?php\nuse App\\Services\\Mailer;\n$m = new Mailer();";
        let d = doc(src);
        let imports = HashMap::from([("Mailer".to_string(), "App\\Services\\Mailer".to_string())]);
        // Cursor on `Mailer` in `new Mailer()`
        let m = moniker_at(src, &d, pos(2, 10), &imports).unwrap();
        assert_eq!(m.identifier, "App\\Services\\Mailer");
    }

    #[test]
    fn use_alias_resolves_to_fqn() {
        let src = "<?php\nuse App\\Http\\Request as Req;\n$r = new Req();";
        let d = doc(src);
        let imports = HashMap::from([("Req".to_string(), "App\\Http\\Request".to_string())]);
        let m = moniker_at(src, &d, pos(2, 10), &imports).unwrap();
        assert_eq!(m.identifier, "App\\Http\\Request");
    }

    #[test]
    fn uniqueness_is_workspace() {
        let src = "<?php\nclass Foo {}";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(1, 7), &empty()).unwrap();
        assert_eq!(m.unique, UniquenessLevel::Project);
    }
}
