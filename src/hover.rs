use std::cell::OnceCell;
use std::sync::Arc;

use php_ast::{
    ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Param, Stmt, StmtKind, UseKind,
    Visibility,
};
use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::ast::{MethodReturnsMap, ParsedDoc, format_type_hint};
use crate::docblock::{Docblock, docblock_before, find_docblock, parse_docblock};
use crate::type_map::TypeMap;
use crate::util::{is_php_builtin, php_doc_url, word_at, word_range_at};

pub fn hover_info(
    source: &str,
    doc: &ParsedDoc,
    doc_returns: &MethodReturnsMap,
    position: Position,
    other_docs: &[(
        tower_lsp::lsp_types::Url,
        Arc<ParsedDoc>,
        Arc<MethodReturnsMap>,
    )],
) -> Option<Hover> {
    hover_at(source, doc, doc_returns, other_docs, position)
}

/// Full hover implementation.
pub fn hover_at(
    source: &str,
    doc: &ParsedDoc,
    doc_returns: &MethodReturnsMap,
    other_docs: &[(
        tower_lsp::lsp_types::Url,
        Arc<ParsedDoc>,
        Arc<MethodReturnsMap>,
    )],
    position: Position,
) -> Option<Hover> {
    let hover_range = word_range_at(source, position);

    // Hover on a `use` line shows the full FQN — check before word_at since the
    // cursor may be past the last word boundary.
    if let Some(line_text) = source.lines().nth(position.line as usize) {
        let trimmed = line_text.trim();
        if trimmed.starts_with("use ") && !trimmed.starts_with("use function ") {
            let fqn = trimmed
                .strip_prefix("use ")
                .unwrap_or("")
                .trim_end_matches(';')
                .trim();
            if !fqn.is_empty() {
                let maybe_word = word_at(source, position);
                let alias = fqn.rsplit('\\').next().unwrap_or(fqn);
                let matches = match &maybe_word {
                    Some(w) => w == alias || fqn.contains(w.as_str()),
                    None => true,
                };
                if matches {
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("`use {};`", fqn),
                        }),
                        range: hover_range,
                    });
                }
            }
        }
    }

    let word = word_at(source, position)?;

    // TypeMap is expensive; build lazily and reuse across branches.
    let type_map_cell: OnceCell<TypeMap> = OnceCell::new();
    let type_map = || {
        type_map_cell.get_or_init(|| {
            TypeMap::from_docs_at_position(
                doc,
                doc_returns,
                other_docs.iter().map(|(_, d, r)| (d.as_ref(), r.as_ref())),
                None,
                position,
            )
        })
    };

    // Hover on $variable shows its inferred type.
    if word.starts_with('$')
        && let Some(class_name) = type_map().get(&word)
    {
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("`{}` `{}`", word, class_name),
            }),
            range: hover_range,
        });
    }

    // Cursor-aware receiver resolution: extract the receiver from immediately
    // before `->word` or `?->word` at the cursor column, not just anywhere on
    // the line.  This correctly handles multiple method calls on one line.
    if !word.starts_with('$')
        && let Some(line_text) = source.lines().nth(position.line as usize)
    {
        if let Some(var_name) =
            extract_receiver_var_before_cursor(line_text, position.character as usize)
        {
            let tm = type_map();
            let class_name = if var_name == "$this" {
                crate::type_map::enclosing_class_at(source, doc, position)
                    .or_else(|| tm.get("$this").map(|s| s.to_string()))
            } else {
                tm.get(&var_name).map(|s| s.to_string())
            };
            if let Some(cls) = class_name {
                let first_cls = cls.split('|').next().unwrap_or(&cls);
                // Try method lookup first, then property lookup.
                for d in std::iter::once(doc).chain(other_docs.iter().map(|(_, d, _)| d.as_ref())) {
                    if let Some(sig) = scan_method_of_class(&d.program().stmts, first_cls, &word) {
                        let mut value = wrap_php(&sig);
                        if let Some(db) = find_method_docblock(d, first_cls, &word) {
                            let md = db.to_markdown();
                            if !md.is_empty() {
                                value.push_str("\n\n---\n\n");
                                value.push_str(&md);
                            }
                        }
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value,
                            }),
                            range: hover_range,
                        });
                    }
                    if let Some((modifiers, type_str, db)) = find_property_info(d, first_cls, &word)
                    {
                        let sig = format!(
                            "(property) {}{}::${}{}",
                            modifiers,
                            first_cls,
                            word,
                            if type_str.is_empty() {
                                String::new()
                            } else {
                                format!(": {}", type_str)
                            }
                        );
                        let mut value = wrap_php(&sig);
                        if let Some(doc) = db {
                            let md = doc.to_markdown();
                            if !md.is_empty() {
                                value.push_str("\n\n---\n\n");
                                value.push_str(&md);
                            }
                        }
                        return Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value,
                            }),
                            range: hover_range,
                        });
                    }
                }
            }
        }

        // Static call: `ClassName::method()` or `ClassName::CONST`.
        if let Some(class_name) =
            extract_static_class_before_cursor(line_text, position.character as usize)
        {
            let effective_class = if class_name == "self" || class_name == "static" {
                crate::type_map::enclosing_class_at(source, doc, position)
                    .unwrap_or(class_name.clone())
            } else if class_name == "parent" {
                // Find the enclosing class, then its parent
                crate::type_map::enclosing_class_at(source, doc, position)
                    .and_then(|enc| find_parent_class_name(&doc.program().stmts, &enc))
                    .unwrap_or(class_name.clone())
            } else {
                class_name.clone()
            };
            for d in std::iter::once(doc).chain(other_docs.iter().map(|(_, d, _)| d.as_ref())) {
                if let Some(sig) = scan_method_of_class(&d.program().stmts, &effective_class, &word)
                {
                    let mut value = wrap_php(&sig);
                    if let Some(db) = find_method_docblock(d, &effective_class, &word) {
                        let md = db.to_markdown();
                        if !md.is_empty() {
                            value.push_str("\n\n---\n\n");
                            value.push_str(&md);
                        }
                    }
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value,
                        }),
                        range: hover_range,
                    });
                }
            }
        }
    }

    // Resolve use-import aliases: `use Foo\Bar as Baz` — hovering on `Baz`
    // should show what `Bar` is.
    let all_stmts = &*doc.program().stmts as &[_];
    let resolved_word = resolve_use_alias(all_stmts, &word).unwrap_or_else(|| word.clone());

    // Search current document first, then cross-file (using resolved name).
    let found = scan_statements(&doc.program().stmts, &resolved_word).map(|sig| (sig, source, doc));
    let found = found.or_else(|| {
        for (_, other, _) in other_docs {
            if let Some(sig) = scan_statements(&other.program().stmts, &resolved_word) {
                return Some((sig, other.source(), other.as_ref()));
            }
        }
        None
    });

    if let Some((sig, sig_source, sig_doc)) = found {
        let mut value = wrap_php(&sig);
        if let Some(db) = find_docblock(sig_source, &sig_doc.program().stmts, &resolved_word) {
            let md = db.to_markdown();
            if !md.is_empty() {
                value.push_str("\n\n---\n\n");
                value.push_str(&md);
            }
        }
        if is_php_builtin(&resolved_word) {
            value.push_str(&format!(
                "\n\n[php.net documentation]({})",
                php_doc_url(&resolved_word)
            ));
        }
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: hover_range,
        });
    }

    // Fallback: built-in function with no user-defined counterpart.
    if is_php_builtin(&resolved_word) {
        let value = format!(
            "```php\nfunction {}()\n```\n\n[php.net documentation]({})",
            resolved_word,
            php_doc_url(&resolved_word)
        );
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: hover_range,
        });
    }

    // Hover on a built-in class name shows stub info.
    if let Some(stub) = crate::stubs::builtin_class_members(&resolved_word) {
        let method_names: Vec<&str> = stub
            .methods
            .iter()
            .filter(|(_, is_static)| !is_static)
            .map(|(n, _)| n.as_str())
            .take(8)
            .collect();
        let static_names: Vec<&str> = stub
            .methods
            .iter()
            .filter(|(_, is_static)| *is_static)
            .map(|(n, _)| n.as_str())
            .take(4)
            .collect();
        let mut lines = vec![format!("**{}** — built-in class", resolved_word)];
        if !method_names.is_empty() {
            lines.push(format!(
                "Methods: {}",
                method_names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !static_names.is_empty() {
            lines.push(format!(
                "Static: {}",
                static_names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(parent) = &stub.parent {
            lines.push(format!("Extends: `{parent}`"));
        }
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: lines.join("\n\n"),
            }),
            range: hover_range,
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
                let kw = if c.modifiers.is_abstract {
                    "abstract class"
                } else if c.modifiers.is_final {
                    "final class"
                } else if c.modifiers.is_readonly {
                    "readonly class"
                } else {
                    "class"
                };
                let mut sig = format!("{} {}", kw, word);
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
            StmtKind::Interface(i) => {
                for member in i.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            let prefix = format_method_prefix(
                                m.visibility.as_ref(),
                                m.is_static,
                                m.is_abstract,
                                m.is_final,
                            );
                            let params = format_params(&m.params);
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|r| format!(": {}", format_type_hint(r)))
                                .unwrap_or_default();
                            return Some(format!("{}function {}({}){}", prefix, word, params, ret));
                        }
                        ClassMemberKind::ClassConst(k) if k.name == word => {
                            return Some(format_class_const(k));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Trait(t) if t.name == word => {
                return Some(format!("trait {}", word));
            }
            StmtKind::Enum(e) if e.name == word => {
                let mut sig = if let Some(scalar) = &e.scalar_type {
                    format!("enum {}: {}", word, scalar.to_string_repr())
                } else {
                    format!("enum {}", word)
                };
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
                            let prefix = format_method_prefix(
                                m.visibility.as_ref(),
                                m.is_static,
                                m.is_abstract,
                                m.is_final,
                            );
                            let params = format_params(&m.params);
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|r| format!(": {}", format_type_hint(r)))
                                .unwrap_or_default();
                            return Some(format!("{}function {}({}){}", prefix, word, params, ret));
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
                        EnumMemberKind::ClassConst(k) if k.name == word => {
                            return Some(format_class_const(k));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            let prefix = format_method_prefix(
                                m.visibility.as_ref(),
                                m.is_static,
                                m.is_abstract,
                                m.is_final,
                            );
                            let params = format_params(&m.params);
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|r| format!(": {}", format_type_hint(r)))
                                .unwrap_or_default();
                            return Some(format!("{}function {}({}){}", prefix, word, params, ret));
                        }
                        ClassMemberKind::ClassConst(k) if k.name == word => {
                            return Some(format_class_const(k));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            let prefix = format_method_prefix(
                                m.visibility.as_ref(),
                                m.is_static,
                                m.is_abstract,
                                m.is_final,
                            );
                            let params = format_params(&m.params);
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|r| format!(": {}", format_type_hint(r)))
                                .unwrap_or_default();
                            return Some(format!("{}function {}({}){}", prefix, word, params, ret));
                        }
                        ClassMemberKind::ClassConst(k) if k.name == word => {
                            return Some(format_class_const(k));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(sig) = scan_statements(inner, word)
                {
                    return Some(sig);
                }
            }
            _ => {}
        }
    }
    None
}

