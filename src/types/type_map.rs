//! AST-level class-structure queries: parent class, members (methods,
//! properties, constants, mixins), enclosing class at a cursor position, enum
//! backing type, and function/method parameter lists. These answer
//! structural facts directly from the parsed source and don't depend on mir.
use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind, Visibility};
use tower_lsp_server::ls_types::Position;

use crate::document::ast::{ParsedDoc, SourceView};
use crate::lang::docblock::{docblock_before, parse_docblock};

pub fn parent_class_name(doc: &ParsedDoc, class_name: &str) -> Option<String> {
    parent_in_stmts(&doc.program().stmts, class_name)
}

fn parent_in_stmts(stmts: &[Stmt<'_, '_>], class_name: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c)
                if c.name.as_ref().map(|n| n.to_string()) == Some(class_name.to_string()) =>
            {
                return c.extends.as_ref().map(|n| n.to_string_repr().to_string());
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let found @ Some(_) = parent_in_stmts(&inner.stmts, class_name)
                {
                    return found;
                }
            }
            _ => {}
        }
    }
    None
}

/// All members of a named class split by kind and static-ness.
#[derive(Debug, Default)]
pub struct ClassMembers {
    /// (name, is_static, has_params)
    pub methods: Vec<(String, bool, bool)>,
    /// (name, is_static)
    pub properties: Vec<(String, bool)>,
    /// Names of readonly properties (PHP 8.1+).
    pub readonly_properties: Vec<String>,
    pub constants: Vec<String>,
    /// Non-public members as `(name, is_private)`; `false` means protected.
    pub restricted_methods: Vec<(String, bool)>,
    pub restricted_properties: Vec<(String, bool)>,
    pub restricted_constants: Vec<(String, bool)>,
    /// Direct parent class name, if any.
    pub parent: Option<String>,
    /// Trait names used by this class (`use Foo, Bar;`), or the parent
    /// interfaces of an interface.
    pub trait_uses: Vec<String>,
    /// True when a class/enum/trait with this name was found in the doc.
    /// Lets workspace-wide loops short-circuit once the defining doc is hit
    /// instead of continuing to scan every file.
    pub found: bool,
}

/// Return all members (methods, properties, constants) of `class_name`.
/// Also returns the direct parent class name via `ClassMembers::parent`.
pub fn members_of_class(doc: &ParsedDoc, class_name: &str) -> ClassMembers {
    let short = class_name.rsplit('\\').next().unwrap_or(class_name);
    let mut out = ClassMembers::default();
    out.parent = collect_members_stmts(doc.source(), &doc.program().stmts, short, &mut out);
    out
}

/// Fully-qualified name for `name` as written inside the declaration of
/// `declaring_class` in `doc`: resolved through that file's `use` imports and
/// namespace, not the viewer's.
pub fn resolve_class_ref(doc: &ParsedDoc, declaring_class: &str, name: &str) -> String {
    if let Some(rest) = name.strip_prefix('\\') {
        return rest.to_owned();
    }
    let (first, rest) = match name.split_once('\\') {
        Some((first, rest)) => (first, Some(rest)),
        None => (name, None),
    };
    let join = |base: &str| match rest {
        Some(rest) => format!("{base}\\{rest}"),
        None => base.to_owned(),
    };
    if let Some(target) = doc.file_imports().get(first) {
        return join(target.trim_start_matches('\\'));
    }
    let short = declaring_class
        .rsplit('\\')
        .next()
        .unwrap_or(declaring_class);
    match namespace_of_class(&doc.program().stmts, short, "") {
        Some(ns) if !ns.is_empty() => format!("{ns}\\{name}"),
        _ => name.to_owned(),
    }
}

fn namespace_of_class(stmts: &[Stmt<'_, '_>], short: &str, ns_prefix: &str) -> Option<String> {
    let mut current_ns = ns_prefix.to_owned();
    for stmt in stmts {
        let declared = match &stmt.kind {
            StmtKind::Class(c) => c.name.as_ref().is_some_and(|n| *n == short),
            StmtKind::Interface(i) => i.name == short,
            StmtKind::Trait(t) => t.name == short,
            StmtKind::Enum(e) => e.name == short,
            StmtKind::Namespace(ns) => {
                let ns_name = ns
                    .name
                    .as_ref()
                    .map(|n| n.to_string_repr().to_string())
                    .unwrap_or_default();
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        if let found @ Some(_) = namespace_of_class(&inner.stmts, short, &ns_name) {
                            return found;
                        }
                    }
                    NamespaceBody::Simple => current_ns = ns_name,
                }
                false
            }
            _ => false,
        };
        if declared {
            return Some(current_ns);
        }
    }
    None
}

