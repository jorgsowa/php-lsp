//! Reference sites mir's index doesn't record: docblock `@see`/`@uses` links,
//! function names held in string literals, and class constants read off a
//! receiver whose type can't be resolved.
use std::ops::ControlFlow;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{ClassMemberKind, Expr, ExprKind, NamespaceBody, Span, Stmt, StmtKind, TypeHintKind};

use super::moniker::resolve_fqn;
use super::references::collect_file_imports;
use crate::document::ast::ParsedDoc;

/// Functions whose arguments name a callable or a function by string.
const CALLABLE_STRING_FUNCTIONS: &[&str] = &[
    "function_exists",
    "is_callable",
    "call_user_func",
    "call_user_func_array",
    "array_map",
    "array_filter",
    "array_walk",
    "array_walk_recursive",
    "array_reduce",
    "usort",
    "uasort",
    "uksort",
    "register_shutdown_function",
    "spl_autoload_register",
    "set_error_handler",
    "set_exception_handler",
    "iterator_apply",
    "forward_static_call",
    "forward_static_call_array",
];

/// What the extra scan looks for; class and function names are FQNs without a leading `\`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ExtraTarget<'a> {
    Function(&'a str),
    Method { class: &'a str, name: &'a str },
    Property { class: &'a str, name: &'a str },
    Constant { class: &'a str, name: &'a str },
}

impl ExtraTarget<'_> {
    /// The identifier every matching site contains, for a text prefilter.
    pub(crate) fn needle(&self) -> &str {
        match self {
            Self::Function(fqn) => fqn.rsplit('\\').next().unwrap_or(fqn),
            Self::Method { name, .. } | Self::Constant { name, .. } => name,
            Self::Property { name, .. } => name.trim_start_matches('$'),
        }
    }
}

/// Cheap text check: whether `text` could hold any extra site for `target`.
pub(crate) fn may_contain_extra_site(text: &str, target: ExtraTarget<'_>) -> bool {
    let has_doc_link = || text.contains("@see") || text.contains("@uses");
    match target {
        ExtraTarget::Function(_) => contains_ignore_ascii_case(text, target.needle()),
        ExtraTarget::Constant { name, .. } => {
            text.contains(name) && (text.contains(&format!("::{name}")) || has_doc_link())
        }
        ExtraTarget::Method { .. } | ExtraTarget::Property { .. } => {
            has_doc_link() && contains_ignore_ascii_case(text, target.needle())
        }
    }
}

/// Docblock and string-literal sites in `doc` that name `target`.
pub(crate) fn docblock_and_string_spans(doc: &ParsedDoc, target: ExtraTarget<'_>) -> Vec<Span> {
    let source = doc.source();
    let needle = target.needle();
    if needle.is_empty() || !contains_ignore_ascii_case(source, needle) {
        return Vec::new();
    }
    let mut out = docblock_spans(doc, target);
    if let ExtraTarget::Function(fqn) = target {
        let mut v = StringCallableVisitor {
            fqn,
            out: Vec::new(),
        };
        for stmt in doc.program().stmts.iter() {
            let _ = v.visit_stmt(stmt);
        }
        out.extend(v.out);
    }
    out
}

/// `$expr::NAME` accesses whose class side is an expression rather than a name.
pub(crate) fn dynamic_class_constant_spans(doc: &ParsedDoc, name: &str) -> Vec<Span> {
    if !doc.source().contains(name) {
        return Vec::new();
    }
    let mut v = DynamicConstVisitor {
        name,
        out: Vec::new(),
    };
    for stmt in doc.program().stmts.iter() {
        let _ = v.visit_stmt(stmt);
    }
    v.out
}

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

struct DynamicConstVisitor<'a> {
    name: &'a str,
    out: Vec<Span>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for DynamicConstVisitor<'_> {
    type Break = ();
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::ClassConstAccess(access) = &expr.kind
            && !matches!(
                access.class.kind,
                ExprKind::Name(_) | ExprKind::Identifier(_)
            )
            && let ExprKind::Identifier(member) = &access.member.kind
            && member.as_str() == self.name
        {
            self.out.push(access.member.span);
        }
        walk_expr(self, expr)
    }
}

struct StringCallableVisitor<'a> {
    fqn: &'a str,
    out: Vec<Span>,
}