/// Format a literal expression value for hover display (int, float, bool, or string literals).
fn format_expr_literal(expr: &php_ast::Expr<'_, '_>) -> Option<String> {
    match &expr.kind {
        ExprKind::Int(n) => Some(n.to_string()),
        ExprKind::Float(f) => Some(f.to_string()),
        ExprKind::Bool(b) => Some(if *b { "true" } else { "false" }.to_string()),
        ExprKind::String(s) => Some(format!("'{}'", s)),
        _ => None,
    }
}

/// Format a class/interface/enum constant declaration for hover display.
fn format_class_const(c: &php_ast::ClassConstDecl<'_, '_>) -> String {
    let type_str = c
        .type_hint
        .as_ref()
        .map(|t| format!("{} ", format_type_hint(t)))
        .or_else(|| match &c.value.kind {
            ExprKind::Int(_) => Some("int ".to_string()),
            ExprKind::String(_) => Some("string ".to_string()),
            ExprKind::Float(_) => Some("float ".to_string()),
            ExprKind::Bool(_) => Some("bool ".to_string()),
            _ => None,
        })
        .unwrap_or_default();
    let value_str = format_expr_literal(&c.value)
        .map(|v| format!(" = {v}"))
        .unwrap_or_default();
    format!("const {}{}{}", type_str, c.name, value_str)
}

