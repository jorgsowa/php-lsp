/// Single-pass type inference: collects `$var = new ClassName()` assignments
/// to map variable names to class names.  Used to scope method completions
/// after `->`.
use std::collections::HashMap;

use php_ast::{ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind, TypeHintKind};
use tower_lsp::lsp_types::Position;

use crate::ast::{ParsedDoc, offset_to_position};
use crate::docblock::{docblock_before, parse_docblock};
use crate::phpstorm_meta::PhpStormMeta;

/// Maps variable name (with `$`) → class name.
#[derive(Debug, Default, Clone)]
pub struct TypeMap(HashMap<String, String>);

impl TypeMap {
    /// Build from a parsed document.
    pub fn from_doc(doc: &ParsedDoc) -> Self {
        Self::from_doc_with_meta(doc, None)
    }

    /// Build from a parsed document, optionally enriched by PHPStorm metadata
    /// for factory-method return type inference.
    pub fn from_doc_with_meta(doc: &ParsedDoc, meta: Option<&PhpStormMeta>) -> Self {
        let mut map = HashMap::new();
        collect_types_stmts(doc.source(), &doc.program().stmts, &mut map, meta);
        TypeMap(map)
    }

    /// Returns the class name for a variable, e.g. `get("$obj")` → `Some("Foo")`.
    pub fn get<'a>(&'a self, var: &str) -> Option<&'a str> {
        self.0.get(var).map(|s| s.as_str())
    }
}

fn collect_types_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    map: &mut HashMap<String, String>,
    meta: Option<&PhpStormMeta>,
) {
    for stmt in stmts {
        // Check for `/** @var ClassName $varName */` docblock before this statement.
        if let Some(raw) = docblock_before(source, stmt.span.start) {
            let db = parse_docblock(&raw);
            if let Some(type_str) = db.var_type {
                // Only map object types (starts with uppercase or backslash).
                let base = type_str.trim_start_matches('\\').trim_start_matches('?');
                let first_char = base.chars().next().unwrap_or('_');
                if first_char.is_uppercase() {
                    let class_name = base.rsplit('\\').next().unwrap_or(base).to_string();
                    if let Some(vname) = db.var_name {
                        // `@var Foo $obj` — explicit variable name.
                        map.insert(format!("${vname}"), class_name);
                    } else if let StmtKind::Expression(e) = &stmt.kind {
                        // `@var Foo` above `$obj = ...` — infer from the LHS.
                        if let ExprKind::Assign(a) = &e.kind {
                            if let ExprKind::Variable(vn) = &a.target.kind {
                                map.insert(format!("${vn}"), class_name);
                            }
                        }
                    }
                }
            }
        }

        match &stmt.kind {
            StmtKind::Expression(e) => collect_types_expr(e, map, meta),
            StmtKind::Function(f) => {
                for p in f.params.iter() {
                    if let Some(hint) = &p.type_hint {
                        if let TypeHintKind::Named(name) = &hint.kind {
                            map.insert(format!("${}", p.name), name.to_string_repr().to_string());
                        }
                    }
                }
                collect_types_stmts(source, &f.body, map, meta);
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        for p in m.params.iter() {
                            if let Some(hint) = &p.type_hint {
                                if let TypeHintKind::Named(name) = &hint.kind {
                                    map.insert(
                                        format!("${}", p.name),
                                        name.to_string_repr().to_string(),
                                    );
                                }
                            }
                        }
                        if let Some(body) = &m.body {
                            collect_types_stmts(source, body, map, meta);
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_types_stmts(source, inner, map, meta);
                }
            }
            _ => {}
        }
    }
}

fn collect_types_expr(
    expr: &php_ast::Expr<'_, '_>,
    map: &mut HashMap<String, String>,
    meta: Option<&PhpStormMeta>,
) {
    if let ExprKind::Assign(assign) = &expr.kind {
        if let ExprKind::Variable(var_name) = &assign.target.kind {
            if let ExprKind::New(new_expr) = &assign.value.kind {
                if let Some(class_name) = extract_class_name(new_expr.class) {
                    map.insert(format!("${}", var_name), class_name);
                }
            }
            // PHPStorm meta: `$var = $obj->make(SomeClass::class)`
            if let Some(meta) = meta {
                if let Some(inferred) =
                    infer_from_meta_method_call(&assign.value, map, meta)
                {
                    map.insert(format!("${}", var_name), inferred);
                }
            }
        }
        collect_types_expr(assign.value, map, meta);
    }
}

fn extract_class_name(expr: &php_ast::Expr<'_, '_>) -> Option<String> {
    match &expr.kind {
        ExprKind::Identifier(name) => Some(name.to_string()),
        _ => None,
    }
}