impl StringCallableVisitor<'_> {
    fn matching_literal_span(&self, expr: &Expr<'_, '_>) -> Option<Span> {
        let ExprKind::String(value) = &expr.kind else {
            return None;
        };
        let value = value.to_str()?;
        if !value
            .trim_start_matches('\\')
            .eq_ignore_ascii_case(self.fqn)
        {
            return None;
        }
        // Drop the surrounding quotes.
        (expr.span.end >= expr.span.start + 2).then(|| Span {
            start: expr.span.start + 1,
            end: expr.span.end - 1,
        })
    }
}

impl<'arena, 'src> Visitor<'arena, 'src> for StringCallableVisitor<'_> {
    type Break = ();
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::FunctionCall(call) = &expr.kind
            && let ExprKind::Name(callee) = &call.name.kind
        {
            let callee = callee.to_string_repr();
            let callee = callee.trim_start_matches('\\');
            let callee = callee.rsplit('\\').next().unwrap_or(callee);
            if CALLABLE_STRING_FUNCTIONS
                .iter()
                .any(|f| f.eq_ignore_ascii_case(callee))
            {
                for arg in call.args.iter() {
                    if let Some(span) = arg.value().and_then(|v| self.matching_literal_span(v)) {
                        self.out.push(span);
                    }
                }
            }
        } else if self.fqn.contains('\\')
            && let Some(span) = self.matching_literal_span(expr)
        {
            // A namespaced FQN in a string is unambiguous wherever it appears.
            self.out.push(span);
            return ControlFlow::Continue(());
        }
        walk_expr(self, expr)
    }
}

/// `@see`/`@uses` targets (block or inline `{@see ...}`) inside `/** */` comments.
fn docblock_spans(doc: &ParsedDoc, target: ExtraTarget<'_>) -> Vec<Span> {
    let source = doc.source();
    let mut out = Vec::new();
    let mut imports = None;
    let mut search_from = 0;
    while let Some(open) = source[search_from..].find("/**").map(|i| i + search_from) {
        let Some(close) = source[open + 3..].find("*/").map(|i| i + open + 3) else {
            break;
        };
        search_from = close + 2;
        let comment = &source[open..close];
        for tag in ["@see", "@uses"] {
            let mut from = 0;
            while let Some(at) = comment[from..].find(tag).map(|i| i + from) {
                from = at + tag.len();
                let rest = &comment[from..];
                if !rest.starts_with([' ', '\t']) {
                    continue;
                }
                let lead = rest.len() - rest.trim_start_matches([' ', '\t']).len();
                let token_start = from + lead;
                let token_len = comment[token_start..]
                    .find(|c: char| !(c.is_alphanumeric() || "_\\:$()".contains(c)))
                    .unwrap_or(comment.len() - token_start);
                let token = &comment[token_start..token_start + token_len];
                let imports = imports.get_or_insert_with(|| collect_file_imports(doc));
                if let Some((offset, len)) =
                    match_doc_link(doc, imports, token, (open + token_start) as u32, target)
                {
                    let start = (open + token_start) as u32 + offset;
                    out.push(Span {
                        start,
                        end: start + len,
                    });
                }
            }
        }
    }
    out
}

/// Byte offset and length within `token` of the part naming `target`.
fn match_doc_link(
    doc: &ParsedDoc,
    imports: &std::collections::HashMap<String, String>,
    token: &str,
    token_offset: u32,
    target: ExtraTarget<'_>,
) -> Option<(u32, u32)> {
    let (subject, has_parens) = match token.strip_suffix("()") {
        Some(s) => (s, true),
        None => (token, false),
    };
    let Some((class, member)) = subject.split_once("::") else {
        let ExtraTarget::Function(fqn) = target else {
            return None;
        };
        if !has_parens || subject.is_empty() {
            return None;
        }
        let resolved = resolve_fqn(doc, subject, imports);
        let bare = subject.trim_start_matches('\\');
        let matches = resolved.eq_ignore_ascii_case(fqn)
            || (!bare.contains('\\') && bare.eq_ignore_ascii_case(fqn));
        return matches.then(|| {
            let short = bare.rsplit('\\').next().unwrap_or(bare);
            ((subject.len() - short.len()) as u32, short.len() as u32)
        });
    };
    let (target_class, member_matches) = match target {
        ExtraTarget::Function(_) => return None,
        ExtraTarget::Method { class, name } => (
            class,
            !member.starts_with('$') && member.eq_ignore_ascii_case(name),
        ),
        ExtraTarget::Property { class, name } => {
            let name = name.trim_start_matches('$');
            (class, member.strip_prefix('$') == Some(name))
        }
        ExtraTarget::Constant { class, name } => (class, !has_parens && member == name),
    };
    if !member_matches || class.is_empty() {
        return None;
    }
    let resolved = if ["self", "static", "$this"]
        .iter()
        .any(|k| k.eq_ignore_ascii_case(class))
    {
        enclosing_class_fqn(doc, token_offset)?
    } else {
        resolve_doc_class(doc, class, imports)
    };
    // Member ranges exclude a property's `$`, matching mir's reference ranges.
    let sigil = u32::from(member.starts_with('$'));
    resolved
        .trim_start_matches('\\')
        .eq_ignore_ascii_case(target_class)
        .then(|| {
            (
                (class.len() + 2) as u32 + sigil,
                member.len() as u32 - sigil,
            )
        })
}