pub(crate) fn format_params_str(params: &[Param<'_, '_>]) -> String {
    format_params(params)
}

// ── Index-based variants ──────────────────────────────────────────────────────

/// Return a function/method signature string from a `FileIndex` slice.
/// Falls back to built-in doc URL for built-in functions.
pub fn signature_for_symbol_from_index(
    name: &str,
    indexes: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::file_index::FileIndex>,
    )],
) -> Option<String> {
    for (_, idx) in indexes {
        for f in &idx.functions {
            if f.name == name {
                let params_str = f
                    .params
                    .iter()
                    .map(|p| {
                        let mut s = String::new();
                        if let Some(t) = &p.type_hint {
                            s.push_str(&format!("{} ", t));
                        }
                        if p.variadic {
                            s.push_str("...");
                        }
                        s.push_str(&format!("${}", p.name));
                        s
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let ret = f
                    .return_type
                    .as_deref()
                    .map(|r| format!(": {}", r))
                    .unwrap_or_default();
                return Some(format!("function {}({}){}", name, params_str, ret));
            }
        }
        for cls in &idx.classes {
            for m in &cls.methods {
                if m.name == name {
                    let params_str = m
                        .params
                        .iter()
                        .map(|p| {
                            let mut s = String::new();
                            if let Some(t) = &p.type_hint {
                                s.push_str(&format!("{} ", t));
                            }
                            if p.variadic {
                                s.push_str("...");
                            }
                            s.push_str(&format!("${}", p.name));
                            s
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let ret = m
                        .return_type
                        .as_deref()
                        .map(|r| format!(": {}", r))
                        .unwrap_or_default();
                    return Some(format!("function {}({}){}", name, params_str, ret));
                }
            }
        }
    }
    None
}

/// Return hover documentation for a symbol from a `FileIndex` slice.
pub fn docs_for_symbol_from_index(
    name: &str,
    indexes: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::file_index::FileIndex>,
    )],
) -> Option<String> {
    if let Some(sig) = signature_for_symbol_from_index(name, indexes) {
        let mut value = wrap_php(&sig);
        // Look for docblock text in the index.
        for (_, idx) in indexes {
            for f in &idx.functions {
                if f.name == name {
                    if let Some(raw) = &f.doc {
                        let db = crate::docblock::parse_docblock(raw);
                        let md = db.to_markdown();
                        if !md.is_empty() {
                            value.push_str("\n\n---\n\n");
                            value.push_str(&md);
                        }
                    }
                    break;
                }
            }
            for cls in &idx.classes {
                for m in &cls.methods {
                    if m.name == name {
                        if let Some(raw) = &m.doc {
                            let db = crate::docblock::parse_docblock(raw);
                            let md = db.to_markdown();
                            if !md.is_empty() {
                                value.push_str("\n\n---\n\n");
                                value.push_str(&md);
                            }
                        }
                        break;
                    }
                }
            }
        }
        if is_php_builtin(name) {
            value.push_str(&format!(
                "\n\n[php.net documentation]({})",
                php_doc_url(name)
            ));
        }
        return Some(value);
    }
    // Fallback: built-in.
    if is_php_builtin(name) {
        return Some(format!(
            "```php\nfunction {}()\n```\n\n[php.net documentation]({})",
            name,
            php_doc_url(name)
        ));
    }
    None
}

/// Build a hover for a class/interface/trait/enum found by short name in the workspace index.
/// Returns `None` when no class with that name exists in `indexes`.
pub fn class_hover_from_index(
    word: &str,
    indexes: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::file_index::FileIndex>,
    )],
) -> Option<Hover> {
    use crate::file_index::ClassKind;

    for (_, idx) in indexes {
        for cls in &idx.classes {
            if cls.name == word || cls.fqn.trim_start_matches('\\') == word {
                let kw = match cls.kind {
                    ClassKind::Interface => "interface",
                    ClassKind::Trait => "trait",
                    ClassKind::Enum => "enum",
                    ClassKind::Class => {
                        if cls.is_abstract {
                            "abstract class"
                        } else {
                            "class"
                        }
                    }
                };
                let mut sig = format!("{} {}", kw, cls.name);
                if let Some(parent) = &cls.parent {
                    sig.push_str(&format!(" extends {}", parent));
                }
                if !cls.implements.is_empty() {
                    let list: Vec<&str> = cls.implements.iter().map(|s| s.as_ref()).collect();
                    sig.push_str(&format!(" implements {}", list.join(", ")));
                }
                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: wrap_php(&sig),
                    }),
                    range: None,
                });
            }
        }
    }
    None
}

fn visibility_str(v: &Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

fn format_method_prefix(
    visibility: Option<&Visibility>,
    is_static: bool,
    is_abstract: bool,
    is_final: bool,
) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(v) = visibility {
        parts.push(visibility_str(v));
    }
    if is_abstract {
        parts.push("abstract");
    }
    if is_final {
        parts.push("final");
    }
    if is_static {
        parts.push("static");
    }
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(" ") + " "
    }
}

