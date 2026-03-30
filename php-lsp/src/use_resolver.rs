#![allow(dead_code)]
/// Resolves short class/function names to fully-qualified names using
/// the `use` statements found in a PHP file.
///
/// Example: `use App\Services\Mailer;` lets callers resolve `Mailer` → `App\Services\Mailer`.
use std::collections::HashMap;

use php_ast::{NamespaceBody, Stmt, StmtKind};

use crate::ast::ParsedDoc;

/// Map of short (unqualified) name → fully-qualified name.
#[derive(Debug, Default, Clone)]
pub struct UseMap(HashMap<String, String>);

impl UseMap {
    /// Build a UseMap from the `use` statements in a parsed document.
    pub fn from_doc(doc: &ParsedDoc) -> Self {
        let mut map = HashMap::new();
        collect_uses(&doc.program().stmts, &mut map);
        UseMap(map)
    }

    /// Resolve a short name to a FQN, if a matching `use` statement exists.
    pub fn resolve<'a>(&'a self, short: &str) -> Option<&'a str> {
        self.0.get(short).map(|s| s.as_str())
    }
}

fn collect_uses(stmts: &[Stmt<'_, '_>], map: &mut HashMap<String, String>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) => {
                for use_item in u.uses.iter() {
                    let fqn = use_item.name.to_string_repr().into_owned();
                    let short = use_item
                        .alias
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| last_segment(&fqn).to_string());
                    map.insert(short, fqn);
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_uses(inner, map);
                }
            }
            _ => {}
        }
    }
}

fn last_segment(fqn: &str) -> &str {
    fqn.rsplit('\\').next().unwrap_or(fqn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_simple_use() {
        let src = "<?php\nuse App\\Services\\Mailer;\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let map = UseMap::from_doc(&doc);
        assert_eq!(map.resolve("Mailer"), Some("App\\Services\\Mailer"));
    }

    #[test]
    fn resolves_aliased_use() {
        let src = "<?php\nuse App\\Services\\Mailer as Mail;\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let map = UseMap::from_doc(&doc);
        assert_eq!(map.resolve("Mail"), Some("App\\Services\\Mailer"));
        assert!(map.resolve("Mailer").is_none());
    }

    #[test]
    fn unknown_short_name_returns_none() {
        let src = "<?php\nuse App\\Foo;\n";
        let doc = ParsedDoc::parse(src.to_string());
        let map = UseMap::from_doc(&doc);
        assert!(map.resolve("Bar").is_none());
    }

    #[test]
    fn multiple_use_statements() {
        let src = "<?php\nuse App\\Foo;\nuse App\\Bar;\n";
        let doc = ParsedDoc::parse(src.to_string());
        let map = UseMap::from_doc(&doc);
        assert_eq!(map.resolve("Foo"), Some("App\\Foo"));
        assert_eq!(map.resolve("Bar"), Some("App\\Bar"));
    }

    #[test]
    fn use_inside_namespace_is_collected() {
        let src = "<?php\nnamespace MyNs {\nuse Lib\\Util;\nfunction f() {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let map = UseMap::from_doc(&doc);
        assert_eq!(map.resolve("Util"), Some("Lib\\Util"));
    }

    #[test]
    fn empty_file_gives_empty_map() {
        let src = "<?php\nfunction f() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let map = UseMap::from_doc(&doc);
        assert!(map.resolve("Anything").is_none());
    }
}
