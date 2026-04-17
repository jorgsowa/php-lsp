/// PHPStorm metadata (`.phpstorm.meta.php`) parser.
///
/// Reads `override(ClassName::method(argIndex), map([literal => ReturnType]))` declarations
/// and exposes `resolve_return_type(class, method, arg_literal)` for type inference.
///
/// The most common use-case is DI containers:
/// ```php
/// override(\App\Container::make(0), map([
///     App\UserService::class => App\UserService::class,
/// ]));
/// ```
use std::collections::HashMap;
use std::path::Path;

use php_ast::{ExprKind, NamespaceBody, Stmt, StmtKind};

use crate::ast::ParsedDoc;

type MetaEntries = HashMap<(String, String), Vec<(Option<String>, String)>>;

/// A parsed `.phpstorm.meta.php` file.
#[derive(Debug, Default, Clone)]
pub struct PhpStormMeta {
    /// Key: `(lowercase_class, lowercase_method)`
    /// Value: list of `(arg_literal, return_class)` pairs.
    ///        `arg_literal == None` is the wildcard `'' => ReturnType` entry.
    entries: MetaEntries,
}

impl PhpStormMeta {
    /// Load from `<root>/.phpstorm.meta.php`.  Returns an empty map if the
    /// file doesn't exist or cannot be parsed.
    pub fn load(root: &Path) -> Self {
        let path = root.join(".phpstorm.meta.php");
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return Self::default(),
        };
        let doc = ParsedDoc::parse(text);
        let mut meta = Self::default();
        collect_overrides(&doc.program().stmts, &mut meta);
        meta
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Given `($obj->make)(UserService::class)`, resolve the return type.
    ///
    /// `arg` is the string value of the first argument (class name, with or
    /// without leading `\`).  Returns `None` when there is no matching entry.
    pub fn resolve_return_type(
        &self,
        class_name: &str,
        method_name: &str,
        arg: &str,
    ) -> Option<&str> {
        let key = (class_name.to_lowercase(), method_name.to_lowercase());
        let pairs = self.entries.get(&key)?;

        let needle = arg.trim_start_matches('\\');

        // 1. Exact match (case-insensitive, strip leading `\`)
        for (literal, ret) in pairs {
            if let Some(lit) = literal
                && lit.trim_start_matches('\\').eq_ignore_ascii_case(needle)
            {
                return Some(ret.as_str());
            }
        }
        // 2. Wildcard (`'' => ReturnType`)
        for (literal, ret) in pairs {
            if literal.is_none() {
                return Some(ret.as_str());
            }
        }
        None
    }
}

// ── AST walking ───────────────────────────────────────────────────────────────

fn collect_overrides(stmts: &[Stmt<'_, '_>], meta: &mut PhpStormMeta) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Namespace(ns) => {
                // We only care about the PHPSTORM_META namespace.
                let name = ns.name.as_ref().map(|n| n.to_string_repr());
                if name.as_deref() == Some("PHPSTORM_META")
                    && let NamespaceBody::Braced(inner) = &ns.body
                {
                    collect_overrides(inner, meta);
                }
            }
            StmtKind::Expression(expr) => {
                // Top-level `override(...)` call.
                if let ExprKind::FunctionCall(f) = &expr.kind
                    && let ExprKind::Identifier(name) = &f.name.kind
                    && name.eq_ignore_ascii_case("override")
                    && f.args.len() == 2
                {
                    parse_override(&f.args[0].value, &f.args[1].value, meta);
                }
            }
            _ => {}
        }
    }
}

/// Parse one `override(Target, map([...]))` pair.
fn parse_override(
    target: &php_ast::Expr<'_, '_>,
    mapping: &php_ast::Expr<'_, '_>,
    meta: &mut PhpStormMeta,
) {
    // Target: `\ClassName::methodName(argIndex)` — a static method call.
    let (class_name, method_name) = match extract_static_call_target(target) {
        Some(pair) => pair,
        None => return,
    };

    // Mapping: `map([...])` — a function call named "map" with one array arg.
    let pairs = match extract_map_pairs(mapping) {
        Some(p) => p,
        None => return,
    };

    let key = (class_name.to_lowercase(), method_name.to_lowercase());
    meta.entries.entry(key).or_default().extend(pairs);
}