fn format_prop_prefix(
    visibility: Option<&Visibility>,
    is_static: bool,
    is_readonly: bool,
) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(v) = visibility {
        parts.push(visibility_str(v));
    }
    if is_static {
        parts.push("static");
    }
    if is_readonly {
        parts.push("readonly");
    }
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(" ") + " "
    }
}

fn format_params(params: &[Param<'_, '_>]) -> String {
    params
        .iter()
        .map(|p| {
            let mut s = String::new();
            if p.by_ref {
                s.push('&');
            }
            if let Some(t) = &p.type_hint {
                s.push_str(&format!("{} ", format_type_hint(t)));
            }
            if p.variadic {
                s.push_str("...");
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
        ExprKind::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        ExprKind::Null => "null".to_string(),
        ExprKind::Array(items) => {
            if items.is_empty() {
                "[]".to_string()
            } else {
                "[...]".to_string()
            }
        }
        _ => "...".to_string(),
    }
}

fn wrap_php(sig: &str) -> String {
    format!("```php\n{}\n```", sig)
}

/// Extract the receiver variable from immediately before `->word` or `?->word`
/// at the cursor's exact column position.  Uses the column rather than
/// `str::find()` so multiple method calls on the same line are handled
/// correctly.
fn extract_receiver_var_before_cursor(line: &str, cursor_col_utf16: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();

    // Convert UTF-16 cursor column to char index.
    let mut utf16 = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16 >= cursor_col_utf16 {
            break;
        }
        utf16 += ch.len_utf16();
        char_idx += 1;
    }

    // Find the start of the word under the cursor (expand left).
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut word_start = char_idx;
    while word_start > 0 && is_word_char(chars[word_start - 1]) {
        word_start -= 1;
    }

    // Check for `?->` (3 chars) or `->` (2 chars) immediately before word_start.
    let (is_arrow, arrow_end) = if word_start >= 3
        && chars[word_start - 3] == '?'
        && chars[word_start - 2] == '-'
        && chars[word_start - 1] == '>'
    {
        (true, word_start - 3)
    } else if word_start >= 2 && chars[word_start - 2] == '-' && chars[word_start - 1] == '>' {
        (true, word_start - 2)
    } else {
        (false, 0)
    };

    if !is_arrow {
        return None;
    }

    extract_name_from_chars_end(&chars[..arrow_end])
}

/// Extract the class name from immediately before `::` at the cursor's column.
fn extract_static_class_before_cursor(line: &str, cursor_col_utf16: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();

    let mut utf16 = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16 >= cursor_col_utf16 {
            break;
        }
        utf16 += ch.len_utf16();
        char_idx += 1;
    }

    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut word_start = char_idx;
    while word_start > 0 && is_word_char(chars[word_start - 1]) {
        word_start -= 1;
    }

    if word_start < 2 || chars[word_start - 2] != ':' || chars[word_start - 1] != ':' {
        return None;
    }

    let before_colons = &chars[..word_start - 2];
    // Class name may contain `\` for FQN; extract the short name (last segment).
    let is_name_char = |c: char| c.is_alphanumeric() || c == '_' || c == '\\';
    let end = before_colons.len().saturating_sub(
        before_colons
            .iter()
            .rev()
            .take_while(|&&c| c == ' ' || c == '\t')
            .count(),
    );
    let mut start = end;
    while start > 0 && is_name_char(before_colons[start - 1]) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let full: String = before_colons[start..end].iter().collect();
    // Return only the last segment so callers get a short name.
    Some(full.rsplit('\\').next().unwrap_or(&full).to_owned())
}

/// Walk backwards through `chars`, skipping whitespace, and return the
/// identifier (with `$` prefix if present) ending at the last non-space char.
fn extract_name_from_chars_end(chars: &[char]) -> Option<String> {
    let is_var_char = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let end = chars.len()
        - chars
            .iter()
            .rev()
            .take_while(|&&c| c == ' ' || c == '\t')
            .count();
    if end == 0 {
        return None;
    }
    let mut start = end;
    while start > 0 && is_var_char(chars[start - 1]) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let name: String = chars[start..end].iter().collect();
    if name.starts_with('$') && name.len() > 1 {
        Some(name)
    } else if !name.is_empty() && !name.starts_with('$') {
        // Plain identifier (e.g. `$obj->getUser()->name` — the inner result):
        // treat as a non-variable receiver; callers handle the `$` lookup.
        Some(format!("${}", name))
    } else {
        None
    }
}

