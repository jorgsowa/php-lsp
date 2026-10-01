//! Cursor-on-declaration detection.
//!
//! AST pre-passes that run before the character-based `symbol_kind_at`
//! heuristic, so that *declarations* (method/property/constant names, promoted
//! constructor params) are classified precisely rather than by surrounding
//! punctuation.

use tower_lsp_server::ls_types::Position;

use php_ast::{
    ClassMember, ClassMemberKind, EnumMember, EnumMemberKind, ExprKind, NamespaceBody, Stmt,
    StmtKind,
};

use crate::document::ast::str_offset;

use super::position::position_to_byte_offset_strict;

/// Locate `name` within `member_span` rather than searching the whole source —
/// the global `str_offset` returns the first occurrence in the file, which
/// causes a method named `status` to also match a property named `$status`
/// (cursor on the `$status` declaration falsely tests positive for "on method
/// decl").
fn name_offset_in_member(source: &str, member_span: php_ast::Span, name: &str) -> Option<u32> {
    let s = member_span.start as usize;
    let e = (member_span.end as usize).min(source.len());
    source
        .get(s..e)?
        .find(name)
        .map(|off| member_span.start + off as u32)
}

/// If the cursor is on a class or trait property *declaration* name (e.g.
/// `public string $status`), return the property name without the leading `$`
/// so the caller can search for `status` via `SymbolKind::Property`.  Returns
/// `None` when the cursor is elsewhere.
pub(crate) fn cursor_is_on_property_decl(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    position: Position,
) -> Option<String> {
    let cursor = position_to_byte_offset_strict(source, position)?;
    fn check(source: &str, stmts: &[Stmt<'_, '_>], cursor: u32) -> Option<String> {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Class(c) => {
                    for member in c.body.members.iter() {
                        if let ClassMemberKind::Property(p) = &member.kind {
                            let name = p.name.to_string();
                            let start =
                                name_offset_in_member(source, member.span, &name).unwrap_or(0);
                            let end = start + name.len() as u32;
                            if cursor >= start && cursor < end {
                                return Some(name);
                            }
                        }
                    }
                }
                StmtKind::Trait(t) => {
                    for member in t.body.members.iter() {
                        if let ClassMemberKind::Property(p) = &member.kind {
                            let name = p.name.to_string();
                            let start =
                                name_offset_in_member(source, member.span, &name).unwrap_or(0);
                            let end = start + name.len() as u32;
                            if cursor >= start && cursor < end {
                                return Some(name);
                            }
                        }
                    }
                }
                StmtKind::Namespace(ns) => {
                    if let NamespaceBody::Braced(inner) = &ns.body
                        && let Some(name) = check(source, &inner.stmts, cursor)
                    {
                        return Some(name);
                    }
                }
                _ => {}
            }
        }
        None
    }

    check(source, stmts, cursor)
}