/// Class names are case-insensitive, so an import matches whatever case the docblock uses.
fn resolve_doc_class(
    doc: &ParsedDoc,
    class: &str,
    imports: &std::collections::HashMap<String, String>,
) -> String {
    if !class.starts_with('\\') {
        let (first, rest) = match class.split_once('\\') {
            Some((first, rest)) => (first, Some(rest)),
            None => (class, None),
        };
        if let Some(fqn) = imports
            .iter()
            .find_map(|(alias, fqn)| alias.eq_ignore_ascii_case(first).then_some(fqn))
        {
            return match rest {
                Some(rest) => format!("{fqn}\\{rest}"),
                None => fqn.clone(),
            };
        }
    }
    resolve_fqn(doc, class, imports)
}

fn enclosing_class_fqn(doc: &ParsedDoc, offset: u32) -> Option<String> {
    fn find(stmts: &[Stmt<'_, '_>], offset: u32) -> Option<String> {
        stmts.iter().find_map(|stmt| {
            if offset < stmt.span.start || offset >= stmt.span.end {
                return None;
            }
            match &stmt.kind {
                StmtKind::Class(c) => c.name.map(|n| n.or_error().to_string()),
                StmtKind::Interface(i) => Some(i.name.or_error().to_string()),
                StmtKind::Trait(t) => Some(t.name.or_error().to_string()),
                StmtKind::Enum(e) => Some(e.name.or_error().to_string()),
                StmtKind::Namespace(ns) => match &ns.body {
                    NamespaceBody::Braced(inner) => find(&inner.stmts, offset),
                    _ => None,
                },
                _ => None,
            }
        })
    }
    // A class docblock sits just before the class statement's span.
    let stmts = &doc.program().stmts;
    let short = find(stmts, offset).or_else(|| {
        let next = stmts
            .iter()
            .flat_map(|s| match &s.kind {
                StmtKind::Namespace(ns) => match &ns.body {
                    NamespaceBody::Braced(inner) => inner.stmts.iter().collect::<Vec<_>>(),
                    _ => vec![],
                },
                _ => vec![s],
            })
            .find(|s| s.span.start > offset)?;
        find(std::slice::from_ref(next), next.span.start)
    })?;
    Some(resolve_fqn(doc, &short, &collect_file_imports(doc)))
}

/// Cheap text check: whether `text` could hold `$this->prop->name` on an untyped `prop`.
pub(crate) fn may_contain_untyped_property_access(text: &str, name: &str) -> bool {
    contains_ignore_ascii_case(text, &format!("->{name}")) && declares_untyped_property(text)
}

fn declares_untyped_property(text: &str) -> bool {
    ["private ", "protected ", "public ", "var "]
        .iter()
        .any(|kw| {
            text.match_indices(kw).any(|(at, _)| {
                let rest = text[at + kw.len()..].trim_start_matches([' ', '\t']);
                let Some(rest) = rest.strip_prefix('$') else {
                    return false;
                };
                let after_name = rest.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_');
                after_name.trim_start().starts_with([';', '=', ','])
            })
        })
}

