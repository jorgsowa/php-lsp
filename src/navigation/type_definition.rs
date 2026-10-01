/// `textDocument/typeDefinition` — jump to the class declaration of the type
/// of the symbol under the cursor.
///
/// Works for variables resolved by mir (flow-sensitive, generics/unions)
/// and for function parameters with a declared type hint.
use std::collections::HashMap;

use php_ast::{ClassMemberKind, EnumMemberKind, Expr, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp_server::ls_types::Position;

use crate::document::ast::{ParsedDoc, format_type_hint};
use crate::navigation::moniker::resolve_fqn;
use crate::navigation::references::collect_class_imports;
use crate::text::{word_at_position, word_range_at};
use mir_analyzer::FileAnalysis;

/// Resolve the PHP type at `position` to a fully-qualified class name.
/// Returns `(imports, fqn)` on success, or `None` if no type could be inferred.
fn resolve_type_at_cursor(
    source: &str,
    doc: &ParsedDoc,
    analysis: Option<&FileAnalysis>,
    position: Position,
) -> Option<(HashMap<String, String>, String)> {
    let imports = collect_class_imports(doc);

    let class_name = if let Some(word) = word_at_position(source, position) {
        if word.starts_with('$') {
            // Primary: resolve the variable's type from mir's recorded symbols
            // (flow-sensitive, carries generics/unions). mir produces a name
            // already qualified through the file's namespace + `use` imports,
            // so no `resolve_fqn` is needed. The query offset is the `$` (word
            // range start), which lands strictly inside mir's end-exclusive
            // variable span.
            // For parameters with late-binding type hints (`parent`, `self`, `static`),
            // resolve via the AST directly — mir's TParent/TSelf/TStaticObject carry
            // the containing class's FQCN, not the actual parent, so class_names()
            // would navigate to the wrong class without this bypass.
            let bare_word = word.trim_start_matches('$');
            let hint = param_type_for(&doc.program().stmts, bare_word)
                .or_else(|| param_type_for(&doc.program().stmts, &word));
            let is_late_binding = hint.as_deref().is_some_and(|h| {
                h.split(['|', '&']).any(|p| {
                    matches!(
                        p.trim().trim_start_matches('?'),
                        "parent" | "self" | "static"
                    )
                })
            });
            if is_late_binding {
                param_decl_type(source, doc, &imports, &word, position)?
            } else {
                let from_mir = analysis.and_then(|a| {
                    let offset = word_range_at(source, position)
                        .map(|r| doc.view().byte_of_position(r.start))
                        .unwrap_or_else(|| doc.view().byte_of_position(position));
                    let names = crate::types::type_query::class_names(
                        crate::types::type_query::type_at_offset(a, offset)?,
                    );
                    // Join named classes with `|`; the downstream `type_candidates`
                    // splits unions back apart for the declaration search.
                    (!names.is_empty()).then(|| names.join("|"))
                });
                match from_mir {
                    Some(joined) => joined,
                    None => param_decl_type(source, doc, &imports, &word, position)?,
                }
            }
        } else {
            let raw = param_type_for(&doc.program().stmts, &word)?;
            resolve_fqn(doc, &raw, &imports)
        }
    } else {
        // Cursor is not on a word — it sits in a method-call chain gap such as
        // `$q->where()$0->next()`. mir records a symbol at each call's method
        // identifier; find the innermost call whose span contains the cursor and
        // read mir's resolved type there. The AST walk is the glue that stays;
        // the type resolution is mir's.
        let analysis = analysis?;
        let cursor_byte = doc.view().byte_of_position(position);
        let offset = innermost_call_method_offset(&doc.program().stmts, cursor_byte)?;
        let names = crate::types::type_query::class_names(
            crate::types::type_query::type_at_offset(analysis, offset)?,
        );
        if names.is_empty() {
            return None;
        }
        names.join("|")
    };

    Some((imports, class_name))
}

/// Class FQNs of the type at `position`: one per member of a union/intersection.
pub fn type_class_fqns(
    source: &str,
    doc: &ParsedDoc,
    analysis: Option<&FileAnalysis>,
    position: Position,
) -> Vec<String> {
    let Some((_, class_name)) = resolve_type_at_cursor(source, doc, analysis, position) else {
        return Vec::new();
    };
    let mut fqns: Vec<String> = type_candidates(&class_name)
        .into_iter()
        .map(|c| c.trim_start_matches('\\').to_string())
        .collect();
    fqns.dedup();
    fqns
}

/// Decompose a formatted type hint into searchable class-name candidates.
/// `"?Foo"` → `["Foo"]`, `"Foo|Bar"` → `["Foo", "Bar"]`, `"Foo&Bar"` → `["Foo", "Bar"]`.
fn type_candidates(type_hint: &str) -> Vec<&str> {
    let hint = type_hint.strip_prefix('?').unwrap_or(type_hint);
    hint.split(['|', '&'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Find the innermost method-call expression whose span contains `cursor` and
/// return the byte offset of its method-name identifier — a position mir
/// recorded a `ResolvedSymbol` for (the call's resolved receiver/return type).
///
/// Used for the chain-gap case: when the cursor sits between calls
/// (`$q->where()$0->next()`) there is no word under it, but the enclosing call's
/// identifier carries the type. Descends receiver-first so the deepest call
/// boundary that still contains the cursor wins.
fn innermost_call_method_offset(stmts: &[Stmt<'_, '_>], cursor: u32) -> Option<u32> {
    for stmt in stmts {
        if !span_contains_cursor(stmt.span, cursor) {
            continue;
        }
        let found = match &stmt.kind {
            StmtKind::Expression(e) => call_method_offset_in_expr(e, cursor),
            StmtKind::Return(Some(e)) => call_method_offset_in_expr(e, cursor),
            StmtKind::Echo(exprs) => exprs
                .iter()
                .find_map(|e| call_method_offset_in_expr(e, cursor)),
            StmtKind::Function(f) => innermost_call_method_offset(&f.body.stmts, cursor),
            StmtKind::Class(c) => c.body.members.iter().find_map(|m| {
                if let ClassMemberKind::Method(method) = &m.kind
                    && let Some(body) = &method.body
                {
                    innermost_call_method_offset(&body.stmts, cursor)
                } else {
                    None
                }
            }),
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    innermost_call_method_offset(&inner.stmts, cursor)
                } else {
                    None
                }
            }
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn call_method_offset_in_expr(expr: &Expr<'_, '_>, cursor: u32) -> Option<u32> {
    if !span_contains_cursor(expr.span, cursor) {
        return None;
    }
    match &expr.kind {
        ExprKind::MethodCall(mc) | ExprKind::NullsafeMethodCall(mc) => {
            // Prefer a deeper call in the receiver; otherwise this call carries
            // the type at the cursor.
            call_method_offset_in_expr(mc.object, cursor).or(Some(mc.method.span.start))
        }
        ExprKind::Assign(a) => call_method_offset_in_expr(a.value, cursor),
        _ => None,
    }
}

#[inline]
fn span_contains_cursor(span: php_ast::Span, cursor: u32) -> bool {
    // Inclusive end so a cursor in the gap after a closing paren still matches
    // the parent call (e.g. `$q->where()$0->next()`).
    cursor >= span.start && cursor <= span.end
}

/// Resolve the declared type of a parameter named `word` at `position`, to a
/// `|`-joined string of FQNs that the downstream `type_candidates` search can
/// consume. Used when the mir symbol path is skipped (e.g. late-binding type
/// hints: `parent`/`self`/`static`) or when mir returns no symbol.
///
/// Reads the type hint, strips nullable `?`, splits unions/intersections,
/// resolves `self`/`static`/`parent` against the enclosing class, and
/// qualifies the rest through the file's namespace + `use` imports.
fn param_decl_type(
    source: &str,
    doc: &ParsedDoc,
    imports: &HashMap<String, String>,
    word: &str,
    position: Position,
) -> Option<String> {
    // Param names in the AST are stored without the leading `$`; accept both.
    let raw = param_type_for(&doc.program().stmts, word)
        .or_else(|| param_type_for(&doc.program().stmts, word.trim_start_matches('$')))?;
    let bare = raw.trim_start_matches('?');
    let resolved: Vec<String> = bare
        .split(['|', '&'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|cand| match cand {
            "self" | "static" => crate::types::type_map::enclosing_class_at(source, doc, position)
                .map(|c| resolve_fqn(doc, &c, imports))
                .unwrap_or_else(|| cand.to_string()),
            "parent" => crate::types::type_map::enclosing_class_at(source, doc, position)
                .and_then(|c| crate::types::type_map::parent_class_name(doc, &c))
                .map(|p| resolve_fqn(doc, &p, imports))
                .unwrap_or_else(|| cand.to_string()),
            other => resolve_fqn(doc, other, imports),
        })
        .collect();
    (!resolved.is_empty()).then(|| resolved.join("|"))
}

/// Look up the declared type hint for a parameter named `word` in any function/method.
/// Returns the raw string from `format_type_hint`; callers are responsible for
/// resolving unqualified names against namespace/import context via `resolve_fqn`.
fn param_type_for(stmts: &[Stmt<'_, '_>], word: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                for p in f.params.iter() {
                    if p.name == word
                        && let Some(type_hint) = &p.type_hint
                    {
                        return Some(format_type_hint(type_hint));
                    }
                }
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        for p in m.params.iter() {
                            if p.name == word
                                && let Some(type_hint) = &p.type_hint
                            {
                                return Some(format_type_hint(type_hint));
                            }
                        }
                    }
                }
            }
            StmtKind::Interface(i) => {
                for member in i.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        for p in m.params.iter() {
                            if p.name == word
                                && let Some(type_hint) = &p.type_hint
                            {
                                return Some(format_type_hint(type_hint));
                            }
                        }
                    }
                }
            }
            StmtKind::Trait(trait_) => {
                for member in trait_.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        for p in m.params.iter() {
                            if p.name == word
                                && let Some(type_hint) = &p.type_hint
                            {
                                return Some(format_type_hint(type_hint));
                            }
                        }
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind {
                        for p in m.params.iter() {
                            if p.name == word
                                && let Some(type_hint) = &p.type_hint
                            {
                                return Some(format_type_hint(type_hint));
                            }
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(type_hint) = param_type_for(&inner.stmts, word)
                {
                    return Some(type_hint);
                }
            }
            _ => {}
        }
    }
    None
}