/// Try to infer the return type of `$obj->method(SomeClass::class)` using the
/// PHPStorm meta map.  `map` is consulted to resolve `$obj`'s class.
fn infer_from_meta_method_call(
    expr: &php_ast::Expr<'_, '_>,
    var_map: &HashMap<String, String>,
    meta: &PhpStormMeta,
) -> Option<String> {
    let ExprKind::MethodCall(m) = &expr.kind else {
        return None;
    };
    // Resolve the receiver's type.
    let receiver_class = match &m.object.kind {
        ExprKind::Variable(v) => {
            let key = format!("${}", v);
            var_map.get(&key)?.clone()
        }
        _ => return None,
    };
    // Get the method name.
    let method_name = match &m.method.kind {
        ExprKind::Identifier(n) => n.as_ref().to_string(),
        _ => return None,
    };
    // Get the first argument as a class name string.
    let arg = m.args.first()?;
    let arg_str = match &arg.value.kind {
        ExprKind::String(s) => s.as_ref().trim_start_matches('\\').to_string(),
        ExprKind::ClassConstAccess(c) if c.member.as_ref() == "class" => {
            match &c.class.kind {
                ExprKind::Identifier(n) => {
                    n.as_ref().trim_start_matches('\\').rsplit('\\').next()
                        .unwrap_or(n.as_ref())
                        .to_string()
                }
                _ => return None,
            }
        }
        _ => return None,
    };
    meta.resolve_return_type(&receiver_class, &method_name, &arg_str)
        .map(|s| s.to_string())
}

/// Return the direct parent class name of `class_name` in `doc`, if any.
pub fn parent_class_name(doc: &ParsedDoc, class_name: &str) -> Option<String> {
    parent_in_stmts(&doc.program().stmts, class_name)
}

fn parent_in_stmts(stmts: &[Stmt<'_, '_>], class_name: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                return c.extends.as_ref().map(|n| n.to_string_repr().to_string());
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let found @ Some(_) = parent_in_stmts(inner, class_name) {
                        return found;
                    }
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
    /// (name, is_static)
    pub methods: Vec<(String, bool)>,
    /// (name, is_static)
    pub properties: Vec<(String, bool)>,
    pub constants: Vec<String>,
    /// Direct parent class name, if any.
    pub parent: Option<String>,
    /// Trait names used by this class (`use Foo, Bar;`).
    pub trait_uses: Vec<String>,
}

/// Return all members (methods, properties, constants) of `class_name`.
/// Also returns the direct parent class name via `ClassMembers::parent`.
pub fn members_of_class(doc: &ParsedDoc, class_name: &str) -> ClassMembers {
    let mut out = ClassMembers::default();
    out.parent = collect_members_stmts(&doc.program().stmts, class_name, &mut out);
    out
}

