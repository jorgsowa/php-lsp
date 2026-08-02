//! AST-level class-structure queries: parent class, members (methods,
//! properties, constants, mixins), enclosing class at a cursor position, enum
//! backing type, and function/method parameter lists. These answer
//! structural facts directly from the parsed source and don't depend on mir.
use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
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
    /// Direct parent class name, if any.
    pub parent: Option<String>,
    /// Trait names used by this class (`use Foo, Bar;`).
    pub trait_uses: Vec<String>,
    /// True when a class/enum/trait with this name was found in the doc.
    /// Lets workspace-wide loops short-circuit once the defining doc is hit
    /// instead of continuing to scan every file.
    pub found: bool,
}

/// Return all members (methods, properties, constants) of `class_name`.
/// Also returns the direct parent class name via `ClassMembers::parent`.
pub fn members_of_class(doc: &ParsedDoc, class_name: &str) -> ClassMembers {
    let mut out = ClassMembers::default();
    out.parent = collect_members_stmts(doc.source(), &doc.program().stmts, class_name, &mut out);
    out
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
                            out.methods.push((
                                m.name.to_string(),
                                m.is_static,
                                !m.params.is_empty(),
                            ));
                            if m.name == "__construct" {
                                for p in m.params.iter() {
                                    if p.visibility.is_some() {
                                        out.properties.push((p.name.to_string(), false));
                                        if p.is_readonly || class_is_readonly {
                                            out.readonly_properties.push(p.name.to_string());
                                        }
                                    }
                                }
                            }
                        }
                        ClassMemberKind::Property(p) => {
                            out.properties.push((p.name.to_string(), p.is_static));
                            if p.is_readonly || class_is_readonly {
                                out.readonly_properties.push(p.name.to_string());
                            }
                        }
                        ClassMemberKind::ClassConst(c) => {
                            out.constants.push(c.name.to_string());
                        }
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
                        EnumMemberKind::Method(m) => {
                            out.methods.push((
                                m.name.to_string(),
                                m.is_static,
                                !m.params.is_empty(),
                            ));
                        }
                        EnumMemberKind::ClassConst(c) => {
                            out.constants.push(c.name.to_string());
                        }
                        _ => {}
                    }
                }
                return None; // enums have no parent class
            }
            StmtKind::Trait(t) if t.name == class_name => {
                out.found = true;
                for member in t.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            out.methods.push((
                                m.name.to_string(),
                                m.is_static,
                                !m.params.is_empty(),
                            ));
                        }
                        ClassMemberKind::Property(p) => {
                            out.properties.push((p.name.to_string(), p.is_static));
                        }
                        ClassMemberKind::ClassConst(c) => {
                            out.constants.push(c.name.to_string());
                        }
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
    let mut out = Vec::new();
    collect_method_params_stmts(&doc.program().stmts, class_name, method_name, &mut out);
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

/// Returns `true` if `class_name` is declared as an `enum` in `doc`.
pub fn is_enum(doc: &ParsedDoc, class_name: &str) -> bool {
    is_enum_in_stmts(&doc.program().stmts, class_name)
}

fn is_enum_in_stmts(stmts: &[Stmt<'_, '_>], name: &str) -> bool {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Enum(e) if e.name == name => return true,
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && is_enum_in_stmts(&inner.stmts, name)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Returns the declared backing type (`"string"` / `"int"`) of `class_name`
/// if it is a backed enum in `doc`, or `None` if it is not an enum, or is an
/// unbacked (pure) enum.
pub fn enum_backing_type(doc: &ParsedDoc, class_name: &str) -> Option<String> {
    enum_backing_type_in_stmts(&doc.program().stmts, class_name)
}

fn enum_backing_type_in_stmts(stmts: &[Stmt<'_, '_>], name: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Enum(e) if e.name == name => {
                return e
                    .scalar_type
                    .as_ref()
                    .map(|t| t.to_string_repr().to_string());
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(ty) = enum_backing_type_in_stmts(&inner.stmts, name)
                {
                    return Some(ty);
                }
            }
            _ => {}
        }
    }
    None
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
    fn is_enum_pure() {
        let src = "<?php\nenum Suit { case Hearts; case Clubs; }";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(is_enum(&doc, "Suit"));
        assert!(enum_backing_type(&doc, "Suit").is_none());
    }

    #[test]
    fn is_backed_enum_string() {
        let src = "<?php\nenum Status: string { case Active = 'active'; }";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(is_enum(&doc, "Status"));
        assert_eq!(
            enum_backing_type(&doc, "Status"),
            Some("string".to_string())
        );
    }

    #[test]
    fn is_enum_false_for_class() {
        let src = "<?php\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(!is_enum(&doc, "Foo"));
        assert!(enum_backing_type(&doc, "Foo").is_none());
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
