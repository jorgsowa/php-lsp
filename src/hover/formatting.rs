use php_ast::{MethodDecl, Param, Visibility};
use tower_lsp_server::ls_types::{Hover, HoverContents, MarkupContent, MarkupKind};

use crate::document::ast::format_type_hint;
use crate::lang::php_names::{is_php_builtin, php_doc_url};
use crate::types::resolve::Declaration;

/// Format an expression literal value.
pub(crate) fn format_expr_literal(expr: &php_ast::Expr<'_, '_>) -> Option<String> {
    use php_ast::ExprKind;
    match &expr.kind {
        ExprKind::Int(n) => Some(n.to_string()),
        ExprKind::Float(f) => Some(f.to_string()),
        ExprKind::Bool(b) => Some(if *b { "true" } else { "false" }.to_string()),
        ExprKind::String(s) => Some(format!("'{}'", s)),
        _ => None,
    }
}

/// Format a class/interface/enum constant declaration for hover display.
pub(crate) fn format_class_const(c: &php_ast::ClassConstDecl<'_, '_>) -> String {
    use php_ast::ExprKind;
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

pub fn format_params_str(params: &[Param<'_, '_>]) -> String {
    format_params(params)
}

pub(crate) fn format_params(params: &[Param<'_, '_>]) -> String {
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
pub(crate) fn format_default_value(expr: &php_ast::Expr<'_, '_>) -> String {
    use php_ast::ExprKind;
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

pub(crate) fn wrap_php(sig: &str) -> String {
    format!("```php\n{}\n```", sig)
}

/// Format a method/function-style member signature, e.g.
/// `public static function foo(int $x): void`.
pub(crate) fn method_signature(m: &MethodDecl<'_, '_>) -> String {
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
    format!("{}function {}({}){}", prefix, m.name, params, ret)
}

/// Render the hover signature for a resolved declaration. Returns `None` for
/// kinds rendered elsewhere (properties via mir-primary path).
pub(crate) fn declaration_signature(decl: &Declaration<'_>, word: &str) -> Option<String> {
    let sig = match decl {
        Declaration::Function { decl: f, .. } => {
            let params = format_params(&f.params);
            let ret = f
                .return_type
                .as_ref()
                .map(|r| format!(": {}", format_type_hint(r)))
                .unwrap_or_default();
            format!("function {}({}){}", word, params, ret)
        }
        Declaration::Class { decl: c, .. } => {
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
            sig
        }
        Declaration::Interface { .. } => format!("interface {}", word),
        Declaration::Trait { .. } => format!("trait {}", word),
        Declaration::Enum { decl: e, .. } => {
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
            sig
        }
        Declaration::Method { method, .. } => method_signature(method),
        Declaration::ClassConst { konst, .. } => format_class_const(konst),
        Declaration::EnumCase {
            case, enum_name, ..
        } => {
            let value_str = case
                .value
                .as_ref()
                .and_then(format_expr_literal)
                .map(|v| format!(" = {v}"))
                .unwrap_or_default();
            format!("case {}::{}{}", enum_name, case.name, value_str)
        }
        Declaration::Property { .. } | Declaration::PromotedParam { .. } => return None,
    };
    Some(sig)
}

fn visibility_str(v: &Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

pub(crate) fn format_method_prefix(
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

pub(crate) fn format_prop_prefix(
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

/// Return a function/method signature string from a `FileIndex` slice.
/// A class whose FQN or short name matches `class_hint` (case-insensitive,
/// ignoring a leading `\`). Empty/absent hint matches every class — the
/// original unscoped behavior.
fn class_matches_hint(cls: &crate::index::file_index::ClassDef, class_hint: Option<&str>) -> bool {
    let Some(hint) = class_hint else { return true };
    let hint = hint.trim_start_matches('\\');
    cls.fqn.trim_start_matches('\\').eq_ignore_ascii_case(hint)
        || cls.name.eq_ignore_ascii_case(hint)
}

fn signature_for_symbol_in_index(
    name: &str,
    idx: &crate::index::file_index::FileIndex,
    class_hint: Option<&str>,
) -> Option<String> {
    if class_hint.is_none() {
        for f in &idx.functions {
            if f.name.as_ref() == name {
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
    }
    for cls in &idx.classes {
        if !class_matches_hint(cls, class_hint) {
            continue;
        }
        for m in &cls.methods {
            if m.name.as_ref() == name {
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
    None
}

fn docs_for_symbol_in_index(
    name: &str,
    idx: &crate::index::file_index::FileIndex,
    class_hint: Option<&str>,
) -> Option<String> {
    let sig = signature_for_symbol_in_index(name, idx, class_hint)?;
    let mut value = wrap_php(&sig);
    if class_hint.is_none() {
        for f in &idx.functions {
            if f.name.as_ref() == name {
                if let Some(raw) = &f.docblock {
                    let db = crate::lang::docblock::parse_docblock(raw);
                    let md = db.to_markdown();
                    if !md.is_empty() {
                        value.push_str("\n\n---\n\n");
                        value.push_str(&md);
                    }
                }
                return Some(value);
            }
        }
    }
    for cls in &idx.classes {
        if !class_matches_hint(cls, class_hint) {
            continue;
        }
        for m in &cls.methods {
            if m.name.as_ref() == name {
                if let Some(raw) = &m.docblock {
                    let db = crate::lang::docblock::parse_docblock(raw);
                    let md = db.to_markdown();
                    if !md.is_empty() {
                        value.push_str("\n\n---\n\n");
                        value.push_str(&md);
                    }
                }
                return Some(value);
            }
        }
    }
    Some(value)
}

pub fn signature_for_symbol_from_index(
    name: &str,
    indexes: &[(
        tower_lsp_server::ls_types::Uri,
        std::sync::Arc<crate::index::file_index::FileIndex>,
    )],
) -> Option<String> {
    signature_for_symbol_from_index_scoped(name, indexes, None)
}

/// Like [`signature_for_symbol_from_index`], but when `class_hint` is given,
/// only a method whose owning class matches it is considered — disambiguates
/// same-named methods on unrelated classes (e.g. two classes both declaring
/// `save()`) instead of returning whichever one is indexed first. A hint
/// also skips the free-function search entirely, since a hint means the
/// symbol is known to be a method.
pub fn signature_for_symbol_from_index_scoped(
    name: &str,
    indexes: &[(
        tower_lsp_server::ls_types::Uri,
        std::sync::Arc<crate::index::file_index::FileIndex>,
    )],
    class_hint: Option<&str>,
) -> Option<String> {
    for (_, idx) in indexes {
        if let Some(sig) = signature_for_symbol_in_index(name, idx, class_hint) {
            return Some(sig);
        }
    }
    None
}

/// Return hover documentation for a symbol from a `FileIndex` slice.
pub fn docs_for_symbol_from_index(
    name: &str,
    indexes: &[(
        tower_lsp_server::ls_types::Uri,
        std::sync::Arc<crate::index::file_index::FileIndex>,
    )],
) -> Option<String> {
    docs_for_symbol_from_index_scoped(name, indexes, None)
}

/// Like [`docs_for_symbol_from_index`], but scoped to a class hint — see
/// [`signature_for_symbol_from_index_scoped`]. Also skips the PHP-builtin
/// fallback when a class hint is given, since a hint means the symbol is
/// known to be a user-defined method, not a global builtin function.
pub fn docs_for_symbol_from_index_scoped(
    name: &str,
    indexes: &[(
        tower_lsp_server::ls_types::Uri,
        std::sync::Arc<crate::index::file_index::FileIndex>,
    )],
    class_hint: Option<&str>,
) -> Option<String> {
    if let Some(sig) = signature_for_symbol_from_index_scoped(name, indexes, class_hint) {
        let mut value = wrap_php(&sig);
        for (_, idx) in indexes {
            if let Some(doc) = docs_for_symbol_in_index(name, idx, class_hint) {
                value = doc;
                break;
            }
        }
        if class_hint.is_none() && is_php_builtin(name) {
            value.push_str(&format!(
                "\n\n[php.net documentation]({})",
                php_doc_url(name)
            ));
        }
        return Some(value);
    }
    if class_hint.is_none() && is_php_builtin(name) {
        return Some(format!(
            "```php\nfunction {}()\n```\n\n[php.net documentation]({})",
            name,
            php_doc_url(name)
        ));
    }
    None
}

/// Workspace-index-aware symbol signature lookup: resolves a class-scoped
/// method directly through `resolve_class_ref`, and narrows unscoped
/// function/method lookups to mir's candidate file set rather than scanning
/// every indexed file.
pub fn signature_for_symbol_from_workspace_index_scoped(
    name: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    class_hint: Option<&str>,
    resolve_class_ref: &dyn Fn(&str) -> Option<crate::db::workspace_index::ClassRef>,
    declaration_candidates: &dyn Fn(&str) -> Vec<tower_lsp_server::ls_types::Uri>,
) -> Option<String> {
    if let Some(class_hint) = class_hint {
        let cr = resolve_class_ref(class_hint)?;
        let (_, cls) = wi.at(cr)?;
        let idx = crate::index::file_index::FileIndex {
            namespace: None,
            functions: Vec::new(),
            classes: vec![cls.clone()],
            constants: Vec::new(),
            use_imports: Vec::new(),
        };
        return signature_for_symbol_in_index(name, &idx, Some(class_hint));
    }
    for uri in declaration_candidates(name) {
        let Some(file_idx) = wi.path_to_file_idx.get(uri.as_str()).copied() else {
            continue;
        };
        let (_, idx) = &wi.files[file_idx as usize];
        if let Some(sig) = signature_for_symbol_in_index(name, idx, None) {
            return Some(sig);
        }
    }
    None
}

/// Workspace-index-aware symbol documentation lookup; same narrowing strategy
/// as [`signature_for_symbol_from_workspace_index_scoped`].
pub fn docs_for_symbol_from_workspace_index_scoped(
    name: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    class_hint: Option<&str>,
    resolve_class_ref: &dyn Fn(&str) -> Option<crate::db::workspace_index::ClassRef>,
    declaration_candidates: &dyn Fn(&str) -> Vec<tower_lsp_server::ls_types::Uri>,
) -> Option<String> {
    if let Some(class_hint) = class_hint {
        let cr = resolve_class_ref(class_hint)?;
        let (_, cls) = wi.at(cr)?;
        let idx = crate::index::file_index::FileIndex {
            namespace: None,
            functions: Vec::new(),
            classes: vec![cls.clone()],
            constants: Vec::new(),
            use_imports: Vec::new(),
        };
        return docs_for_symbol_in_index(name, &idx, Some(class_hint));
    }
    for uri in declaration_candidates(name) {
        let Some(file_idx) = wi.path_to_file_idx.get(uri.as_str()).copied() else {
            continue;
        };
        let (_, idx) = &wi.files[file_idx as usize];
        if let Some(doc) = docs_for_symbol_in_index(name, idx, None) {
            return Some(if is_php_builtin(name) {
                format!("{doc}\n\n[php.net documentation]({})", php_doc_url(name))
            } else {
                doc
            });
        }
    }
    if is_php_builtin(name) {
        return Some(format!(
            "```php\nfunction {}()\n```\n\n[php.net documentation]({})",
            name,
            php_doc_url(name)
        ));
    }
    None
}

/// Build a hover for `method_name` on `cls`, if declared there. Shared by the
/// O(1) `resolve_class_ref` path and the legacy linear scan below.
fn method_hover_from_class(
    class_name: &str,
    method_name: &str,
    cls: &crate::index::file_index::ClassDef,
) -> Option<Hover> {
    for m in &cls.methods {
        if m.name.as_ref() != method_name {
            continue;
        }
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
        let sig = format!("{}::{}({}){}", class_name, method_name, params_str, ret);
        let mut value = wrap_php(&sig);
        if let Some(raw) = &m.docblock {
            let db = crate::lang::docblock::parse_docblock(raw);
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
            range: None,
        });
    }
    None
}

/// Build a hover for a static method call found by class short name + method name
/// in the workspace index. Used when the primary mir path cannot resolve a cross-file
/// static call (e.g. `Str::camel(…)` where `Str` is only known through a `use`-import).
pub fn method_hover_from_index(
    class_name: &str,
    method_name: &str,
    indexes: &[(
        tower_lsp_server::ls_types::Uri,
        std::sync::Arc<crate::index::file_index::FileIndex>,
    )],
) -> Option<Hover> {
    for (_, idx) in indexes {
        for cls in &idx.classes {
            if cls.name.as_ref() != class_name
                && crate::text::fqn_short_name(cls.fqn.as_ref()) != class_name
            {
                continue;
            }
            if let Some(h) = method_hover_from_class(class_name, method_name, cls) {
                return Some(h);
            }
        }
    }
    None
}

/// O(candidates) variant of [`class_hover_from_index`]: resolves `word`/`fqn`
/// via `resolve_class_ref` (typically `DocumentStore::resolve_class_ref`)
/// first, falling back to the full linear scan only when that doesn't
/// resolve the class — preserves every edge case the scan covers (e.g.
/// names the mention index doesn't disambiguate the same way), just skips
/// the common-case cost.
pub fn class_hover_from_workspace_index(
    word: &str,
    fqn: Option<&str>,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    resolve_class_ref: &dyn Fn(&str) -> Option<crate::db::workspace_index::ClassRef>,
) -> Option<Hover> {
    if let Some(cr) = resolve_class_ref(fqn.unwrap_or(word))
        && let Some((_, cls)) = wi.at(cr)
    {
        return Some(class_hover_for(cls));
    }
    class_hover_from_index(word, fqn, &wi.files)
}

/// O(candidates) variant of [`method_hover_from_index`], same fallback
/// contract as [`class_hover_from_workspace_index`].
pub fn method_hover_from_workspace_index(
    class_name: &str,
    method_name: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    resolve_class_ref: &dyn Fn(&str) -> Option<crate::db::workspace_index::ClassRef>,
) -> Option<Hover> {
    if let Some(cr) = resolve_class_ref(class_name)
        && let Some((_, cls)) = wi.at(cr)
        && let Some(h) = method_hover_from_class(class_name, method_name, cls)
    {
        return Some(h);
    }
    method_hover_from_index(class_name, method_name, &wi.files)
}

/// Build a hover for a class/interface/trait/enum found by short name in the workspace index.
///
/// `fqn` is the fully-qualified name the caller resolved `word` to (e.g. via
/// a `use ... as Alias` import), when known. Across a large workspace many
/// unrelated classes can share a short name (and even the same local alias),
/// so when `fqn` is available a candidate whose own `cls.fqn` matches it
/// exactly is preferred over the first short-name match found. Falls back to
/// the first short-name match when `fqn` is `None` (no `use` import to
/// disambiguate a genuinely ambiguous bare reference).
pub fn class_hover_from_index(
    word: &str,
    fqn: Option<&str>,
    indexes: &[(
        tower_lsp_server::ls_types::Uri,
        std::sync::Arc<crate::index::file_index::FileIndex>,
    )],
) -> Option<Hover> {
    let mut first_match: Option<&crate::index::file_index::ClassDef> = None;
    for (_, idx) in indexes {
        for cls in &idx.classes {
            if cls.name.as_ref() == word || cls.fqn.as_ref().trim_start_matches('\\') == word {
                if let Some(target) = fqn {
                    if cls.fqn.as_ref().trim_start_matches('\\') == target {
                        return Some(class_hover_for(cls));
                    }
                    if first_match.is_none() {
                        first_match = Some(cls);
                    }
                    continue;
                }
                return Some(class_hover_for(cls));
            }
        }
    }
    first_match.map(class_hover_for)
}

fn class_hover_for(cls: &crate::index::file_index::ClassDef) -> Hover {
    use crate::index::file_index::ClassKind;

    let kw = match cls.kind {
        ClassKind::Interface => "interface",
        ClassKind::Trait => "trait",
        ClassKind::Enum => "enum",
        ClassKind::Class => {
            if cls.is_abstract {
                "abstract class"
            } else if cls.is_readonly {
                "readonly class"
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
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: wrap_php(&sig),
        }),
        range: None,
    }
}