/// `Some(true)` for private, `Some(false)` for protected, `None` for public.
fn restriction(visibility: Option<Visibility>) -> Option<bool> {
    match visibility {
        Some(Visibility::Private) => Some(true),
        Some(Visibility::Protected) => Some(false),
        _ => None,
    }
}

fn push_method(out: &mut ClassMembers, m: &php_ast::MethodDecl<'_, '_>) {
    out.methods
        .push((m.name.to_string(), m.is_static, !m.params.is_empty()));
    if let Some(private) = restriction(m.visibility) {
        out.restricted_methods.push((m.name.to_string(), private));
    }
}

fn push_property(out: &mut ClassMembers, p: &php_ast::PropertyDecl<'_, '_>) {
    out.properties.push((p.name.to_string(), p.is_static));
    if let Some(private) = restriction(p.visibility) {
        out.restricted_properties
            .push((p.name.to_string(), private));
    }
}

fn push_const(out: &mut ClassMembers, c: &php_ast::ClassConstDecl<'_, '_>) {
    out.constants.push(c.name.to_string());
    if let Some(private) = restriction(c.visibility) {
        out.restricted_constants.push((c.name.to_string(), private));
    }
}

fn collect_members_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    out: &mut ClassMembers,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c)
                if c.name.as_ref().map(|n| n.to_string()) == Some(class_name.to_string()) =>
            {
                out.found = true;
                // A `readonly class` makes every property readonly even when
                // the property itself carries no `readonly` keyword of its own.
                let class_is_readonly = c.modifiers.is_readonly;
                // Check docblock for @property and @method tags
                if let Some(raw) = docblock_before(source, stmt.span.start) {
                    let db = parse_docblock(&raw);
                    for prop in &db.properties {
                        out.properties.push((prop.name.clone(), false));
                    }
                    for method in &db.methods {
                        out.methods.push((
                            method.name.clone(),
                            method.is_static,
                            !method.params.is_empty(),
                        ));
                    }
                }
                for member in c.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            push_method(out, m);
                            if m.name == "__construct" {
                                for p in m.params.iter() {
                                    if p.visibility.is_some() {
                                        out.properties.push((p.name.to_string(), false));
                                        if let Some(private) = restriction(p.visibility) {
                                            out.restricted_properties
                                                .push((p.name.to_string(), private));
                                        }
                                        if p.is_readonly || class_is_readonly {
                                            out.readonly_properties.push(p.name.to_string());
                                        }
                                    }
                                }
                            }
                        }
                        ClassMemberKind::Property(p) => {
                            push_property(out, p);
                            if p.is_readonly || class_is_readonly {
                                out.readonly_properties.push(p.name.to_string());
                            }
                        }
                        ClassMemberKind::ClassConst(c) => push_const(out, c),
                        ClassMemberKind::TraitUse(t) => {
                            for name in t.traits.iter() {
                                out.trait_uses.push(name.to_string_repr().to_string());
                            }
                        }
                    }
                }
                return c.extends.as_ref().map(|n| n.to_string_repr().to_string());
            }
            StmtKind::Enum(e) if e.name == class_name => {
                out.found = true;
                let is_backed = e.scalar_type.is_some();
                out.properties.push(("name".to_string(), false));
                if is_backed {
                    out.properties.push(("value".to_string(), false));
                }
                out.methods.push(("cases".to_string(), true, false));
                if is_backed {
                    out.methods.push(("from".to_string(), true, true));
                    out.methods.push(("tryFrom".to_string(), true, true));
                }
                for member in e.body.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Case(c) => {
                            out.constants.push(c.name.to_string());
                        }
                        EnumMemberKind::Method(m) => push_method(out, m),
                        EnumMemberKind::ClassConst(c) => push_const(out, c),
                        _ => {}
                    }
                }
                return None; // enums have no parent class
            }
            StmtKind::Interface(i) if i.name == class_name => {
                out.found = true;
                for member in i.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => push_method(out, m),
                        ClassMemberKind::Property(p) => push_property(out, p),
                        ClassMemberKind::ClassConst(c) => push_const(out, c),
                        ClassMemberKind::TraitUse(_) => {}
                    }
                }
                for parent in i.extends.iter() {
                    out.trait_uses.push(parent.to_string_repr().to_string());
                }
                return None;
            }
            StmtKind::Trait(t) if t.name == class_name => {
                out.found = true;
                for member in t.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => push_method(out, m),
                        ClassMemberKind::Property(p) => push_property(out, p),
                        ClassMemberKind::ClassConst(c) => push_const(out, c),
                        ClassMemberKind::TraitUse(t) => {
                            for name in t.traits.iter() {
                                out.trait_uses.push(name.to_string_repr().to_string());
                            }
                        }
                    }
                }
                return None; // traits have no parent
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    let result = collect_members_stmts(source, &inner.stmts, class_name, out);
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