/// When the cursor sits on a class / interface / trait / enum constant
/// declaration (`const NAME = ...`), return `(const_name, owning_class_short_name)`.
/// `owning_class_short_name` is the short name of the declaring type; it is used
/// as a class filter when searching for references so that same-named constants
/// in different classes don't cross-match.
pub(crate) fn cursor_is_on_constant_decl(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    position: Position,
) -> Option<(String, Option<String>)> {
    let cursor = position_to_byte_offset_strict(source, position)?;

    fn check_members(source: &str, members: &[ClassMember<'_, '_>], cursor: u32) -> Option<String> {
        for member in members {
            if let ClassMemberKind::ClassConst(c) = &member.kind {
                let name = c.name.to_string();
                let start = name_offset_in_member(source, member.span, &name).unwrap_or(0);
                let end = start + name.len() as u32;
                if cursor >= start && cursor < end {
                    return Some(name);
                }
            }
        }
        None
    }

    fn check_enum_members(
        source: &str,
        members: &[EnumMember<'_, '_>],
        cursor: u32,
    ) -> Option<String> {
        for member in members {
            // Cases and class consts are both accessed the same way
            // (`Status::Active`), so both must resolve the same way as a
            // find-references starting point: as a Constant owned by this
            // enum, not — for cases — via the uppercase-name heuristic in
            // `symbol_kind_at`, which misroutes it to the Class walker and
            // finds no usages at all (case names are conventionally
            // PascalCase, same as class names).
            let name = match &member.kind {
                EnumMemberKind::ClassConst(c) => c.name.to_string(),
                EnumMemberKind::Case(c) => c.name.to_string(),
                _ => continue,
            };
            let start = name_offset_in_member(source, member.span, &name).unwrap_or(0);
            let end = start + name.len() as u32;
            if cursor >= start && cursor < end {
                return Some(name);
            }
        }
        None
    }

    fn check(
        source: &str,
        stmts: &[Stmt<'_, '_>],
        cursor: u32,
    ) -> Option<(String, Option<String>)> {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Class(c) => {
                    if let Some(const_name) = check_members(source, &c.body.members, cursor) {
                        let owner = c.name.map(|n| n.to_string());
                        return Some((const_name, owner));
                    }
                }
                StmtKind::Interface(i) => {
                    if let Some(const_name) = check_members(source, &i.body.members, cursor) {
                        return Some((const_name, Some(i.name.to_string())));
                    }
                }
                StmtKind::Trait(t) => {
                    if let Some(const_name) = check_members(source, &t.body.members, cursor) {
                        return Some((const_name, Some(t.name.to_string())));
                    }
                }
                StmtKind::Enum(e) => {
                    if let Some(const_name) = check_enum_members(source, &e.body.members, cursor) {
                        return Some((const_name, Some(e.name.to_string())));
                    }
                }
                StmtKind::Const(items) => {
                    for item in items.iter() {
                        let name = item.name.to_string();
                        let s = item.span.start as usize;
                        let e = (item.span.end as usize).min(source.len());
                        if let Some(off) = source.get(s..e).and_then(|sl| sl.find(&name)) {
                            let start = item.span.start + off as u32;
                            let end = start + name.len() as u32;
                            if cursor >= start && cursor < end {
                                return Some((name, None));
                            }
                        }
                    }
                }
                StmtKind::Expression(expr) => {
                    // Detect cursor inside `define('NAME', value)` string literal.
                    if let ExprKind::FunctionCall(f) = &expr.kind
                        && let ExprKind::Identifier(id) = &f.name.kind
                        && id.as_str() == "define"
                        && let Some(first_arg) = f.args.first()
                        && let Some(first_arg_value) = &first_arg.value
                        && let ExprKind::String(s) = &first_arg_value.kind
                    {
                        // String content starts one byte after the opening quote.
                        let start = first_arg_value.span.start + 1;
                        let end = start + s.len() as u32;
                        if cursor >= start && cursor < end {
                            return Some((s.to_string(), None));
                        }
                    }
                }
                StmtKind::Namespace(ns) => {
                    if let NamespaceBody::Braced(inner) = &ns.body
                        && let Some(result) = check(source, &inner.stmts, cursor)
                    {
                        return Some(result);
                    }
                }
                _ => {}
            }
        }
        None
    }

    check(source, stmts, cursor)
}

/// If the cursor sits on a promoted constructor property parameter (one that
/// has a visibility modifier like `public`/`protected`/`private`), return the
/// property name without the leading `$` so the caller can search for
/// `->name` property accesses (`SymbolKind::Property`).
///
/// Returns `None` for regular (non-promoted) params and for any cursor position
/// not on a constructor param name.
pub(crate) fn promoted_property_at_cursor(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    position: Position,
) -> Option<String> {
    let cursor = position_to_byte_offset_strict(source, position)?;

    fn check(source: &str, stmts: &[Stmt<'_, '_>], cursor: u32) -> Option<String> {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Class(c) => {
                    for member in c.body.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == "__construct"
                        {
                            for param in m.params.iter() {
                                if param.visibility.is_none() {
                                    continue;
                                }
                                let param_name = param.name.or_error();
                                let name_start = str_offset(source, param_name).unwrap_or(0);
                                let name_end = name_start + param_name.len() as u32;
                                if cursor >= name_start && cursor < name_end {
                                    return Some(param_name.trim_start_matches('$').to_string());
                                }
                            }
                        }
                    }
                }
                StmtKind::Namespace(ns) => {
                    if let NamespaceBody::Braced(inner) = &ns.body
                        && let Some(name) = check(source, &inner.stmts, cursor)
                    {
                        return Some(name);
                    }
                }
                _ => {}
            }
        }
        None
    }

    check(source, stmts, cursor)
}
