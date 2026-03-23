/// Deep AST walker — collects all spans where `word` appears as a name reference
/// (function calls, `new Foo`, method calls, bare identifiers, static calls).
use php_parser_rs::lexer::token::Span;
use php_parser_rs::parser::ast::{
    arguments::Argument,
    classes::ClassMember,
    control_flow::{IfStatementBody, IfStatementElseIf, IfStatementElseIfBlock},
    identifiers::Identifier as AstIdentifier,
    loops::{ForeachStatementBody, ForeachStatementIterator, WhileStatementBody},
    namespaces::NamespaceStatement,
    operators::{
        ArithmeticOperationExpression, BitwiseOperationExpression, ComparisonOperationExpression,
        LogicalOperationExpression,
    },
    traits::TraitMember,
    Expression, Statement,
};

pub fn refs_in_stmts(stmts: &[Statement], word: &str, out: &mut Vec<Span>) {
    for stmt in stmts {
        refs_in_stmt(stmt, word, out);
    }
}

/// Like `refs_in_stmts`, but also matches spans inside `use` statements.
/// Needed so that renaming a class also renames its `use` import.
pub fn refs_in_stmts_with_use(stmts: &[Statement], word: &str, out: &mut Vec<Span>) {
    // First collect all normal refs
    refs_in_stmts(stmts, word, out);
    // Then scan `use` statements for the last segment matching `word`
    use_refs(stmts, word, out);
}