/// Extract `(ClassName, methodName)` from `\ClassName::method(0)`.
fn extract_static_call_target(expr: &php_ast::Expr<'_, '_>) -> Option<(String, String)> {
    let ExprKind::StaticMethodCall(s) = &expr.kind else {
        return None;
    };
    let class_name = extract_class_name(s.class)?;
    let method_name = s.method.name_str()?.to_string();
    Some((class_name, method_name))
}

/// Get the bare class name from an Expr (handles `\Ns\Class` identifiers).
fn extract_class_name(expr: &php_ast::Expr<'_, '_>) -> Option<String> {
    match &expr.kind {
        ExprKind::Identifier(name) => {
            // Strip leading `\` and use only the last component.
            let s = name.trim_start_matches('\\');
            let short = s.rsplit('\\').next().unwrap_or(s);
            Some(short.to_string())
        }
        _ => None,
    }
}

/// Extract `[(literal | ClassName::class) => ReturnType]` from `map([...])`.
fn extract_map_pairs(expr: &php_ast::Expr<'_, '_>) -> Option<Vec<(Option<String>, String)>> {
    // `map([...])` — a function call with a single array argument.
    let ExprKind::FunctionCall(f) = &expr.kind else {
        return None;
    };
    if !matches!(&f.name.kind, ExprKind::Identifier(n) if n.eq_ignore_ascii_case("map")) {
        return None;
    }
    let array_arg = f.args.first()?;
    let ExprKind::Array(elements) = &array_arg.value.kind else {
        return None;
    };

    let mut pairs: Vec<(Option<String>, String)> = Vec::new();
    for elem in elements.iter() {
        let key_str = elem.key.as_ref().and_then(|k| extract_string_or_class(k));
        let val_str = extract_string_or_class(&elem.value);
        if let Some(ret_type) = val_str {
            pairs.push((key_str, ret_type));
        }
    }
    Some(pairs)
}

/// Extract a string value from either a string literal `'Foo'` or a
/// `ClassName::class` constant access.
fn extract_string_or_class(expr: &php_ast::Expr<'_, '_>) -> Option<String> {
    match &expr.kind {
        ExprKind::String(s) => {
            let raw = s.trim_start_matches('\\');
            // An empty string key is the wildcard; return `None` for that.
            if raw.is_empty() {
                None
            } else {
                // Use the short name (last component after `\`).
                let short = raw.rsplit('\\').next().unwrap_or(raw);
                Some(short.to_string())
            }
        }
        ExprKind::ClassConstAccess(c) => {
            // `Foo::class` — extract `Foo`.
            if c.member.name_str() == Some("class") {
                extract_class_name(c.class)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_meta(src: &str) -> PhpStormMeta {
        let doc = ParsedDoc::parse(src.to_string());
        let mut meta = PhpStormMeta::default();
        collect_overrides(&doc.program().stmts, &mut meta);
        meta
    }

    #[test]
    fn parses_simple_override() {
        let src = r#"<?php
namespace PHPSTORM_META {
    override(\App\Container::make(0), map([
        \App\UserService::class => \App\UserService::class,
    ]));
}"#;
        let meta = parse_meta(src);
        assert!(!meta.is_empty());
        let ret = meta.resolve_return_type("Container", "make", "UserService");
        assert_eq!(ret, Some("UserService"));
    }

    #[test]
    fn parses_string_literal_key() {
        let src = r#"<?php
namespace PHPSTORM_META {
    override(\App\Container::get(0), map([
        'UserService' => \App\UserService::class,
    ]));
}"#;
        let meta = parse_meta(src);
        let ret = meta.resolve_return_type("Container", "get", "UserService");
        assert_eq!(ret, Some("UserService"));
    }

    #[test]
    fn wildcard_fallback() {
        let src = r#"<?php
namespace PHPSTORM_META {
    override(\App\Container::make(0), map([
        '' => \stdClass::class,
    ]));
}"#;
        let meta = parse_meta(src);
        let ret = meta.resolve_return_type("Container", "make", "Anything");
        assert_eq!(ret, Some("stdClass"));
    }

    #[test]
    fn no_match_returns_none() {
        let src = r#"<?php
namespace PHPSTORM_META {
    override(\App\Container::make(0), map([
        'Foo' => \Foo::class,
    ]));
}"#;
        let meta = parse_meta(src);
        let ret = meta.resolve_return_type("Container", "make", "Bar");
        assert!(ret.is_none());
    }
}