/// For a member access at `member_offset` on `$this->prop`, where `prop` is an
/// untyped property, the one class every `$this->prop = ...` assignment in the
/// class stores: a class-typed parameter or a `new X`.
pub(crate) fn inferred_untyped_property_receiver(
    doc: &ParsedDoc,
    member_offset: u32,
) -> Option<String> {
    let mut finder = ThisPropReceiverFinder {
        member_offset,
        prop: None,
    };
    for stmt in doc.program().stmts.iter() {
        if finder.visit_stmt(stmt).is_break() {
            break;
        }
    }
    let prop = finder.prop?;
    let class = enclosing_class_decl(&doc.program().stmts, member_offset)?;

    let mut declared_untyped = false;
    let mut assigned: Vec<String> = Vec::new();
    for member in class.body.members.iter() {
        match &member.kind {
            ClassMemberKind::Property(p) if p.name.or_error().trim_start_matches('$') == prop => {
                if p.type_hint.is_some() {
                    return None;
                }
                declared_untyped = true;
            }
            ClassMemberKind::Method(m) => {
                let Some(body) = m.body else { continue };
                let mut v = ThisPropAssignVisitor {
                    prop: &prop,
                    params: &m.params,
                    out: &mut assigned,
                };
                for stmt in body.stmts.iter() {
                    let _ = v.visit_stmt(stmt);
                }
            }
            _ => {}
        }
    }
    if !declared_untyped {
        return None;
    }
    let first = assigned.first().filter(|c| !c.is_empty())?;
    if assigned.iter().any(|c| !c.eq_ignore_ascii_case(first)) {
        return None;
    }
    let fqn = resolve_doc_class(doc, first, &collect_file_imports(doc));
    Some(fqn.trim_start_matches('\\').to_string())
}

fn enclosing_class_decl<'a, 'arena, 'src>(
    stmts: &'a [Stmt<'arena, 'src>],
    offset: u32,
) -> Option<&'a php_ast::ClassDecl<'arena, 'src>> {
    stmts.iter().find_map(|stmt| {
        if offset < stmt.span.start || offset >= stmt.span.end {
            return None;
        }
        match &stmt.kind {
            StmtKind::Class(c) => Some(&**c),
            StmtKind::Namespace(ns) => match &ns.body {
                NamespaceBody::Braced(inner) => enclosing_class_decl(&inner.stmts, offset),
                _ => None,
            },
            _ => None,
        }
    })
}

struct ThisPropReceiverFinder {
    member_offset: u32,
    prop: Option<String>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for ThisPropReceiverFinder {
    type Break = ();
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if expr.span.end <= self.member_offset || expr.span.start > self.member_offset {
            return ControlFlow::Continue(());
        }
        let access = match &expr.kind {
            ExprKind::MethodCall(c) | ExprKind::NullsafeMethodCall(c) => Some((c.object, c.method)),
            ExprKind::PropertyAccess(a) | ExprKind::NullsafePropertyAccess(a) => {
                Some((a.object, a.property))
            }
            _ => None,
        };
        if let Some((object, member)) = access
            && member.span.start == self.member_offset
        {
            if let ExprKind::PropertyAccess(inner) = &object.kind
                && matches!(&inner.object.kind, ExprKind::Variable(v) if v.as_str().trim_start_matches('$') == "this")
                && let ExprKind::Identifier(prop) = &inner.property.kind
            {
                self.prop = Some(prop.as_str().to_string());
            }
            return ControlFlow::Break(());
        }
        walk_expr(self, expr)
    }
}

struct ThisPropAssignVisitor<'a, 'arena, 'src> {
    prop: &'a str,
    params: &'a [php_ast::Param<'arena, 'src>],
    out: &'a mut Vec<String>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for ThisPropAssignVisitor<'_, 'arena, 'src> {
    type Break = ();
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::Assign(assign) = &expr.kind
            && let ExprKind::PropertyAccess(target) = &assign.target.kind
            && matches!(&target.object.kind, ExprKind::Variable(v) if v.as_str().trim_start_matches('$') == "this")
            && matches!(&target.property.kind, ExprKind::Identifier(p) if p.as_str() == self.prop)
        {
            let class = match &assign.value.kind {
                ExprKind::New(new) => match &new.class.kind {
                    ExprKind::Name(name) => Some(name.to_string_repr().into_owned()),
                    _ => None,
                },
                ExprKind::Variable(var) => {
                    let var = var.as_str().trim_start_matches('$');
                    self.params
                        .iter()
                        .find(|p| p.name.or_error().trim_start_matches('$') == var)
                        .and_then(|p| match p.type_hint.as_ref().map(|t| &t.kind) {
                            Some(TypeHintKind::Named(name)) => {
                                Some(name.to_string_repr().into_owned())
                            }
                            Some(TypeHintKind::Nullable(inner)) => match &inner.kind {
                                TypeHintKind::Named(name) => {
                                    Some(name.to_string_repr().into_owned())
                                }
                                _ => None,
                            },
                            _ => None,
                        })
                }
                _ => None,
            };
            // An assignment the scan can't type makes the property's class unknown.
            self.out.push(class.unwrap_or_default());
        }
        walk_expr(self, expr)
    }
}