fn use_refs(stmts: &[Statement], word: &str, out: &mut Vec<Span>) {
    use php_parser_rs::lexer::token::Span;
    for stmt in stmts {
        match stmt {
            Statement::Use(u) => {
                for use_item in &u.uses {
                    let fqn = use_item.name.value.to_string();
                    let alias_match = use_item
                        .alias
                        .as_ref()
                        .map(|a| a.value.to_string() == word)
                        .unwrap_or(false);
                    let last_seg = fqn.rsplit('\\').next().unwrap_or(&fqn);
                    if alias_match || last_seg == word {
                        // Create a synthetic span pointing only to the last segment
                        // so rename edits target just the class name part.
                        let offset = fqn.len() - last_seg.len();
                        let syn_span = Span {
                            line: use_item.name.span.line,
                            column: use_item.name.span.column + offset,
                            position: use_item.name.span.position + offset,
                        };
                        out.push(syn_span);
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                use_refs(inner, word, out);
            }
            _ => {}
        }
    }
}

pub fn refs_in_stmt(stmt: &Statement, word: &str, out: &mut Vec<Span>) {
    match stmt {
        Statement::Expression(e) => refs_in_expr(&e.expression, word, out),
        Statement::Return(r) => {
            if let Some(v) = &r.value {
                refs_in_expr(v, word, out);
            }
        }
        Statement::Echo(e) => {
            for expr in &e.values {
                refs_in_expr(expr, word, out);
            }
        }
        Statement::Function(f) => {
            if f.name.value.to_string() == word {
                out.push(f.name.span);
            }
            refs_in_stmts(&f.body.statements, word, out);
        }
        Statement::Class(c) => {
            if c.name.value.to_string() == word {
                out.push(c.name.span);
            }
            for member in &c.body.members {
                match member {
                    ClassMember::ConcreteMethod(m) => {
                        if m.name.value.to_string() == word {
                            out.push(m.name.span);
                        }
                        refs_in_stmts(&m.body.statements, word, out);
                    }
                    _ => {}
                }
            }
        }
        Statement::Interface(i) => {
            if i.name.value.to_string() == word {
                out.push(i.name.span);
            }
        }
        Statement::Trait(t) => {
            if t.name.value.to_string() == word {
                out.push(t.name.span);
            }
            for member in &t.body.members {
                match member {
                    TraitMember::ConcreteMethod(m) => {
                        if m.name.value.to_string() == word {
                            out.push(m.name.span);
                        }
                        refs_in_stmts(&m.body.statements, word, out);
                    }
                    _ => {}
                }
            }
        }
        Statement::Namespace(ns) => match ns {
            NamespaceStatement::Unbraced(u) => refs_in_stmts(&u.statements, word, out),
            NamespaceStatement::Braced(b) => refs_in_stmts(&b.body.statements, word, out),
        },
        Statement::If(i) => {
            refs_in_expr(&i.condition, word, out);
            match &i.body {
                IfStatementBody::Statement { statement, elseifs, r#else } => {
                    refs_in_stmt(statement, word, out);
                    for ei in elseifs {
                        refs_in_elseif(ei, word, out);
                    }
                    if let Some(e) = r#else {
                        refs_in_stmt(&e.statement, word, out);
                    }
                }
                IfStatementBody::Block { statements, elseifs, r#else, .. } => {
                    refs_in_stmts(statements, word, out);
                    for ei in elseifs {
                        refs_in_elseif_block(ei, word, out);
                    }
                    if let Some(e) = r#else {
                        refs_in_stmts(&e.statements, word, out);
                    }
                }
            }
        }
        Statement::While(w) => {
            refs_in_expr(&w.condition, word, out);
            match &w.body {
                WhileStatementBody::Statement { statement } => refs_in_stmt(statement, word, out),
                WhileStatementBody::Block { statements, .. } => refs_in_stmts(statements, word, out),
            }
        }
        Statement::DoWhile(d) => {
            refs_in_stmt(&d.body, word, out);
            refs_in_expr(&d.condition, word, out);
        }
        Statement::Foreach(f) => {
            match &f.iterator {
                ForeachStatementIterator::Value { expression, .. } => refs_in_expr(expression, word, out),
                ForeachStatementIterator::KeyAndValue { expression, .. } => refs_in_expr(expression, word, out),
            }
            match &f.body {
                ForeachStatementBody::Statement { statement } => refs_in_stmt(statement, word, out),
                ForeachStatementBody::Block { statements, .. } => refs_in_stmts(statements, word, out),
            }
        }
        Statement::For(f) => {
            for cond in &f.iterator.conditions.inner {
                refs_in_expr(cond, word, out);
            }
            match &f.body {
                php_parser_rs::parser::ast::loops::ForStatementBody::Statement { statement } => {
                    refs_in_stmt(statement, word, out);
                }
                php_parser_rs::parser::ast::loops::ForStatementBody::Block { statements, .. } => {
                    refs_in_stmts(statements, word, out);
                }
            }
        }
        Statement::Try(t) => {
            refs_in_stmts(&t.body, word, out);
            for catch in &t.catches {
                refs_in_stmts(&catch.body, word, out);
            }
            if let Some(finally) = &t.finally {
                refs_in_stmts(&finally.body, word, out);
            }
        }
        Statement::Block(b) => refs_in_stmts(&b.statements, word, out),
        Statement::Static(s) => {
            for var in &s.vars {
                if let Some(v) = &var.default {
                    refs_in_expr(v, word, out);
                }
            }
        }
        _ => {}
    }
}

fn refs_in_elseif(ei: &IfStatementElseIf, word: &str, out: &mut Vec<Span>) {
    refs_in_expr(&ei.condition, word, out);
    refs_in_stmt(&ei.statement, word, out);
}

fn refs_in_elseif_block(ei: &IfStatementElseIfBlock, word: &str, out: &mut Vec<Span>) {
    refs_in_expr(&ei.condition, word, out);
    refs_in_stmts(&ei.statements, word, out);
}

fn args(arg_list: &php_parser_rs::parser::ast::arguments::ArgumentList, word: &str, out: &mut Vec<Span>) {
    for a in &arg_list.arguments {
        match a {
            Argument::Positional(p) => refs_in_expr(&p.value, word, out),
            Argument::Named(n) => refs_in_expr(&n.value, word, out),
        }
    }
}

fn ident_matches(id: &AstIdentifier, word: &str) -> Option<Span> {
    match id {
        AstIdentifier::SimpleIdentifier(si) if si.value.to_string() == word => Some(si.span),
        _ => None,
    }
}

