/// Single-pass type inference: collects `$var = new ClassName()` assignments
/// to map variable names to class names.  Used to scope method completions
/// after `->`.
use std::collections::HashMap;

use php_parser_rs::parser::ast::{
    classes::ClassMember,
    identifiers::Identifier as AstIdentifier,
    namespaces::NamespaceStatement,
    variables::Variable,
    Expression, Statement,
};

/// Maps variable name (with `$`) → class name.
#[derive(Debug, Default, Clone)]
pub struct TypeMap(HashMap<String, String>);

impl TypeMap {
    pub fn empty() -> Self {
        TypeMap(HashMap::new())
    }

    /// Build from all statements in a file.
    pub fn from_stmts(stmts: &[Statement]) -> Self {
        let mut map = HashMap::new();
        collect_types_stmts(stmts, &mut map);
        TypeMap(map)
    }

    /// Returns the class name for a variable, e.g. `get("$obj")` → `Some("Foo")`.
    pub fn get<'a>(&'a self, var: &str) -> Option<&'a str> {
        self.0.get(var).map(|s| s.as_str())
    }
}

fn collect_types_stmts(stmts: &[Statement], map: &mut HashMap<String, String>) {
    for stmt in stmts {
        match stmt {
            Statement::Expression(e) => collect_types_expr(&e.expression, map),
            Statement::Function(f) => collect_types_stmts(&f.body.statements, map),
            Statement::Class(c) => {
                for member in &c.body.members {
                    if let ClassMember::ConcreteMethod(m) = member {
                        collect_types_stmts(&m.body.statements, map);
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                collect_types_stmts(inner, map);
            }
            _ => {}
        }
    }
}

fn collect_types_expr(expr: &Expression, map: &mut HashMap<String, String>) {
    match expr {
        Expression::AssignmentOperation(assign) => {
            // Check for `$var = new ClassName()`
            if let Expression::Variable(Variable::SimpleVariable(v)) = assign.left() {
                if let Expression::New(new_expr) = assign.right() {
                    if let Some(class_name) = extract_class_name(&new_expr.target) {
                        map.insert(v.name.to_string(), class_name);
                    }
                }
            }
            // Also recurse the right side for nested expressions
            collect_types_expr(assign.right(), map);
        }
        _ => {}
    }
}

fn extract_class_name(expr: &Expression) -> Option<String> {
    match expr {
        Expression::Identifier(AstIdentifier::SimpleIdentifier(si)) => {
            Some(si.value.to_string())
        }
        _ => None,
    }
}

/// Return the names of all methods defined on `class_name` in `stmts`.
pub fn methods_of_class<'a>(stmts: &'a [Statement], class_name: &str) -> Vec<String> {
    let mut methods = Vec::new();
    collect_methods_stmts(stmts, class_name, &mut methods);
    methods
}

fn collect_methods_stmts(stmts: &[Statement], class_name: &str, out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Statement::Class(c) if c.name.value.to_string() == class_name => {
                for member in &c.body.members {
                    match member {
                        ClassMember::ConcreteMethod(m) => {
                            out.push(m.name.value.to_string());
                        }
                        ClassMember::AbstractMethod(m) => {
                            out.push(m.name.value.to_string());
                        }
                        _ => {}
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                collect_methods_stmts(inner, class_name, out);
            }
            _ => {}
        }
    }
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
    fn infers_type_from_new_expression() {
        let src = "<?php\n$obj = new Foo();";
        let ast = parse_ast(src);
        let tm = TypeMap::from_stmts(&ast);
        assert_eq!(tm.get("$obj"), Some("Foo"));
    }

    #[test]
    fn unknown_variable_returns_none() {
        let src = "<?php\n$obj = new Foo();";
        let ast = parse_ast(src);
        let tm = TypeMap::from_stmts(&ast);
        assert!(tm.get("$other").is_none());
    }

    #[test]
    fn multiple_assignments() {
        let src = "<?php\n$a = new Foo();\n$b = new Bar();";
        let ast = parse_ast(src);
        let tm = TypeMap::from_stmts(&ast);
        assert_eq!(tm.get("$a"), Some("Foo"));
        assert_eq!(tm.get("$b"), Some("Bar"));
    }

    #[test]
    fn later_assignment_overwrites() {
        let src = "<?php\n$a = new Foo();\n$a = new Bar();";
        let ast = parse_ast(src);
        let tm = TypeMap::from_stmts(&ast);
        assert_eq!(tm.get("$a"), Some("Bar"));
    }

    #[test]
    fn methods_of_class_finds_methods() {
        let src = "<?php\nclass Calc { public function add() {} public function sub() {} }";
        let ast = parse_ast(src);
        let methods = methods_of_class(&ast, "Calc");
        assert!(methods.contains(&"add".to_string()));
        assert!(methods.contains(&"sub".to_string()));
    }

    #[test]
    fn methods_of_unknown_class_is_empty() {
        let src = "<?php\nclass Calc { public function add() {} }";
        let ast = parse_ast(src);
        let methods = methods_of_class(&ast, "Unknown");
        assert!(methods.is_empty());
    }
}
