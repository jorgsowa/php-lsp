use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Param, Stmt, StmtKind};
use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::ast::{ParsedDoc, format_type_hint};
use crate::docblock::find_docblock;
use crate::type_map::TypeMap;
use crate::util::{is_php_builtin, php_doc_url, word_at};

pub fn hover_info(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    other_docs: &[(tower_lsp::lsp_types::Url, Arc<ParsedDoc>)],
) -> Option<Hover> {
    hover_at(source, doc, other_docs, position, None)
}

/// Full hover implementation with optional other-docs slice.
/// The `other_docs_arc` parameter is used internally for TypeMap construction.
pub fn hover_at(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[(tower_lsp::lsp_types::Url, Arc<ParsedDoc>)],
    position: Position,
    _other_docs_arc: Option<&[Arc<ParsedDoc>]>,
) -> Option<Hover> {
    // Feature 6: hover on use statement shows full FQN
    // Check this before word_at since cursor may be past the last word boundary
    if let Some(line_text) = source.lines().nth(position.line as usize) {
        let trimmed = line_text.trim();
        if trimmed.starts_with("use ") && !trimmed.starts_with("use function ") {
            let fqn = trimmed
                .strip_prefix("use ")
                .unwrap_or("")
                .trim_end_matches(';')
                .trim();
            if !fqn.is_empty() {
                // Find the word at position (may be None if at end of line)
                let maybe_word = word_at(source, position);
                let alias = fqn.rsplit('\\').next().unwrap_or(fqn);
                let matches = match &maybe_word {
                    Some(w) => w == alias || fqn.contains(w.as_str()),
                    None => true, // hovering past end of line on a use statement
                };
                if matches {
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("`use {};`", fqn),
                        }),
                        range: None,
                    });
                }
            }
        }
    }

    let word = word_at(source, position)?;

    // Feature 2: hover on $variable shows its type
    if word.starts_with('$') {
        let arc_docs: Vec<Arc<ParsedDoc>> = other_docs.iter().map(|(_, d)| d.clone()).collect();
        let type_map = TypeMap::from_docs_with_meta(doc, &arc_docs, None);
        if let Some(class_name) = type_map.get(&word) {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("`{}` `{}`", word, class_name),
                }),
                range: None,
            });
        }
    }

    // Search current document first, then cross-file.
    let found = scan_statements(&doc.program().stmts, &word).map(|sig| (sig, source, doc));
    let found = found.or_else(|| {
        for (_, other) in other_docs {
            if let Some(sig) = scan_statements(&other.program().stmts, &word) {
                return Some((sig, other.source(), other.as_ref()));
            }
        }
        None
    });

    if let Some((sig, sig_source, sig_doc)) = found {
        let mut value = wrap_php(&sig);
        if let Some(db) = find_docblock(sig_source, &sig_doc.program().stmts, &word) {
            let md = db.to_markdown();
            if !md.is_empty() {
                value.push_str("\n\n---\n\n");
                value.push_str(&md);
            }
        }
        if is_php_builtin(&word) {
            value.push_str(&format!("\n\n[php.net documentation]({})", php_doc_url(&word)));
        }
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value }),
            range: None,
        });
    }

    // Fallback: built-in function with no user-defined counterpart.
    if is_php_builtin(&word) {
        let value = format!(
            "```php\nfunction {}()\n```\n\n[php.net documentation]({})",
            word,
            php_doc_url(&word)
        );
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value }),
            range: None,
        });
    }

    // Feature 4: hover on a property name in `$obj->propName` or `$this->propName`
    if !word.starts_with('$') {
        if let Some(line_text) = source.lines().nth(position.line as usize) {
            // Check if the word appears after `->` or `?->` on this line
            let arrow_word = format!("->{}", word);
            let nullsafe_arrow_word = format!("?->{}", word);
            if line_text.contains(&arrow_word) || line_text.contains(&nullsafe_arrow_word) {
                // Find the position of `->word` in the line and extract the receiver var
                // before it.
                let arrow_pos = line_text.find(&nullsafe_arrow_word)
                    .or_else(|| line_text.find(&arrow_word));
                if let Some(apos) = arrow_pos {
                    let before_arrow = &line_text[..apos];
                    let receiver_var = extract_receiver_var_from_end(before_arrow);
                    if let Some(var_name) = receiver_var {
                        let arc_docs: Vec<Arc<ParsedDoc>> = other_docs.iter().map(|(_, d)| d.clone()).collect();
                        let type_map = TypeMap::from_docs_with_meta(doc, &arc_docs, None);
                        let class_name = if var_name == "$this" {
                            crate::type_map::enclosing_class_at(source, doc, position)
                                .or_else(|| type_map.get("$this").map(|s| s.to_string()))
                        } else {
                            type_map.get(&var_name).map(|s| s.to_string())
                        };
                        if let Some(cls) = class_name {
                            // Search current doc + other docs for the property type
                            let all_docs_search: Vec<&ParsedDoc> = std::iter::once(doc)
                                .chain(other_docs.iter().map(|(_, d)| d.as_ref()))
                                .collect();
                            for d in &all_docs_search {
                                if let Some(type_str) = find_property_type(d, &cls, &word) {
                                    let value = format!("`(property) {}::${}:{}`",
                                        cls, word,
                                        if type_str.is_empty() { String::new() } else { format!(" {}", type_str) }
                                    );
                                    return Some(Hover {
                                        contents: HoverContents::Markup(MarkupContent {
                                            kind: MarkupKind::Markdown,
                                            value,
                                        }),
                                        range: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Feature 3: hover on a built-in class name shows stub info
    if let Some(stub) = crate::stubs::builtin_class_members(&word) {
        let method_names: Vec<&str> = stub.methods.iter()
            .filter(|(_, is_static)| !is_static)
            .map(|(n, _)| n.as_str())
            .take(8)
            .collect();
        let static_names: Vec<&str> = stub.methods.iter()
            .filter(|(_, is_static)| *is_static)
            .map(|(n, _)| n.as_str())
            .take(4)
            .collect();
        let mut lines = vec![format!("**{}** — built-in class", word)];
        if !method_names.is_empty() {
            lines.push(format!("Methods: {}", method_names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")));
        }
        if !static_names.is_empty() {
            lines.push(format!("Static: {}", static_names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")));
        }
        if let Some(parent) = &stub.parent {
            lines.push(format!("Extends: `{parent}`"));
        }
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: lines.join("\n\n"),
            }),
            range: None,
        });
    }

    None
}

fn scan_statements(stmts: &[Stmt<'_, '_>], word: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                let params = format_params(&f.params);
                let ret = f
                    .return_type
                    .as_ref()
                    .map(|r| format!(": {}", format_type_hint(r)))
                    .unwrap_or_default();
                return Some(format!("function {}({}){}", word, params, ret));
            }
            StmtKind::Class(c) if c.name == Some(word) => {
                let mut sig = format!("class {}", word);
                if let Some(ext) = &c.extends {
                    sig.push_str(&format!(" extends {}", ext.to_string_repr()));
                }
                if !c.implements.is_empty() {
                    let ifaces: Vec<String> = c
                        .implements
                        .iter()
                        .map(|i| i.to_string_repr().into_owned())
                        .collect();
                    sig.push_str(&format!(" implements {}", ifaces.join(", ")));
                }
                return Some(sig);
            }
            StmtKind::Interface(i) if i.name == word => {
                return Some(format!("interface {}", word));
            }
            StmtKind::Trait(t) if t.name == word => {
                return Some(format!("trait {}", word));
            }
            StmtKind::Enum(e) if e.name == word => {
                let mut sig = format!("enum {}", word);
                if !e.implements.is_empty() {
                    let ifaces: Vec<String> = e
                        .implements
                        .iter()
                        .map(|i| i.to_string_repr().into_owned())
                        .collect();
                    sig.push_str(&format!(" implements {}", ifaces.join(", ")));
                }
                return Some(sig);
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Method(m) if m.name == word => {
                            let params = format_params(&m.params);
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|r| format!(": {}", format_type_hint(r)))
                                .unwrap_or_default();
                            return Some(format!("function {}({}){}", word, params, ret));
                        }
                        EnumMemberKind::Case(c) if c.name == word => {
                            let value_str = c
                                .value
                                .as_ref()
                                .and_then(format_expr_literal)
                                .map(|v| format!(" = {v}"))
                                .unwrap_or_default();
                            return Some(format!("case {}::{}{}", e.name, c.name, value_str));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == word {
                            let params = format_params(&m.params);
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|r| format!(": {}", format_type_hint(r)))
                                .unwrap_or_default();
                            return Some(format!("function {}({}){}", word, params, ret));
                        }
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == word {
                            let params = format_params(&m.params);
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|r| format!(": {}", format_type_hint(r)))
                                .unwrap_or_default();
                            return Some(format!("function {}({}){}", word, params, ret));
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(sig) = scan_statements(inner, word) {
                        return Some(sig);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Format a literal expression value for hover display (int or string literals only).
fn format_expr_literal(expr: &php_ast::Expr<'_, '_>) -> Option<String> {
    match &expr.kind {
        ExprKind::Int(n) => Some(n.to_string()),
        ExprKind::String(s) => Some(format!("\"{}\"", s)),
        _ => None,
    }
}

/// Look up markdown documentation for a symbol by name across all indexed documents.
/// Returns a markdown string with a code fence signature and optional PHPDoc annotations,
/// or `None` if the symbol is not found.
pub fn docs_for_symbol(
    name: &str,
    all_docs: &[(tower_lsp::lsp_types::Url, Arc<ParsedDoc>)],
) -> Option<String> {
    for (_, doc) in all_docs {
        if let Some(sig) = scan_statements(&doc.program().stmts, name) {
            let mut value = wrap_php(&sig);
            if let Some(db) = find_docblock(doc.source(), &doc.program().stmts, name) {
                let md = db.to_markdown();
                if !md.is_empty() {
                    value.push_str("\n\n---\n\n");
                    value.push_str(&md);
                }
            }
            if is_php_builtin(name) {
                value.push_str(&format!("\n\n[php.net documentation]({})", php_doc_url(name)));
            }
            return Some(value);
        }
    }
    // Fallback: built-in with no user-defined counterpart.
    if is_php_builtin(name) {
        return Some(format!(
            "```php\nfunction {}()\n```\n\n[php.net documentation]({})",
            name,
            php_doc_url(name)
        ));
    }
    None
}

pub(crate) fn format_params_str(params: &[Param<'_, '_>]) -> String {
    format_params(params)
}

fn format_params(params: &[Param<'_, '_>]) -> String {
    params
        .iter()
        .map(|p| {
            let mut s = String::new();
            if p.by_ref {
                s.push('&');
            }
            if p.variadic {
                s.push_str("...");
            }
            if let Some(t) = &p.type_hint {
                s.push_str(&format!("{} ", format_type_hint(t)));
            }
            s.push_str(&format!("${}", p.name));
            if let Some(default) = &p.default {
                s.push_str(&format!(" = {}", format_default_value(default)));
            }
            s
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Format a default parameter value for display in signatures.
fn format_default_value(expr: &php_ast::Expr<'_, '_>) -> String {
    match &expr.kind {
        ExprKind::Int(n) => n.to_string(),
        ExprKind::Float(f) => f.to_string(),
        ExprKind::String(s) => format!("'{}'", s),
        ExprKind::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
        ExprKind::Null => "null".to_string(),
        ExprKind::Array(items) => if items.is_empty() { "[]".to_string() } else { "[...]".to_string() },
        _ => "...".to_string(),
    }
}

fn wrap_php(sig: &str) -> String {
    format!("```php\n{}\n```", sig)
}

/// Extract the receiver variable name (with `$`) from the end of text that appears
/// immediately before `->` or `?->`.
fn extract_receiver_var_from_end(before_arrow: &str) -> Option<String> {
    // The text ends with the variable name (and possibly whitespace)
    let trimmed = before_arrow.trim_end();
    let var_name: String = trimmed
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '$')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if var_name.starts_with('$') && var_name.len() > 1 {
        Some(var_name)
    } else if !var_name.is_empty() && !var_name.starts_with('$') {
        Some(format!("${}", var_name))
    } else {
        None
    }
}

/// Find the type hint for a property named `prop_name` in class `class_name` within `doc`.
/// Returns `Some(type_str)` if found, where `type_str` may be empty if no type hint is present.
fn find_property_type(doc: &ParsedDoc, class_name: &str, prop_name: &str) -> Option<String> {
    find_property_type_in_stmts(&doc.program().stmts, class_name, prop_name)
}

fn find_property_type_in_stmts<'a>(
    stmts: &[Stmt<'a, 'a>],
    class_name: &str,
    prop_name: &str,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                for member in c.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Property(p) if p.name == prop_name => {
                            let type_str = p.type_hint.as_ref()
                                .map(|t| crate::ast::format_type_hint(t))
                                .unwrap_or_default();
                            return Some(type_str);
                        }
                        ClassMemberKind::Method(m) if m.name == "__construct" => {
                            // Check promoted constructor parameters
                            for p in m.params.iter() {
                                if p.name == prop_name && p.visibility.is_some() {
                                    let type_str = p.type_hint.as_ref()
                                        .map(|t| crate::ast::format_type_hint(t))
                                        .unwrap_or_default();
                                    return Some(type_str);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                // Property not found in this class
                return None;
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(t) = find_property_type_in_stmts(inner, class_name, prop_name) {
                        return Some(t);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn hover_on_function_name_returns_signature() {
        let src = "<?php\nfunction greet(string $name): string {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 10), &[]);
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("function greet("),
                "expected function signature, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_class_name_returns_class_sig() {
        let src = "<?php\nclass MyService {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 8), &[]);
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("class MyService"),
                "expected class sig, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_unknown_word_returns_none() {
        let src = "<?php\n$unknown = 42;";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 2), &[]);
        assert!(result.is_none(), "expected None for unknown word");
    }

    #[test]
    fn hover_at_column_beyond_line_length_returns_none() {
        let src = "<?php\nfunction hi() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 999), &[]);
        assert!(result.is_none());
    }

    #[test]
    fn word_at_extracts_from_middle_of_identifier() {
        let src = "<?php\nfunction greetUser() {}";
        let word = word_at(src, pos(1, 13));
        assert_eq!(word.as_deref(), Some("greetUser"));
    }

    #[test]
    fn hover_on_class_with_extends_shows_parent() {
        let src = "<?php\nclass Dog extends Animal {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 8), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("extends Animal"),
                "expected 'extends Animal', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_class_with_implements_shows_interfaces() {
        let src = "<?php\nclass Repo implements Countable, Serializable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 8), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("implements Countable, Serializable"),
                "expected implements list, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_trait_returns_trait_sig() {
        let src = "<?php\ntrait Loggable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 8), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("trait Loggable"),
                "expected 'trait Loggable', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_interface_returns_interface_sig() {
        let src = "<?php\ninterface Serializable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 12), &[]);
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("interface Serializable"),
                "expected interface sig, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn function_with_no_params_no_return_shows_no_colon() {
        let src = "<?php\nfunction init() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 10), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("function init()"),
                "expected 'function init()', got: {}",
                mc.value
            );
            assert!(
                !mc.value.contains(':'),
                "should not contain ':' when no return type, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_enum_returns_enum_sig() {
        let src = "<?php\nenum Suit {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 6), &[]);
        assert!(result.is_some());
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(mc.value.contains("enum Suit"), "expected 'enum Suit', got: {}", mc.value);
        }
    }

    #[test]
    fn hover_on_enum_with_implements_shows_interface() {
        let src = "<?php\nenum Status: string implements Stringable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 6), &[]);
        assert!(result.is_some());
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("implements Stringable"),
                "expected implements clause, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_enum_case_shows_case_sig() {
        let src = "<?php\nenum Status { case Active; case Inactive; }";
        let doc = ParsedDoc::parse(src.to_string());
        // "Active" starts at col 19: "enum Status { case Active;"
        let result = hover_info(src, &doc, pos(1, 21), &[]);
        assert!(result.is_some(), "expected hover on enum case");
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("Status::Active"),
                "expected 'Status::Active', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_backed_enum_case_shows_value() {
        let src = "<?php\nenum Color: string { case Red = 'red'; }";
        let doc = ParsedDoc::parse(src.to_string());
        // "Red" starts at col 26: "enum Color: string { case Red"
        let result = hover_info(src, &doc, pos(1, 27), &[]);
        assert!(result.is_some(), "expected hover on backed enum case");
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("Color::Red"),
                "expected 'Color::Red', got: {}",
                mc.value
            );
            assert!(
                mc.value.contains("\"red\""),
                "expected case value, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_trait_method_returns_signature() {
        let src = "<?php\ntrait Loggable { public function log(string $msg): void {} }";
        let doc = ParsedDoc::parse(src.to_string());
        // "log" at "trait Loggable { public function log(" — col 33
        let result = hover_info(src, &doc, pos(1, 34), &[]);
        assert!(result.is_some(), "expected hover on trait method");
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("function log("),
                "expected function sig, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn cross_file_hover_finds_class_in_other_doc() {
        use std::sync::Arc;
        let src = "<?php\n$x = new PaymentService();";
        let other_src = "<?php\nclass PaymentService { public function charge() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let other_doc = Arc::new(ParsedDoc::parse(other_src.to_string()));
        let uri = tower_lsp::lsp_types::Url::parse("file:///other.php").unwrap();
        let other_docs = vec![(uri, other_doc)];
        // Hover on "PaymentService" in line 1
        let result = hover_info(src, &doc, pos(1, 12), &other_docs);
        assert!(result.is_some(), "expected cross-file hover result");
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("PaymentService"),
                "expected 'PaymentService', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_variable_shows_type() {
        let src = "<?php\n$obj = new Mailer();\n$obj";
        let doc = ParsedDoc::parse(src.to_string());
        let h = hover_at(src, &doc, &[], pos(2, 2), None);
        assert!(h.is_some());
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("Mailer"), "hover on $obj should show Mailer");
    }

    #[test]
    fn hover_on_builtin_class_shows_stub_info() {
        let src = "<?php\n$pdo = new PDO('sqlite::memory:');\n$pdo->query('SELECT 1');";
        let doc = ParsedDoc::parse(src.to_string());
        let h = hover_at(src, &doc, &[], pos(1, 12), None);
        assert!(h.is_some(), "should hover on PDO");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("PDO"), "hover should mention PDO");
    }

    #[test]
    fn hover_on_property_shows_type() {
        let src = "<?php\nclass User { public string $name; public int $age; }\n$u = new User();\n$u->name";
        let doc = ParsedDoc::parse(src.to_string());
        // "name" in "$u->name" — col 4 in "$u->name"
        let h = hover_at(src, &doc, &[], pos(3, 5), None);
        assert!(h.is_some(), "expected hover on property");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("User"), "should mention class name");
        assert!(text.contains("name"), "should mention property name");
        assert!(text.contains("string"), "should show type hint");
    }

    #[test]
    fn hover_on_promoted_property_shows_type() {
        let src = "<?php\nclass Point {\n    public function __construct(\n        public float $x,\n        public float $y,\n    ) {}\n}\n$p = new Point(1.0, 2.0);\n$p->x";
        let doc = ParsedDoc::parse(src.to_string());
        // "x" at the end of "$p->x"
        let h = hover_at(src, &doc, &[], pos(8, 4), None);
        assert!(h.is_some(), "expected hover on promoted property");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("Point"), "should mention class name");
        assert!(text.contains("x"), "should mention property name");
        assert!(text.contains("float"), "should show type hint for promoted property");
    }

    #[test]
    fn hover_on_use_alias_shows_fqn() {
        let src = "<?php\nuse App\\Mail\\Mailer;\n$m = new Mailer();";
        let doc = ParsedDoc::parse(src.to_string());
        let h = hover_at(src, &doc, &[], Position { line: 1, character: 20 }, None);
        assert!(h.is_some());
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("App\\Mail\\Mailer"), "should show full FQN");
    }
}