/// Resolve a use-import alias to the short class name.
///
/// Given `use App\Foo as Bar`, hovering on `Bar` anywhere in the file should
/// resolve to `Foo` so the declaration lookup succeeds.
pub(crate) fn resolve_use_alias(stmts: &[Stmt<'_, '_>], word: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) if u.kind == UseKind::Normal => {
                for item in u.uses.iter() {
                    if let Some(alias) = item.alias
                        && alias == word
                    {
                        let fqn = item.name.to_string_repr();
                        let short = fqn.rsplit('\\').next().unwrap_or(fqn.as_ref()).to_owned();
                        return Some(short);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = resolve_use_alias(inner, word)
                {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find visibility, type, and docblock for a property named `prop_name` in class `class_name`.
/// Returns `Some((modifiers, type_str, docblock))` where `modifiers` is a prefix like
/// `"public readonly "` and `type_str` is the declared type (may be empty).
fn find_property_info(
    doc: &ParsedDoc,
    class_name: &str,
    prop_name: &str,
) -> Option<(String, String, Option<Docblock>)> {
    find_property_info_in_stmts(doc.source(), &doc.program().stmts, class_name, prop_name)
}

fn find_property_info_in_stmts<'a>(
    source: &str,
    stmts: &[Stmt<'a, 'a>],
    class_name: &str,
    prop_name: &str,
) -> Option<(String, String, Option<Docblock>)> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                for member in c.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Property(p) if p.name == prop_name => {
                            let modifiers = format_prop_prefix(
                                p.visibility.as_ref(),
                                p.is_static,
                                p.is_readonly,
                            );
                            let type_str = p
                                .type_hint
                                .as_ref()
                                .map(|t| crate::ast::format_type_hint(t))
                                .unwrap_or_default();
                            let db = docblock_before(source, member.span.start)
                                .map(|raw| parse_docblock(&raw));
                            return Some((modifiers, type_str, db));
                        }
                        ClassMemberKind::Method(m) if m.name == "__construct" => {
                            // Check promoted constructor parameters
                            for p in m.params.iter() {
                                if p.name == prop_name && p.visibility.is_some() {
                                    let modifiers =
                                        format_prop_prefix(p.visibility.as_ref(), false, false);
                                    let type_str = p
                                        .type_hint
                                        .as_ref()
                                        .map(|t| crate::ast::format_type_hint(t))
                                        .unwrap_or_default();
                                    // Promoted params don't have their own docblock;
                                    // filter the constructor's docblock to the @param for this
                                    // property only — exclude description, @return, @throws, etc.
                                    // Returns None (not Some(empty)) when no matching @param
                                    // exists, preserving the contract of this function.
                                    let db = docblock_before(source, member.span.start).and_then(
                                        |raw| {
                                            let full = parse_docblock(&raw);
                                            let matching: Vec<_> = full
                                                .params
                                                .into_iter()
                                                .filter(|dp| {
                                                    dp.name.strip_prefix('$') == Some(prop_name)
                                                })
                                                .collect();
                                            if matching.is_empty() {
                                                None
                                            } else {
                                                Some(crate::docblock::Docblock {
                                                    params: matching,
                                                    ..Default::default()
                                                })
                                            }
                                        },
                                    );
                                    return Some((modifiers, type_str, db));
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
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(t) =
                        find_property_info_in_stmts(source, inner, class_name, prop_name)
                {
                    return Some(t);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the signature of `method_name` within `class_name` (including trait
/// uses and the extends chain within the same stmts slice).
fn scan_method_of_class(
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    method_name: &str,
) -> Option<String> {
    scan_method_of_class_impl(stmts, stmts, class_name, method_name)
}

fn scan_method_of_class_impl<'a>(
    root: &[Stmt<'a, 'a>],
    stmts: &[Stmt<'a, 'a>],
    class_name: &str,
    method_name: &str,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                // 1. Direct method lookup.
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        let params = format_params(&m.params);
                        let ret = m
                            .return_type
                            .as_ref()
                            .map(|r| format!(": {}", format_type_hint(r)))
                            .unwrap_or_default();
                        return Some(format!(
                            "{}::{}({}){}",
                            class_name, method_name, params, ret
                        ));
                    }
                }
                // 2. Walk trait uses within the same document.
                let mut trait_names: Vec<String> = Vec::new();
                for member in c.members.iter() {
                    if let ClassMemberKind::TraitUse(tu) = &member.kind {
                        for tn in tu.traits.iter() {
                            let s = tn.to_string_repr();
                            let short = s.rsplit('\\').next().unwrap_or(s.as_ref()).to_owned();
                            trait_names.push(short);
                        }
                    }
                }
                for tname in &trait_names {
                    if let Some(partial) = find_method_sig_in_trait(root, tname, method_name) {
                        return Some(format!("{}::{}", class_name, partial));
                    }
                }
                // 3. Walk extends chain within the same document.
                if let Some(parent) = &c.extends {
                    let pn = parent.to_string_repr();
                    let short = pn.rsplit('\\').next().unwrap_or(pn.as_ref()).to_owned();
                    if let Some(sig) = scan_method_of_class_impl(root, root, &short, method_name) {
                        // Replace "Parent::" with "ClassName::" so the hover always
                        // shows the receiver type.
                        return Some(sig.replacen(
                            &format!("{}::", short),
                            &format!("{}::", class_name),
                            1,
                        ));
                    }
                }
                return None;
            }
            StmtKind::Trait(t) if t.name == class_name => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        let params = format_params(&m.params);
                        let ret = m
                            .return_type
                            .as_ref()
                            .map(|r| format!(": {}", format_type_hint(r)))
                            .unwrap_or_default();
                        return Some(format!(
                            "{}::{}({}){}",
                            class_name, method_name, params, ret
                        ));
                    }
                }
                return None;
            }
            StmtKind::Enum(e) if e.name == class_name => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        let params = format_params(&m.params);
                        let ret = m
                            .return_type
                            .as_ref()
                            .map(|r| format!(": {}", format_type_hint(r)))
                            .unwrap_or_default();
                        return Some(format!(
                            "{}::{}({}){}",
                            class_name, method_name, params, ret
                        ));
                    }
                }
                return None;
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    let result = scan_method_of_class_impl(root, inner, class_name, method_name);
                    if result.is_some() {
                        return result;
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Return `"methodName(params): ReturnType"` for `method_name` inside `trait_name`.
fn find_method_sig_in_trait(
    stmts: &[Stmt<'_, '_>],
    trait_name: &str,
    method_name: &str,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Trait(t) if t.name == trait_name => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        let params = format_params(&m.params);
                        let ret = m
                            .return_type
                            .as_ref()
                            .map(|r| format!(": {}", format_type_hint(r)))
                            .unwrap_or_default();
                        return Some(format!("{}({}){}", method_name, params, ret));
                    }
                }
                return None;
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = find_method_sig_in_trait(inner, trait_name, method_name)
                {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

/// Return the short name of the parent class of `class_name`, if declared in
/// these stmts.
fn find_parent_class_name(stmts: &[Stmt<'_, '_>], class_name: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                return c.extends.as_ref().map(|p| {
                    let pn = p.to_string_repr();
                    pn.rsplit('\\').next().unwrap_or(pn.as_ref()).to_owned()
                });
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = find_parent_class_name(inner, class_name)
                {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_method_docblock(
    doc: &ParsedDoc,
    class_name: &str,
    method_name: &str,
) -> Option<crate::docblock::Docblock> {
    find_method_docblock_in_stmts(doc.source(), &doc.program().stmts, class_name, method_name)
}

fn find_method_docblock_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    method_name: &str,
) -> Option<crate::docblock::Docblock> {
    find_method_docblock_impl(source, stmts, stmts, class_name, method_name)
}

fn find_method_docblock_impl<'a>(
    source: &str,
    root: &[Stmt<'a, 'a>],
    stmts: &[Stmt<'a, 'a>],
    class_name: &str,
    method_name: &str,
) -> Option<crate::docblock::Docblock> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                // Direct lookup.
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return docblock_before(source, member.span.start)
                            .map(|raw| parse_docblock(&raw));
                    }
                }
                // Walk trait uses.
                for member in c.members.iter() {
                    if let ClassMemberKind::TraitUse(tu) = &member.kind {
                        for tn in tu.traits.iter() {
                            let s = tn.to_string_repr();
                            let short = s.rsplit('\\').next().unwrap_or(s.as_ref()).to_owned();
                            if let Some(db) =
                                find_method_docblock_impl(source, root, root, &short, method_name)
                            {
                                return Some(db);
                            }
                        }
                    }
                }
                // Walk extends.
                if let Some(parent) = &c.extends {
                    let pn = parent.to_string_repr();
                    let short = pn.rsplit('\\').next().unwrap_or(pn.as_ref()).to_owned();
                    if let Some(db) =
                        find_method_docblock_impl(source, root, root, &short, method_name)
                    {
                        return Some(db);
                    }
                }
                return None;
            }
            StmtKind::Trait(t) if t.name == class_name => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return docblock_before(source, member.span.start)
                            .map(|raw| parse_docblock(&raw));
                    }
                }
                return None;
            }
            StmtKind::Enum(e) if e.name == class_name => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return docblock_before(source, member.span.start)
                            .map(|raw| parse_docblock(&raw));
                    }
                }
                return None;
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    let result =
                        find_method_docblock_impl(source, root, inner, class_name, method_name);
                    if result.is_some() {
                        return result;
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
    use crate::test_utils::cursor;
    use crate::type_map::build_method_returns;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn hover_on_function_name_returns_signature() {
        let (src, p) = cursor("<?php\nfunction g$0reet(string $name): string {}");
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, &build_method_returns(&doc), p, &[]);
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
        let (src, p) = cursor("<?php\nclass My$0Service {}");
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, &build_method_returns(&doc), p, &[]);
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
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 2), &[]);
        assert!(result.is_none(), "expected None for unknown word");
    }

    #[test]
    fn hover_at_column_beyond_line_length_returns_none() {
        let src = "<?php\nfunction hi() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 999), &[]);
        assert!(result.is_none());
    }

    #[test]
    fn word_at_extracts_from_middle_of_identifier() {
        let (src, p) = cursor("<?php\nfunction greet$0User() {}");
        let word = word_at(&src, p);
        assert_eq!(word.as_deref(), Some("greetUser"));
    }

    #[test]
    fn hover_on_class_with_extends_shows_parent() {
        let src = "<?php\nclass Dog extends Animal {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 8), &[]);
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
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 8), &[]);
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
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 8), &[]);
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
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 12), &[]);
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
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 10), &[]);
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
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 6), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("enum Suit"),
                "expected 'enum Suit', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_enum_with_implements_shows_interface() {
        let src = "<?php\nenum Status: string implements Stringable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 6), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
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
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 21), &[]);
        assert!(result.is_some(), "expected hover on enum case");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("Status::Active"),
                "expected 'Status::Active', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn snapshot_hover_backed_enum_case_shows_value() {
        check_hover(
            "<?php\nenum Color: string { case Red = 'red'; }",
            pos(1, 27),
            expect![[r#"
                ```php
                case Color::Red = 'red'
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_enum_class_const() {
        check_hover(
            "<?php\nenum Suit { const int MAX = 4; }",
            pos(1, 22),
            expect![[r#"
                ```php
                const int MAX = 4
                ```"#]],
        );
    }

    #[test]
    fn hover_on_trait_method_returns_signature() {
        let src = "<?php\ntrait Loggable { public function log(string $msg): void {} }";
        let doc = ParsedDoc::parse(src.to_string());
        // "log" at "trait Loggable { public function log(" — col 33
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 34), &[]);
        assert!(result.is_some(), "expected hover on trait method");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
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
        let other_mr = Arc::new(build_method_returns(&other_doc));
        let uri = tower_lsp::lsp_types::Url::parse("file:///other.php").unwrap();
        let other_docs = vec![(uri, other_doc, other_mr)];
        // Hover on "PaymentService" in line 1
        let result = hover_info(
            src,
            &doc,
            &build_method_returns(&doc),
            pos(1, 12),
            &other_docs,
        );
        assert!(result.is_some(), "expected cross-file hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
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
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(2, 2));
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
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(1, 12));
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
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(3, 5));
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
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(8, 4));
        assert!(h.is_some(), "expected hover on promoted property");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("Point"), "should mention class name");
        assert!(text.contains("x"), "should mention property name");
        assert!(
            text.contains("float"),
            "should show type hint for promoted property"
        );
    }

    #[test]
    fn hover_on_promoted_property_shows_only_its_param_docblock() {
        // Issue #26: hovering a promoted property should show only the @param for
        // that property, not the full constructor docblock (no @return, @throws,
        // or @param entries for other parameters).
        let src = "<?php\nclass User {\n    /**\n     * Create a user.\n     * @param string $name The user's display name\n     * @param int $age The user's age\n     * @return void\n     * @throws \\InvalidArgumentException\n     */\n    public function __construct(\n        public string $name,\n        public int $age,\n    ) {}\n}\n$u = new User('Alice', 30);\n$u->name";
        let doc = ParsedDoc::parse(src.to_string());
        // hover on "$u->name" — cursor on 'name' (line 15, char 4 after "$u->")
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(15, 4));
        assert!(h.is_some(), "expected hover on promoted property");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(
            text.contains("@param") && text.contains("$name"),
            "should show @param for $name"
        );
        assert!(
            !text.contains("$age"),
            "should NOT show @param for other parameters"
        );
        assert!(
            !text.contains("@return"),
            "should NOT show @return from constructor docblock"
        );
        assert!(
            !text.contains("@throws"),
            "should NOT show @throws from constructor docblock"
        );
        assert!(
            !text.contains("Create a user"),
            "should NOT show constructor description"
        );
    }

    #[test]
    fn hover_on_promoted_property_with_no_param_docblock_shows_type_only() {
        // When the constructor has a docblock but no @param for this promoted property,
        // hover should still work (showing type) without appending any docblock section.
        let src = "<?php\nclass User {\n    /**\n     * Create a user.\n     * @return void\n     */\n    public function __construct(\n        public string $name,\n    ) {}\n}\n$u = new User('Alice');\n$u->name";
        let doc = ParsedDoc::parse(src.to_string());
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(11, 4));
        assert!(h.is_some(), "expected hover on promoted property");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("string"), "should show type hint");
        assert!(
            !text.contains("---"),
            "should not append a docblock section"
        );
    }

    #[test]
    fn hover_on_use_alias_shows_fqn() {
        let src = "<?php\nuse App\\Mail\\Mailer;\n$m = new Mailer();";
        let doc = ParsedDoc::parse(src.to_string());
        let h = hover_at(
            src,
            &doc,
            &build_method_returns(&doc),
            &[],
            Position {
                line: 1,
                character: 20,
            },
        );
        assert!(h.is_some());
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("App\\Mail\\Mailer"), "should show full FQN");
    }

    #[test]
    fn hover_unknown_symbol_returns_none() {
        // `unknownFunc` is not defined anywhere — hover should return None.
        let src = "<?php\nunknownFunc();";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 3), &[]);
        assert!(
            result.is_none(),
            "hover on undefined symbol should return None"
        );
    }

    #[test]
    fn hover_on_builtin_function_returns_signature() {
        // `strlen` is a built-in function; hovering should return a non-empty
        // string that contains "strlen".
        let src = "<?php\nstrlen('hello');";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, &build_method_returns(&doc), pos(1, 3), &[]);
        let h = result.expect("expected hover result for built-in 'strlen'");
        let text = match h.contents {
            HoverContents::Markup(mc) => mc.value,
            _ => String::new(),
        };
        assert!(
            !text.is_empty(),
            "hover on strlen should return non-empty content"
        );
        assert!(
            text.contains("strlen"),
            "hover content should contain 'strlen', got: {text}"
        );
    }

    #[test]
    fn hover_on_property_shows_docblock() {
        let src = "<?php\nclass User {\n    /** The user's display name. */\n    public string $name;\n}\n$u = new User();\n$u->name";
        let doc = ParsedDoc::parse(src.to_string());
        // "name" in "$u->name" at the last line
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(6, 5));
        assert!(h.is_some(), "expected hover on property with docblock");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("User"), "should mention class name");
        assert!(text.contains("name"), "should mention property name");
        assert!(text.contains("string"), "should show type hint");
        assert!(
            text.contains("display name"),
            "should include docblock description, got: {}",
            text
        );
    }

    #[test]
    fn hover_on_property_with_var_tag_shows_type_annotation() {
        // A property with only `@var TypeHint` (no free-text description) must still
        // surface the @var annotation in the hover — it was previously swallowed because
        // to_markdown() never rendered var_type.
        let src = "<?php\nclass User {\n    /** @var string */\n    public $name;\n}\n$u = new User();\n$u->name";
        let doc = ParsedDoc::parse(src.to_string());
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(6, 5));
        assert!(h.is_some(), "expected hover on @var-only property");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(
            text.contains("@var"),
            "should show @var annotation, got: {}",
            text
        );
        assert!(
            text.contains("string"),
            "should show var type, got: {}",
            text
        );
    }

    #[test]
    fn hover_on_property_with_var_tag_and_description() {
        let src = "<?php\nclass User {\n    /** @var string The display name. */\n    public $name;\n}\n$u = new User();\n$u->name";
        let doc = ParsedDoc::parse(src.to_string());
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(6, 5));
        assert!(
            h.is_some(),
            "expected hover on property with @var description"
        );
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(
            text.contains("@var"),
            "should show @var annotation, got: {}",
            text
        );
        assert!(
            text.contains("The display name"),
            "should show @var description, got: {}",
            text
        );
    }

    #[test]
    fn hover_on_this_property_shows_type() {
        let src = "<?php\nclass Counter {\n    public int $count = 0;\n    public function increment(): void {\n        $this->count;\n    }\n}";
        let doc = ParsedDoc::parse(src.to_string());
        // "$this->count" — "count" starts at col 15 in "        $this->count;"
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(4, 16));
        assert!(h.is_some(), "expected hover on $this->property");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("Counter"), "should mention enclosing class");
        assert!(text.contains("count"), "should mention property name");
        assert!(text.contains("int"), "should show type hint");
    }

    #[test]
    fn hover_on_nullsafe_property_shows_type() {
        let src = "<?php\nclass Profile { public string $bio; }\n$p = new Profile();\n$p?->bio";
        let doc = ParsedDoc::parse(src.to_string());
        // "bio" in "$p?->bio" at line 3, col 5
        let h = hover_at(src, &doc, &build_method_returns(&doc), &[], pos(3, 5));
        assert!(h.is_some(), "expected hover on nullsafe property access");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("Profile"), "should mention class name");
        assert!(text.contains("bio"), "should mention property name");
        assert!(text.contains("string"), "should show type hint");
    }

    // ── Snapshot tests ───────────────────────────────────────────────────────

    use expect_test::{Expect, expect};

    fn check_hover(src: &str, position: Position, expect: Expect) {
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, &build_method_returns(&doc), position, &[]);
        let actual = match result {
            Some(Hover {
                contents: HoverContents::Markup(mc),
                ..
            }) => mc.value,
            Some(_) => "(non-markup hover)".to_string(),
            None => "(no hover)".to_string(),
        };
        expect.assert_eq(&actual);
    }

    #[test]
    fn snapshot_hover_simple_function() {
        check_hover(
            "<?php\nfunction init() {}",
            pos(1, 10),
            expect![[r#"
                ```php
                function init()
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_function_with_return_type() {
        check_hover(
            "<?php\nfunction greet(string $name): string {}",
            pos(1, 10),
            expect![[r#"
                ```php
                function greet(string $name): string
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_class() {
        check_hover(
            "<?php\nclass MyService {}",
            pos(1, 8),
            expect![[r#"
                ```php
                class MyService
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_class_with_extends() {
        check_hover(
            "<?php\nclass Dog extends Animal {}",
            pos(1, 8),
            expect![[r#"
                ```php
                class Dog extends Animal
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_method() {
        check_hover(
            "<?php\nclass Calc { public function add(int $a, int $b): int {} }",
            pos(1, 32),
            expect![[r#"
                ```php
                public function add(int $a, int $b): int
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_trait() {
        check_hover(
            "<?php\ntrait Loggable {}",
            pos(1, 8),
            expect![[r#"
                ```php
                trait Loggable
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_interface() {
        check_hover(
            "<?php\ninterface Serializable {}",
            pos(1, 12),
            expect![[r#"
                ```php
                interface Serializable
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_class_const_with_type_hint() {
        check_hover(
            "<?php\nclass Config { const string VERSION = '1.0.0'; }",
            pos(1, 28),
            expect![[r#"
                ```php
                const string VERSION = '1.0.0'
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_class_const_float_value() {
        check_hover(
            "<?php\nclass Math { const float PI = 3.14; }",
            pos(1, 27),
            expect![[r#"
                ```php
                const float PI = 3.14
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_class_const_infers_type_from_value() {
        let (src, p) = cursor("<?php\nclass Config { const VERSION$0 = '1.0.0'; }");
        check_hover(
            &src,
            p,
            expect![[r#"
                ```php
                const string VERSION = '1.0.0'
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_interface_const_shows_type_and_value() {
        let (src, p) = cursor("<?php\ninterface Limits { const int MA$0X = 100; }");
        check_hover(
            &src,
            p,
            expect![[r#"
                ```php
                const int MAX = 100
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_trait_const_shows_type_and_value() {
        let (src, p) = cursor("<?php\ntrait HasVersion { const string TAG$0 = 'v1'; }");
        check_hover(
            &src,
            p,
            expect![[r#"
                ```php
                const string TAG = 'v1'
                ```"#]],
        );
    }

    #[test]
    fn hover_on_catch_variable_shows_exception_class() {
        let (src, p) = cursor("<?php\ntry { } catch (RuntimeException $e$0) { }");
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, &build_method_returns(&doc), p, &[]);
        assert!(result.is_some(), "expected hover result for catch variable");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("RuntimeException"),
                "expected RuntimeException in hover, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_static_var_with_array_default_shows_array() {
        let (src, p) = cursor("<?php\nfunction counter() { static $cach$0e = []; }");
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, &build_method_returns(&doc), p, &[]);
        assert!(
            result.is_some(),
            "expected hover result for static variable"
        );
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("array"),
                "expected array type in hover, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_static_var_with_new_shows_class() {
        let (src, p) = cursor("<?php\nfunction make() { static $inst$0ance = new MyService(); }");
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, &build_method_returns(&doc), p, &[]);
        assert!(
            result.is_some(),
            "expected hover result for static variable"
        );
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("MyService"),
                "expected MyService in hover, got: {}",
                mc.value
            );
        }
    }

    // Gap 1: variables defined in one method must not pollute hover in another method.
    #[test]
    fn hover_variable_in_method_does_not_leak_across_methods() {
        // $result is defined as Widget in methodA but the cursor is in methodB.
        // Before the fix, $result from methodA would appear in methodB's hover.
        let (src, p) = cursor(concat!(
            "<?php\n",
            "class Service {\n",
            "    public function methodA(): void { $result = new Widget(); }\n",
            "    public function methodB(): void { $res$0ult = new Invoice(); }\n",
            "}\n",
        ));
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, &build_method_returns(&doc), p, &[]);
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                !mc.value.contains("Widget"),
                "Widget from methodA must not appear in methodB hover, got: {}",
                mc.value
            );
            assert!(
                mc.value.contains("Invoice"),
                "Invoice from methodB should appear in hover, got: {}",
                mc.value
            );
        }
    }

    // Gap 2: hovering `->method()` should show the signature for the correct class.
    #[test]
    fn hover_method_call_shows_correct_class_signature() {
        // Two classes both have a method named `process`. Hovering on `$mailer->process()`
        // should show Mailer::process, not Queue::process.
        let (src, p) = cursor(concat!(
            "<?php\n",
            "class Mailer { public function process(string $to): bool {} }\n",
            "class Queue  { public function process(int $id): void {} }\n",
            "$mailer = new Mailer();\n",
            "$mailer->proc$0ess();\n",
        ));
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, &build_method_returns(&doc), p, &[]);
        assert!(result.is_some(), "expected hover on method call");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("Mailer::process"),
                "should show Mailer::process, got: {}",
                mc.value
            );
            assert!(
                mc.value.contains("string $to"),
                "should show Mailer's params, got: {}",
                mc.value
            );
            assert!(
                !mc.value.contains("int $id"),
                "must NOT show Queue::process params, got: {}",
                mc.value
            );
        }
    }
}