/// Return the `@mixin` class names declared in `class_name`'s docblock.
pub fn mixin_classes_of(doc: &ParsedDoc, class_name: &str) -> Vec<String> {
    let source = doc.source();
    mixin_classes_in_stmts(source, &doc.program().stmts, class_name)
}

fn mixin_classes_in_stmts(source: &str, stmts: &[Stmt<'_, '_>], class_name: &str) -> Vec<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c)
                if c.name.as_ref().map(|n| n.to_string()) == Some(class_name.to_string()) =>
            {
                if let Some(raw) = docblock_before(source, stmt.span.start) {
                    return parse_docblock(&raw).mixins;
                }
                return vec![];
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    let found = mixin_classes_in_stmts(source, &inner.stmts, class_name);
                    if !found.is_empty() {
                        return found;
                    }
                }
            }
            _ => {}
        }
    }
    vec![]
}

/// Return the name of the class whose body contains `position`, or `None`.
pub fn enclosing_class_at(_source: &str, doc: &ParsedDoc, position: Position) -> Option<String> {
    let sv = doc.view();
    enclosing_class_in_stmts(sv, &doc.program().stmts, position)
}

/// Like [`enclosing_class_at`] but returns the fully-qualified name
/// (`"Ns\\ClassName"`) when the class lives inside a namespace.
/// Used by the `__construct` call-site path so that `construct_references`
/// can apply namespace-level filtering and avoid matching a same-short-named
/// class in a different namespace.
pub fn enclosing_class_fqn_at(
    _source: &str,
    doc: &ParsedDoc,
    position: Position,
) -> Option<String> {
    let sv = doc.view();
    enclosing_class_fqn_in_stmts(sv, &doc.program().stmts, position, "")
}

fn enclosing_class_fqn_in_stmts(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    pos: Position,
    ns_prefix: &str,
) -> Option<String> {
    let make_fqn = |ns: &str, short: &str| -> String {
        if ns.is_empty() {
            short.to_owned()
        } else {
            format!("{}\\{}", ns, short)
        }
    };
    let mut current_ns = ns_prefix.to_owned();
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let start = sv.position_of(stmt.span.start).line;
                let end = sv.position_of(stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return c.name.map(|n| make_fqn(&current_ns, &n.to_string()));
                }
            }
            StmtKind::Interface(i) => {
                let start = sv.position_of(stmt.span.start).line;
                let end = sv.position_of(stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return Some(make_fqn(&current_ns, &i.name.to_string()));
                }
            }
            StmtKind::Trait(t) => {
                let start = sv.position_of(stmt.span.start).line;
                let end = sv.position_of(stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return Some(make_fqn(&current_ns, &t.name.to_string()));
                }
            }
            StmtKind::Enum(e) => {
                let start = sv.position_of(stmt.span.start).line;
                let end = sv.position_of(stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return Some(make_fqn(&current_ns, &e.name.to_string()));
                }
            }
            StmtKind::Namespace(ns) => {
                let ns_name = ns
                    .name
                    .as_ref()
                    .map(|n| n.to_string_repr().to_string())
                    .unwrap_or_default();
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        if let Some(found) =
                            enclosing_class_fqn_in_stmts(sv, &inner.stmts, pos, &ns_name)
                        {
                            return Some(found);
                        }
                    }
                    NamespaceBody::Simple => {
                        current_ns = ns_name;
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Return the LSP range of the class/interface/trait/enum declaration
/// whose body contains `position`, or `None` if the cursor is outside any.
/// Used by linked-editing to scope same-name member rewrites to the
/// enclosing class instead of every class in the file.
pub fn enclosing_class_range_at(
    doc: &ParsedDoc,
    position: Position,
) -> Option<tower_lsp_server::ls_types::Range> {
    let sv = doc.view();
    enclosing_class_range_in_stmts(sv, &doc.program().stmts, position)
}

/// Return the LSP range of every class/interface/trait/enum declaration in
/// the file (recursing into braced-namespace bodies). Used by linked-editing
/// to drop highlights that fall inside an *other* class than the cursor's.
pub fn collect_all_class_ranges(doc: &ParsedDoc) -> Vec<tower_lsp_server::ls_types::Range> {
    let sv = doc.view();
    let mut out = Vec::new();
    collect_class_ranges_in_stmts(sv, &doc.program().stmts, &mut out);
    out
}

fn collect_class_ranges_in_stmts(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    out: &mut Vec<tower_lsp_server::ls_types::Range>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(_)
            | StmtKind::Interface(_)
            | StmtKind::Trait(_)
            | StmtKind::Enum(_) => {
                out.push(sv.range_of(stmt.span));
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_class_ranges_in_stmts(sv, &inner.stmts, out);
                }
            }
            _ => {}
        }
    }
}