pub fn refs_in_expr(expr: &Expression, word: &str, out: &mut Vec<Span>) {
    match expr {
        Expression::Identifier(id) => {
            if let Some(span) = ident_matches(id, word) {
                out.push(span);
            }
        }
        Expression::FunctionCall(f) => {
            refs_in_expr(&f.target, word, out);
            args(&f.arguments, word, out);
        }
        Expression::FunctionClosureCreation(f) => {
            refs_in_expr(&f.target, word, out);
        }
        Expression::MethodCall(m) => {
            refs_in_expr(&m.target, word, out);
            refs_in_expr(&m.method, word, out);
            args(&m.arguments, word, out);
        }
        Expression::NullsafeMethodCall(m) => {
            refs_in_expr(&m.target, word, out);
            refs_in_expr(&m.method, word, out);
            args(&m.arguments, word, out);
        }
        Expression::StaticMethodCall(s) => {
            refs_in_expr(&s.target, word, out);
            if let Some(span) = ident_matches(&s.method, word) {
                out.push(span);
            }
            args(&s.arguments, word, out);
        }
        Expression::New(n) => {
            refs_in_expr(&n.target, word, out);
            if let Some(arg_list) = &n.arguments {
                args(arg_list, word, out);
            }
        }
        Expression::AssignmentOperation(a) => {
            refs_in_expr(a.left(), word, out);
            refs_in_expr(a.right(), word, out);
        }
        Expression::ArithmeticOperation(a) => match a {
            ArithmeticOperationExpression::Addition { left, right, .. }
            | ArithmeticOperationExpression::Subtraction { left, right, .. }
            | ArithmeticOperationExpression::Multiplication { left, right, .. }
            | ArithmeticOperationExpression::Division { left, right, .. }
            | ArithmeticOperationExpression::Modulo { left, right, .. }
            | ArithmeticOperationExpression::Exponentiation { left, right, .. } => {
                refs_in_expr(left, word, out);
                refs_in_expr(right, word, out);
            }
            ArithmeticOperationExpression::Negative { right, .. }
            | ArithmeticOperationExpression::Positive { right, .. }
            | ArithmeticOperationExpression::PreIncrement { right, .. }
            | ArithmeticOperationExpression::PreDecrement { right, .. } => {
                refs_in_expr(right, word, out);
            }
            ArithmeticOperationExpression::PostIncrement { left, .. }
            | ArithmeticOperationExpression::PostDecrement { left, .. } => {
                refs_in_expr(left, word, out);
            }
        },
        Expression::ComparisonOperation(c) => match c {
            ComparisonOperationExpression::Equal { left, right, .. }
            | ComparisonOperationExpression::Identical { left, right, .. }
            | ComparisonOperationExpression::NotEqual { left, right, .. }
            | ComparisonOperationExpression::AngledNotEqual { left, right, .. }
            | ComparisonOperationExpression::NotIdentical { left, right, .. }
            | ComparisonOperationExpression::LessThan { left, right, .. }
            | ComparisonOperationExpression::GreaterThan { left, right, .. }
            | ComparisonOperationExpression::LessThanOrEqual { left, right, .. }
            | ComparisonOperationExpression::GreaterThanOrEqual { left, right, .. }
            | ComparisonOperationExpression::Spaceship { left, right, .. } => {
                refs_in_expr(left, word, out);
                refs_in_expr(right, word, out);
            }
        },
        Expression::LogicalOperation(l) => match l {
            LogicalOperationExpression::And { left, right, .. }
            | LogicalOperationExpression::Or { left, right, .. }
            | LogicalOperationExpression::LogicalAnd { left, right, .. }
            | LogicalOperationExpression::LogicalOr { left, right, .. }
            | LogicalOperationExpression::LogicalXor { left, right, .. } => {
                refs_in_expr(left, word, out);
                refs_in_expr(right, word, out);
            }
            LogicalOperationExpression::Not { right, .. } => refs_in_expr(right, word, out),
        },
        Expression::BitwiseOperation(b) => match b {
            BitwiseOperationExpression::And { left, right, .. }
            | BitwiseOperationExpression::Or { left, right, .. }
            | BitwiseOperationExpression::Xor { left, right, .. }
            | BitwiseOperationExpression::LeftShift { left, right, .. }
            | BitwiseOperationExpression::RightShift { left, right, .. } => {
                refs_in_expr(left, word, out);
                refs_in_expr(right, word, out);
            }
            BitwiseOperationExpression::Not { right, .. } => refs_in_expr(right, word, out),
        },
        Expression::Concat(c) => {
            refs_in_expr(&c.left, word, out);
            refs_in_expr(&c.right, word, out);
        }
        Expression::Instanceof(i) => {
            refs_in_expr(&i.left, word, out);
            refs_in_expr(&i.right, word, out);
        }
        Expression::Ternary(t) => {
            refs_in_expr(&t.condition, word, out);
            refs_in_expr(&t.then, word, out);
            refs_in_expr(&t.r#else, word, out);
        }
        Expression::ShortTernary(t) => {
            refs_in_expr(&t.condition, word, out);
            refs_in_expr(&t.r#else, word, out);
        }
        Expression::Coalesce(c) => {
            refs_in_expr(&c.lhs, word, out);
            refs_in_expr(&c.rhs, word, out);
        }
        Expression::Parenthesized(p) => refs_in_expr(&p.expr, word, out),
        Expression::ErrorSuppress(e) => refs_in_expr(&e.expr, word, out),
        Expression::Reference(r) => refs_in_expr(&r.right, word, out),
        Expression::Clone(c) => refs_in_expr(&c.target, word, out),
        Expression::Throw(t) => refs_in_expr(&t.value, word, out),
        Expression::Yield(y) => {
            if let Some(v) = &y.value {
                refs_in_expr(v, word, out);
            }
            if let Some(k) = &y.key {
                refs_in_expr(k, word, out);
            }
        }
        Expression::ArrayIndex(a) => {
            refs_in_expr(&a.array, word, out);
            if let Some(idx) = &a.index {
                refs_in_expr(idx, word, out);
            }
        }
        Expression::PropertyFetch(p) => refs_in_expr(&p.target, word, out),
        Expression::NullsafePropertyFetch(p) => refs_in_expr(&p.target, word, out),
        Expression::StaticPropertyFetch(p) => refs_in_expr(&p.target, word, out),
        Expression::ConstantFetch(c) => {
            refs_in_expr(&c.target, word, out);
            if let Some(span) = ident_matches(&c.constant, word) {
                out.push(span);
            }
        }
        Expression::Print(p) => {
            if let Some(v) = &p.value {
                refs_in_expr(v, word, out);
            }
        }
        Expression::Closure(c) => refs_in_stmts(&c.body.statements, word, out),
        Expression::ArrowFunction(a) => refs_in_expr(&a.body, word, out),
        Expression::Match(m) => {
            refs_in_expr(&*m.condition, word, out);
            for arm in &m.arms {
                for cond in &arm.conditions {
                    refs_in_expr(cond, word, out);
                }
                refs_in_expr(&arm.body, word, out);
            }
        }
        Expression::ShortArray(a) => {
            for item in a.items.iter() {
                match item {
                    php_parser_rs::parser::ast::ArrayItem::Value { value, .. } => refs_in_expr(value, word, out),
                    php_parser_rs::parser::ast::ArrayItem::ReferencedValue { value, .. } => refs_in_expr(value, word, out),
                    php_parser_rs::parser::ast::ArrayItem::SpreadValue { value, .. } => refs_in_expr(value, word, out),
                    php_parser_rs::parser::ast::ArrayItem::KeyValue { key, value, .. } => {
                        refs_in_expr(key, word, out);
                        refs_in_expr(value, word, out);
                    }
                    php_parser_rs::parser::ast::ArrayItem::ReferencedKeyValue { key, value, .. } => {
                        refs_in_expr(key, word, out);
                        refs_in_expr(value, word, out);
                    }
                    php_parser_rs::parser::ast::ArrayItem::Skipped => {}
                }
            }
        }
        _ => {}
    }
}
