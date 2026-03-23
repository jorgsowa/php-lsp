/// Resolves short class/function names to fully-qualified names using
/// the `use` statements found in a PHP file.
///
/// Example: `use App\Services\Mailer;` lets callers resolve `Mailer` → `App\Services\Mailer`.
use std::collections::HashMap;

use php_parser_rs::parser::ast::{namespaces::NamespaceStatement, Statement};

/// Map of short (unqualified) name → fully-qualified name.
#[derive(Debug, Default, Clone)]
pub struct UseMap(HashMap<String, String>);

impl UseMap {
    pub fn empty() -> Self {
        UseMap(HashMap::new())
    }

    /// Build a UseMap from the `use` statements at the top of `stmts`.
    pub fn from_ast(stmts: &[Statement]) -> Self {
        let mut map = HashMap::new();
        collect_uses(stmts, &mut map);
        UseMap(map)
    }

    /// Resolve a short name to a FQN, if a matching `use` statement exists.
    pub fn resolve<'a>(&'a self, short: &str) -> Option<&'a str> {
        self.0.get(short).map(|s| s.as_str())
    }

    /// Return all short → FQN mappings.
    pub fn all(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

fn collect_uses(stmts: &[Statement], map: &mut HashMap<String, String>) {
    for stmt in stmts {
        match stmt {
            Statement::Use(u) => {
                for use_item in &u.uses {
                    let fqn = use_item.name.value.to_string();
                    // The alias overrides the short name; otherwise use the last segment
                    let short = use_item
                        .alias
                        .as_ref()
                        .map(|a| a.value.to_string())
                        .unwrap_or_else(|| last_segment(&fqn).to_string());
                    map.insert(short, fqn);
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                collect_uses(inner, map);
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

    fn parse_ast(source: &str) -> Vec<Statement> {
        match php_parser_rs::parser::parse(source) {
            Ok(ast) => ast,
            Err(stack) => stack.partial,
        }
    }

    #[test]
    fn resolves_simple_use() {
        let src = "<?php\nuse App\\Services\\Mailer;\nclass Foo {}";
        let ast = parse_ast(src);
        let map = UseMap::from_ast(&ast);
        assert_eq!(map.resolve("Mailer"), Some("App\\Services\\Mailer"));
    }

    #[test]
    fn resolves_aliased_use() {
        let src = "<?php\nuse App\\Services\\Mailer as Mail;\nclass Foo {}";
        let ast = parse_ast(src);
        let map = UseMap::from_ast(&ast);
        assert_eq!(map.resolve("Mail"), Some("App\\Services\\Mailer"));
        assert!(map.resolve("Mailer").is_none());
    }

    #[test]
    fn unknown_short_name_returns_none() {
        let src = "<?php\nuse App\\Foo;\n";
        let ast = parse_ast(src);
        let map = UseMap::from_ast(&ast);
        assert!(map.resolve("Bar").is_none());
    }

    #[test]
    fn multiple_use_statements() {
        let src = "<?php\nuse App\\Foo;\nuse App\\Bar;\n";
        let ast = parse_ast(src);
        let map = UseMap::from_ast(&ast);
        assert_eq!(map.resolve("Foo"), Some("App\\Foo"));
        assert_eq!(map.resolve("Bar"), Some("App\\Bar"));
    }

    #[test]
    fn use_inside_namespace_is_collected() {
        let src = "<?php\nnamespace MyNs;\nuse Lib\\Util;\nfunction f() {}";
        let ast = parse_ast(src);
        let map = UseMap::from_ast(&ast);
        assert_eq!(map.resolve("Util"), Some("Lib\\Util"));
    }

    #[test]
    fn empty_file_gives_empty_map() {
        let src = "<?php\nfunction f() {}";
        let ast = parse_ast(src);
        let map = UseMap::from_ast(&ast);
        assert!(map.resolve("Anything").is_none());
    }
}
