use php_ast::{NamespaceBody, StmtKind};
use tower_lsp::lsp_types::*;

use crate::ast::ParsedDoc;
use crate::use_resolver::UseMap;
use crate::util::word_at;

/// Return a moniker for the symbol at `position`.
///
/// Scheme: `"php"`.
/// Identifier: the PSR-4 fully-qualified name (`Ns\\ClassName`,
/// `Ns\\ClassName::method`, or bare `functionName`).
/// Uniqueness: `workspace` (symbols declared in the current file or resolved
/// via the workspace index).
pub fn moniker_at(source: &str, doc: &ParsedDoc, position: Position) -> Option<Moniker> {
    let word = word_at(source, position)?;
    if word.is_empty() || word.starts_with('$') {
        return None;
    }

    let identifier = resolve_fqn(doc, &word);

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
fn resolve_fqn(doc: &ParsedDoc, name: &str) -> String {
    // Strip a leading `\` from a fully-qualified reference.
    let bare = name.trim_start_matches('\\');

    for stmt in doc.program().stmts.iter() {
        match &stmt.kind {
            // Top-level declarations
            StmtKind::Class(c) if c.name == Some(bare) => return bare.to_string(),
            StmtKind::Interface(i) if i.name == bare => return bare.to_string(),
            StmtKind::Trait(t) if t.name == bare => return bare.to_string(),
            StmtKind::Enum(e) if e.name == bare => return bare.to_string(),
            StmtKind::Function(f) if f.name == bare => return bare.to_string(),
            // Namespaced declarations
            StmtKind::Namespace(ns) => {
                let ns_prefix = ns
                    .name
                    .as_ref()
                    .map(|n| format!("{}\\", n.to_string_repr()))
                    .unwrap_or_default();

                if let NamespaceBody::Braced(inner) = &ns.body {
                    for s in inner.iter() {
                        let matched = match &s.kind {
                            StmtKind::Class(c) => c.name == Some(bare),
                            StmtKind::Interface(i) => i.name == bare,
                            StmtKind::Trait(t) => t.name == bare,
                            StmtKind::Enum(e) => e.name == bare,
                            StmtKind::Function(f) => f.name == bare,
                            _ => false,
                        };
                        if matched {
                            return format!("{ns_prefix}{bare}");
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Not a local declaration — resolve via `use` statements.
    let use_map = UseMap::from_doc(doc);
    if let Some(fqn) = use_map.resolve(bare) {
        return fqn.to_string();
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

    #[test]
    fn bare_class_name() {
        let src = "<?php\nclass Foo {}";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(1, 7)).unwrap();
        assert_eq!(m.scheme, "php");
        assert_eq!(m.identifier, "Foo");
        assert_eq!(m.unique, UniquenessLevel::Project);
        assert_eq!(m.kind, Some(MonikerKind::Export));
    }

    #[test]
    fn namespaced_class() {
        let src = "<?php\nnamespace App\\Services {\n    class FooService {}\n}";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(2, 10)).unwrap();
        assert_eq!(m.identifier, "App\\Services\\FooService");
    }

    #[test]
    fn unknown_word_returns_bare_name() {
        let src = "<?php\n$x = doSomething();";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(1, 6)).unwrap();
        assert_eq!(m.identifier, "doSomething");
    }

    #[test]
    fn empty_position_returns_none() {
        let src = "<?php\n   ";
        let d = doc(src);
        assert!(moniker_at(src, &d, pos(1, 1)).is_none());
    }

    #[test]
    fn variable_returns_none() {
        let src = "<?php\n$foo = 1;";
        let d = doc(src);
        assert!(moniker_at(src, &d, pos(1, 1)).is_none());
    }

    #[test]
    fn imported_name_resolves_via_use_statement() {
        let src = "<?php\nuse App\\Services\\Mailer;\n$m = new Mailer();";
        let d = doc(src);
        // Cursor on `Mailer` in `new Mailer()`
        let m = moniker_at(src, &d, pos(2, 10)).unwrap();
        assert_eq!(m.identifier, "App\\Services\\Mailer");
    }

    #[test]
    fn use_alias_resolves_to_fqn() {
        let src = "<?php\nuse App\\Http\\Request as Req;\n$r = new Req();";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(2, 10)).unwrap();
        assert_eq!(m.identifier, "App\\Http\\Request");
    }

    #[test]
    fn uniqueness_is_workspace() {
        let src = "<?php\nclass Foo {}";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(1, 7)).unwrap();
        assert_eq!(m.unique, UniquenessLevel::Project);
    }
}
