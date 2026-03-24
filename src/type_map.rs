/// Single-pass type inference: collects `$var = new ClassName()` assignments
/// to map variable names to class names.  Used to scope method completions
/// after `->`.
use std::collections::HashMap;

use php_ast::{ClassMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::Position;

use crate::ast::{offset_to_position, ParsedDoc};

/// Maps variable name (with `$`) → class name.
#[derive(Debug, Default, Clone)]
pub struct TypeMap(HashMap<String, String>);

impl TypeMap {
    pub fn empty() -> Self {
        TypeMap(HashMap::new())
    }

    /// Build from a parsed document.
    pub fn from_doc(doc: &ParsedDoc) -> Self {
        let mut map = HashMap::new();
        collect_types_stmts(&doc.program().stmts, &mut map);
        TypeMap(map)
    }

    /// Returns the class name for a variable, e.g. `get("$obj")` → `Some("Foo")`.
    pub fn get<'a>(&'a self, var: &str) -> Option<&'a str> {
        self.0.get(var).map(|s| s.as_str())
    }
}

fn collect_types_stmts(stmts: &[Stmt<'_, '_>], map: &mut HashMap<String, String>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Expression(e) => collect_types_expr(e, map),
            StmtKind::Function(f) => collect_types_stmts(&f.body, map),
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if let Some(body) = &m.body {
                            collect_types_stmts(body, map);
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_types_stmts(inner, map);
                }
            }
            _ => {}
        }
    }
}

fn collect_types_expr(expr: &php_ast::Expr<'_, '_>, map: &mut HashMap<String, String>) {
    if let ExprKind::Assign(assign) = &expr.kind {
        if let ExprKind::Variable(var_name) = &assign.target.kind {
            if let ExprKind::New(new_expr) = &assign.value.kind {
                if let Some(class_name) = extract_class_name(new_expr.class) {
                    map.insert(format!("${}", var_name), class_name);
                }
            }
        }
        collect_types_expr(assign.value, map);
    }
}

fn extract_class_name(expr: &php_ast::Expr<'_, '_>) -> Option<String> {
    match &expr.kind {
        ExprKind::Identifier(name) => Some(name.to_string()),
        _ => None,
    }
}

/// Return the names of all methods defined on `class_name` in the document.
pub fn methods_of_class(doc: &ParsedDoc, class_name: &str) -> Vec<String> {
    let mut methods = Vec::new();
    collect_methods_stmts(&doc.program().stmts, class_name, &mut methods);
    methods
}

fn collect_methods_stmts(stmts: &[Stmt<'_, '_>], class_name: &str, out: &mut Vec<String>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        out.push(m.name.to_string());
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_methods_stmts(inner, class_name, out);
                }
            }
            _ => {}
        }
    }
}

/// All members of a named class split by kind and static-ness.
#[derive(Debug, Default)]
pub struct ClassMembers {
    /// (name, is_static)
    pub methods: Vec<(String, bool)>,
    /// (name, is_static)
    pub properties: Vec<(String, bool)>,
    pub constants: Vec<String>,
}

/// Return all members (methods, properties, constants) of `class_name`.
pub fn members_of_class(doc: &ParsedDoc, class_name: &str) -> ClassMembers {
    let mut out = ClassMembers::default();
    collect_members_stmts(&doc.program().stmts, class_name, &mut out);
    out
}

fn collect_members_stmts(stmts: &[Stmt<'_, '_>], class_name: &str, out: &mut ClassMembers) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                for member in c.members.iter() {
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
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_members_stmts(inner, class_name, out);
                }
            }
            _ => {}
        }
    }
}

/// Return the name of the class whose body contains `position`, or `None`.
pub fn enclosing_class_at(source: &str, doc: &ParsedDoc, position: Position) -> Option<String> {
    enclosing_class_in_stmts(source, &doc.program().stmts, position)
}

fn enclosing_class_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    pos: Position,
) -> Option<String> {
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
    fn methods_of_class_finds_methods() {
        let src = "<?php\nclass Calc { public function add() {} public function sub() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let methods = methods_of_class(&doc, "Calc");
        assert!(methods.contains(&"add".to_string()));
        assert!(methods.contains(&"sub".to_string()));
    }

    #[test]
    fn methods_of_unknown_class_is_empty() {
        let src = "<?php\nclass Calc { public function add() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let methods = methods_of_class(&doc, "Unknown");
        assert!(methods.is_empty());
    }
}
