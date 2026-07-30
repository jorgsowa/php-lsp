use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};

use crate::document::ast::{ParsedDoc, format_type_hint};
use crate::lang::docblock::{Docblock, docblock_before, parse_docblock};
use crate::text::fqn_short_name;

use super::formatting::{
    format_class_const, format_expr_literal, format_params, format_prop_prefix,
};

pub(crate) fn find_property_info(
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
            StmtKind::Class(c) if c.name.map(|n| n.or_error()) == Some(class_name) => {
                // A `readonly class` makes every property readonly even when
                // the property itself carries no `readonly` keyword of its own.
                let class_is_readonly = c.modifiers.is_readonly;
                for member in c.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Property(p) if p.name == prop_name => {
                            let modifiers = format_prop_prefix(
                                p.visibility.as_ref(),
                                p.is_static,
                                p.is_readonly || class_is_readonly,
                            );
                            let type_str = p
                                .type_hint
                                .as_ref()
                                .map(|t| crate::document::ast::format_type_hint(t))
                                .unwrap_or_default();
                            let db = docblock_before(source, member.span.start)
                                .map(|raw| parse_docblock(&raw));
                            return Some((modifiers, type_str, db));
                        }
                        ClassMemberKind::Method(m) if m.name == "__construct" => {
                            // Check promoted constructor parameters
                            for p in m.params.iter() {
                                if p.name == prop_name && p.visibility.is_some() {
                                    let modifiers = format_prop_prefix(
                                        p.visibility.as_ref(),
                                        false,
                                        p.is_readonly || class_is_readonly,
                                    );
                                    let type_str = p
                                        .type_hint
                                        .as_ref()
                                        .map(|t| crate::document::ast::format_type_hint(t))
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
                                                Some(crate::lang::docblock::Docblock {
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
                        find_property_info_in_stmts(source, &inner.stmts, class_name, prop_name)
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
pub(crate) fn scan_method_of_class(
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
            StmtKind::Class(c) if c.name.map(|n| n.or_error()) == Some(class_name) => {
                // 1. Direct method lookup.
                for member in c.body.members.iter() {
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
                for member in c.body.members.iter() {
                    if let ClassMemberKind::TraitUse(tu) = &member.kind {
                        for tn in tu.traits.iter() {
                            let s = tn.to_string_repr();
                            let short = fqn_short_name(&s).to_owned();
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
                    let short = fqn_short_name(&pn).to_owned();
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
                // 4. `@method` docblock tag — a virtual method with no concrete body.
                if let Some(sig) = scan_doc_method_of_class(c.doc_comment, class_name, method_name)
                {
                    return Some(sig);
                }
                return None;
            }
            StmtKind::Trait(t) if t.name == class_name => {
                for member in t.body.members.iter() {
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
                for member in e.body.members.iter() {
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
                    let result =
                        scan_method_of_class_impl(root, &inner.stmts, class_name, method_name);
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

/// Return `"ClassName::methodName(params): ReturnType"` for a method declared
/// only via a class-level `@method` docblock tag (no concrete AST member).
fn scan_doc_method_of_class(
    doc_comment: Option<php_ast::Comment<'_>>,
    class_name: &str,
    method_name: &str,
) -> Option<String> {
    let dm = parse_docblock(doc_comment?.text)
        .methods
        .into_iter()
        .find(|m| m.name == method_name)?;
    let params = dm
        .params
        .iter()
        .map(format_doc_method_param)
        .collect::<Vec<_>>()
        .join(", ");
    let ret = if dm.return_type.is_empty() {
        String::new()
    } else {
        format!(": {}", dm.return_type)
    };
    let prefix = if dm.is_static { "static " } else { "" };
    Some(format!(
        "{}{}::{}({}){}",
        prefix, class_name, method_name, params, ret
    ))
}

fn format_doc_method_param(p: &crate::lang::docblock::DocMethodParam) -> String {
    let mut s = String::new();
    if !p.type_hint.is_empty() {
        s.push_str(&p.type_hint);
        s.push(' ');
    }
    if p.is_byref {
        s.push('&');
    }
    if p.is_variadic {
        s.push_str("...");
    }
    s.push('$');
    s.push_str(&p.name);
    if p.is_optional {
        s.push_str(" = ...");
    }
    s
}

/// Return `"case ClassName::CaseName = value"` for `case_name` inside enum `class_name`.
pub(crate) fn scan_enum_case_of_class(
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    case_name: &str,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Enum(e) if e.name == class_name => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Case(c) = &member.kind
                        && c.name == case_name
                    {
                        let value_str = c
                            .value
                            .as_ref()
                            .and_then(format_expr_literal)
                            .map(|v| format!(" = {v}"))
                            .unwrap_or_default();
                        return Some(format!("case {}::{}{}", e.name, c.name, value_str));
                    }
                }
                return None;
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = scan_enum_case_of_class(&inner.stmts, class_name, case_name)
                {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

/// Return `"const CONST_NAME = value"` for `const_name` in class/interface/enum/trait `class_name`.
pub(crate) fn scan_class_const_of_class(
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    const_name: &str,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name.map(|n| n.or_error()) == Some(class_name) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::ClassConst(k) = &member.kind
                        && k.name == const_name
                    {
                        return Some(format_class_const(k));
                    }
                }
                return None;
            }
            StmtKind::Interface(i) if i.name == class_name => {
                for member in i.body.members.iter() {
                    if let ClassMemberKind::ClassConst(k) = &member.kind
                        && k.name == const_name
                    {
                        return Some(format_class_const(k));
                    }
                }
                return None;
            }
            StmtKind::Enum(e) if e.name == class_name => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::ClassConst(k) = &member.kind
                        && k.name == const_name
                    {
                        return Some(format_class_const(k));
                    }
                }
                return None;
            }
            StmtKind::Trait(t) if t.name == class_name => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::ClassConst(k) = &member.kind
                        && k.name == const_name
                    {
                        return Some(format_class_const(k));
                    }
                }
                return None;
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = scan_class_const_of_class(&inner.stmts, class_name, const_name)
                {
                    return Some(s);
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
                for member in t.body.members.iter() {
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
                    && let Some(s) = find_method_sig_in_trait(&inner.stmts, trait_name, method_name)
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
pub(crate) fn find_parent_class_name(stmts: &[Stmt<'_, '_>], class_name: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name.map(|n| n.or_error()) == Some(class_name) => {
                return c.extends.as_ref().map(|p| {
                    let pn = p.to_string_repr();
                    fqn_short_name(&pn).to_owned()
                });
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = find_parent_class_name(&inner.stmts, class_name)
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
) -> Option<crate::lang::docblock::Docblock> {
    find_method_docblock_in_stmts(doc.source(), &doc.program().stmts, class_name, method_name)
}

/// Like `find_method_docblock` but resolves `{@inheritDoc}` by walking the
/// parent chain across all supplied documents.
pub(crate) fn resolve_method_docblock<'a>(
    docs: impl Iterator<Item = &'a ParsedDoc> + Clone,
    class_name: &str,
    method_name: &str,
) -> Option<crate::lang::docblock::Docblock> {
    let docs: Vec<&'a ParsedDoc> = docs.collect();
    let mut current_class = class_name.to_owned();
    for _ in 0..16 {
        let db = docs
            .iter()
            .find_map(|d| find_method_docblock(d, &current_class, method_name));
        match db {
            Some(d) if d.is_inherit_doc => {
                // Find the parent class name across all documents.
                let parent = docs
                    .iter()
                    .find_map(|d| find_parent_class_name(&d.program().stmts, &current_class));
                match parent {
                    Some(p) => current_class = p,
                    None => return None,
                }
            }
            other => return other,
        }
    }
    None
}

fn find_method_docblock_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    method_name: &str,
) -> Option<crate::lang::docblock::Docblock> {
    find_method_docblock_impl(source, stmts, stmts, class_name, method_name)
}

fn find_method_docblock_impl<'a>(
    source: &str,
    root: &[Stmt<'a, 'a>],
    stmts: &[Stmt<'a, 'a>],
    class_name: &str,
    method_name: &str,
) -> Option<crate::lang::docblock::Docblock> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name.map(|n| n.or_error()) == Some(class_name) => {
                // Direct lookup.
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return docblock_before(source, member.span.start)
                            .map(|raw| parse_docblock(&raw));
                    }
                }
                // Walk trait uses.
                for member in c.body.members.iter() {
                    if let ClassMemberKind::TraitUse(tu) = &member.kind {
                        for tn in tu.traits.iter() {
                            let s = tn.to_string_repr();
                            let short = fqn_short_name(&s).to_owned();
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
                    let short = fqn_short_name(&pn).to_owned();
                    if let Some(db) =
                        find_method_docblock_impl(source, root, root, &short, method_name)
                    {
                        return Some(db);
                    }
                }
                return None;
            }
            StmtKind::Trait(t) if t.name == class_name => {
                for member in t.body.members.iter() {
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
                for member in e.body.members.iter() {
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
                    let result = find_method_docblock_impl(
                        source,
                        root,
                        &inner.stmts,
                        class_name,
                        method_name,
                    );
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