fn enclosing_class_range_in_stmts(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    pos: Position,
) -> Option<tower_lsp_server::ls_types::Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(_)
            | StmtKind::Interface(_)
            | StmtKind::Trait(_)
            | StmtKind::Enum(_) => {
                let r = sv.range_of(stmt.span);
                if pos.line >= r.start.line && pos.line <= r.end.line {
                    return Some(r);
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = enclosing_class_range_in_stmts(sv, &inner.stmts, pos)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn enclosing_class_in_stmts(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    pos: Position,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let start = sv.position_of(stmt.span.start).line;
                let end = sv.position_of(stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return c.name.map(|n| n.to_string());
                }
            }
            StmtKind::Interface(i) => {
                let start = sv.position_of(stmt.span.start).line;
                let end = sv.position_of(stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return Some(i.name.to_string());
                }
            }
            StmtKind::Trait(t) => {
                let start = sv.position_of(stmt.span.start).line;
                let end = sv.position_of(stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return Some(t.name.to_string());
                }
            }
            StmtKind::Enum(e) => {
                let start = sv.position_of(stmt.span.start).line;
                let end = sv.position_of(stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return Some(e.name.to_string());
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(found) = enclosing_class_in_stmts(sv, &inner.stmts, pos)
                {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// Return the parameter names of the function or method named `func_name`.
pub fn params_of_function(doc: &ParsedDoc, func_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_params_stmts(&doc.program().stmts, func_name, &mut out);
    out
}

/// Return the parameter names of `method_name` on class `class_name`.
/// Primarily used to offer named-argument completions for attribute constructors.
pub fn params_of_method(doc: &ParsedDoc, class_name: &str, method_name: &str) -> Vec<String> {
    let short = class_name.rsplit('\\').next().unwrap_or(class_name);
    let mut out = Vec::new();
    collect_method_params_stmts(&doc.program().stmts, short, method_name, &mut out);
    out
}

fn collect_method_params_stmts(
    stmts: &[php_ast::Stmt<'_, '_>],
    class_name: &str,
    method_name: &str,
    out: &mut Vec<String>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c)
                if c.name.as_ref().map(|n| n.to_string()) == Some(class_name.to_string()) =>
            {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        for p in m.params.iter() {
                            out.push(p.name.to_string());
                        }
                        return;
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_method_params_stmts(&inner.stmts, class_name, method_name, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_params_stmts(stmts: &[Stmt<'_, '_>], func_name: &str, out: &mut Vec<String>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == func_name => {
                for p in f.params.iter() {
                    out.push(p.name.to_string());
                }
                return;
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == func_name
                    {
                        for p in m.params.iter() {
                            out.push(p.name.to_string());
                        }
                        return;
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_params_stmts(&inner.stmts, func_name, out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_class_name_finds_parent() {
        let src = "<?php\nclass Base {}\nclass Child extends Base {}";
        let doc = ParsedDoc::parse(src.to_string());
        assert_eq!(parent_class_name(&doc, "Child"), Some("Base".to_string()));
    }

    #[test]
    fn parent_class_name_returns_none_for_top_level() {
        let src = "<?php\nclass Base {}";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(parent_class_name(&doc, "Base").is_none());
    }

    #[test]
    fn members_of_class_includes_parent_field() {
        let src = "<?php\nclass Base {}\nclass Child extends Base {}";
        let doc = ParsedDoc::parse(src.to_string());
        let m = members_of_class(&doc, "Child");
        assert_eq!(m.parent.as_deref(), Some("Base"));
    }

    #[test]
    fn members_of_class_finds_methods() {
        let src = "<?php\nclass Calc { public function add() {} public function sub() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Calc");
        let names: Vec<&str> = members.methods.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(names.contains(&"add"), "missing 'add'");
        assert!(names.contains(&"sub"), "missing 'sub'");
    }

    #[test]
    fn members_of_class_tracks_has_params_per_method() {
        let src = "<?php\nclass Calc { public function reset() {} public function add(int $x) {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Calc");
        let has_params = |name: &str| {
            members
                .methods
                .iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, _, p)| *p)
        };
        assert_eq!(has_params("reset"), Some(false));
        assert_eq!(has_params("add"), Some(true));
    }

    #[test]
    fn members_of_unknown_class_is_empty() {
        let src = "<?php\nclass Calc { public function add() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Unknown");
        assert!(members.methods.is_empty());
    }

    #[test]
    fn constructor_promoted_params_appear_as_properties() {
        let src = "<?php\nclass Point {\n    public function __construct(\n        public float $x,\n        public float $y,\n    ) {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Point");
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            prop_names.contains(&"x"),
            "promoted param x should be a property"
        );
        assert!(
            prop_names.contains(&"y"),
            "promoted param y should be a property"
        );
    }

    #[test]
    fn promoted_readonly_params_appear_in_readonly_properties() {
        let src = "<?php\nclass User {\n    public function __construct(\n        public readonly string $name,\n        public int $age,\n    ) {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "User");
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            prop_names.contains(&"name"),
            "promoted param name should be a property"
        );
        assert!(
            prop_names.contains(&"age"),
            "promoted param age should be a property"
        );
        assert!(
            members.readonly_properties.contains(&"name".to_string()),
            "readonly promoted param name should be in readonly_properties"
        );
        assert!(
            !members.readonly_properties.contains(&"age".to_string()),
            "non-readonly promoted param age should not be in readonly_properties"
        );
    }

    #[test]
    fn enum_instance_members_include_name() {
        let src = "<?php\nenum Status { case Active; case Inactive; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Status");
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            prop_names.contains(&"name"),
            "pure enum should expose ->name"
        );
        assert!(
            !prop_names.contains(&"value"),
            "pure enum should not expose ->value"
        );
    }

    #[test]
    fn backed_enum_exposes_value_and_factory_methods() {
        let src = "<?php\nenum Color: string { case Red = 'red'; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Color");
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        let method_names: Vec<&str> = members.methods.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(
            prop_names.contains(&"value"),
            "backed enum should expose ->value"
        );
        assert!(
            method_names.contains(&"from"),
            "backed enum should have ::from()"
        );
        assert!(
            method_names.contains(&"tryFrom"),
            "backed enum should have ::tryFrom()"
        );
        assert!(
            method_names.contains(&"cases"),
            "enum should have ::cases()"
        );
    }

    #[test]
    fn enum_cases_appear_as_constants() {
        let src = "<?php\nenum Status { case Active; case Inactive; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Status");
        assert!(members.constants.contains(&"Active".to_string()));
        assert!(members.constants.contains(&"Inactive".to_string()));
    }

    #[test]
    fn trait_members_are_collected() {
        let src = "<?php\ntrait Logging { public function log() {} public string $logFile; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Logging");
        let method_names: Vec<&str> = members.methods.iter().map(|(n, _, _)| n.as_str()).collect();
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            method_names.contains(&"log"),
            "trait method log should be collected"
        );
        assert!(
            prop_names.contains(&"logFile"),
            "trait property logFile should be collected"
        );
    }

    #[test]
    fn class_with_trait_use_lists_trait() {
        let src = "<?php\ntrait Logging { public function log() {} }\nclass App { use Logging; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "App");
        assert!(
            members.trait_uses.contains(&"Logging".to_string()),
            "should list used trait"
        );
    }

    #[test]
    fn docblock_property_appears_in_members() {
        let src =
            "<?php\n/**\n * @property string $email\n * @property-read int $id\n */\nclass User {}";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "User");
        let props: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(props.contains(&"email"));
        assert!(props.contains(&"id"));
    }

    #[test]
    fn docblock_method_appears_in_members() {
        let src = "<?php\n/**\n * @method User find(int $id)\n * @method static Builder where(string $col, mixed $val)\n */\nclass Model {}";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Model");
        let method_names: Vec<&str> = members.methods.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(method_names.contains(&"find"));
        assert!(method_names.contains(&"where"));
        let where_static = members
            .methods
            .iter()
            .find(|(n, _, _)| n == "where")
            .map(|(_, s, _)| *s);
        assert_eq!(where_static, Some(true));
        let find_has_params = members
            .methods
            .iter()
            .find(|(n, _, _)| n == "find")
            .map(|(_, _, p)| *p);
        assert_eq!(find_has_params, Some(true));
    }
}