fn collect_members_stmts(
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    out: &mut ClassMembers,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                for member in c.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            out.methods.push((m.name.to_string(), m.is_static));
                            // Constructor-promoted params become instance properties.
                            if m.name == "__construct" {
                                for p in m.params.iter() {
                                    if p.visibility.is_some() {
                                        out.properties.push((p.name.to_string(), false));
                                    }
                                }
                            }
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
                return c.extends.as_ref().map(|n| n.to_string_repr().to_string());
            }
            StmtKind::Enum(e) if e.name == class_name => {
                let is_backed = e.scalar_type.is_some();
                // Every enum instance exposes `->name`; backed enums also expose `->value`.
                out.properties.push(("name".to_string(), false));
                if is_backed {
                    out.properties.push(("value".to_string(), false));
                }
                // Built-in static methods present on every enum.
                out.methods.push(("cases".to_string(), true));
                if is_backed {
                    out.methods.push(("from".to_string(), true));
                    out.methods.push(("tryFrom".to_string(), true));
                }
                // User-declared cases, methods, and constants.
                for member in e.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Case(c) => {
                            out.constants.push(c.name.to_string());
                        }
                        EnumMemberKind::Method(m) => {
                            out.methods.push((m.name.to_string(), m.is_static));
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
                for member in t.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            out.methods.push((m.name.to_string(), m.is_static));
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
                    let result = collect_members_stmts(inner, class_name, out);
                    if result.is_some()
                        || out.methods.len() + out.properties.len() + out.constants.len() > 0
                    {
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
            StmtKind::Class(c) if c.name == Some(class_name) => {
                if let Some(raw) = docblock_before(source, stmt.span.start) {
                    return parse_docblock(&raw).mixins;
                }
                return vec![];
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    let found = mixin_classes_in_stmts(source, inner, class_name);
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
pub fn enclosing_class_at(source: &str, doc: &ParsedDoc, position: Position) -> Option<String> {
    enclosing_class_in_stmts(source, &doc.program().stmts, position)
}

fn enclosing_class_in_stmts(source: &str, stmts: &[Stmt<'_, '_>], pos: Position) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let start = offset_to_position(source, stmt.span.start).line;
                let end = offset_to_position(source, stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return c.name.map(|n| n.to_string());
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(found) = enclosing_class_in_stmts(source, inner, pos) {
                        return Some(found);
                    }
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

/// Returns `true` if `class_name` is declared as an `enum` in `doc`.
pub fn is_enum(doc: &ParsedDoc, class_name: &str) -> bool {
    is_enum_in_stmts(&doc.program().stmts, class_name)
}

fn is_enum_in_stmts(stmts: &[Stmt<'_, '_>], name: &str) -> bool {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Enum(e) if e.name == name => return true,
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if is_enum_in_stmts(inner, name) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// Returns `true` if `class_name` is a *backed* enum (`enum Foo: string` /
/// `enum Foo: int`) in `doc`.  Backed enums have a `->value` property.
pub fn is_backed_enum(doc: &ParsedDoc, class_name: &str) -> bool {
    is_backed_enum_in_stmts(&doc.program().stmts, class_name)
}

fn is_backed_enum_in_stmts(stmts: &[Stmt<'_, '_>], name: &str) -> bool {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Enum(e) if e.name == name => return e.scalar_type.is_some(),
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if is_backed_enum_in_stmts(inner, name) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
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
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == func_name {
                            for p in m.params.iter() {
                                out.push(p.name.to_string());
                            }
                            return;
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_params_stmts(inner, func_name, out);
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
    fn infers_type_from_new_expression() {
        let src = "<?php\n$obj = new Foo();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$obj"), Some("Foo"));
    }

    #[test]
    fn unknown_variable_returns_none() {
        let src = "<?php\n$obj = new Foo();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert!(tm.get("$other").is_none());
    }

    #[test]
    fn multiple_assignments() {
        let src = "<?php\n$a = new Foo();\n$b = new Bar();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$a"), Some("Foo"));
        assert_eq!(tm.get("$b"), Some("Bar"));
    }

    #[test]
    fn later_assignment_overwrites() {
        let src = "<?php\n$a = new Foo();\n$a = new Bar();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$a"), Some("Bar"));
    }

    #[test]
    fn infers_type_from_typed_param() {
        let src = "<?php\nfunction process(Mailer $mailer): void { $mailer-> }";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$mailer"), Some("Mailer"));
    }

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
        let names: Vec<&str> = members.methods.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"add"), "missing 'add'");
        assert!(names.contains(&"sub"), "missing 'sub'");
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
        assert!(prop_names.contains(&"x"), "promoted param x should be a property");
        assert!(prop_names.contains(&"y"), "promoted param y should be a property");
    }

    #[test]
    fn enum_instance_members_include_name() {
        let src = "<?php\nenum Status { case Active; case Inactive; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Status");
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(prop_names.contains(&"name"), "pure enum should expose ->name");
        assert!(!prop_names.contains(&"value"), "pure enum should not expose ->value");
    }

    #[test]
    fn backed_enum_exposes_value_and_factory_methods() {
        let src = "<?php\nenum Color: string { case Red = 'red'; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Color");
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        let method_names: Vec<&str> = members.methods.iter().map(|(n, _)| n.as_str()).collect();
        assert!(prop_names.contains(&"value"), "backed enum should expose ->value");
        assert!(method_names.contains(&"from"), "backed enum should have ::from()");
        assert!(method_names.contains(&"tryFrom"), "backed enum should have ::tryFrom()");
        assert!(method_names.contains(&"cases"), "enum should have ::cases()");
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
        let method_names: Vec<&str> = members.methods.iter().map(|(n, _)| n.as_str()).collect();
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(method_names.contains(&"log"), "trait method log should be collected");
        assert!(prop_names.contains(&"logFile"), "trait property logFile should be collected");
    }

    #[test]
    fn class_with_trait_use_lists_trait() {
        let src = "<?php\ntrait Logging { public function log() {} }\nclass App { use Logging; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "App");
        assert!(members.trait_uses.contains(&"Logging".to_string()), "should list used trait");
    }

    #[test]
    fn var_docblock_with_explicit_varname_infers_type() {
        let src = "<?php\n/** @var Mailer $mailer */\n$mailer = $container->get('mailer');";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$mailer"), Some("Mailer"), "@var with explicit name should map the variable");
    }

    #[test]
    fn var_docblock_without_varname_infers_from_assignment() {
        let src = "<?php\n/** @var Repository */\n$repo = $this->getRepository();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$repo"), Some("Repository"), "@var without name should use assignment LHS");
    }

    #[test]
    fn var_docblock_does_not_map_primitive_types() {
        let src = "<?php\n/** @var string */\n$name = 'hello';";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        // Primitives (lowercase) should not be mapped as class names.
        assert!(tm.get("$name").is_none(), "primitive @var should not produce a class mapping");
    }

    #[test]
    fn is_enum_pure() {
        let src = "<?php\nenum Suit { case Hearts; case Clubs; }";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(is_enum(&doc, "Suit"));
        assert!(!is_backed_enum(&doc, "Suit"));
    }

    #[test]
    fn is_backed_enum_string() {
        let src = "<?php\nenum Status: string { case Active = 'active'; }";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(is_enum(&doc, "Status"));
        assert!(is_backed_enum(&doc, "Status"));
    }

    #[test]
    fn is_enum_false_for_class() {
        let src = "<?php\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(!is_enum(&doc, "Foo"));
        assert!(!is_backed_enum(&doc, "Foo"));
    }
}
