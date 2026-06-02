/// `textDocument/typeDefinition` — jump to the class declaration of the type
/// of the symbol under the cursor.
///
/// Works for variables resolved by mir (flow-sensitive, generics/unions)
/// and for function parameters with a declared type hint.
use std::collections::HashMap;
use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, Expr, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::ast::{ParsedDoc, SourceView, format_type_hint, str_offset_in_range};
use crate::moniker::resolve_fqn;
use crate::references::collect_class_imports;
use crate::util::{fqn_short_name, word_at_position, word_range_at, zero_width_range};
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
            let from_mir = analysis.and_then(|a| {
                let offset = word_range_at(source, position)
                    .map(|r| doc.view().byte_of_position(r.start))
                    .unwrap_or_else(|| doc.view().byte_of_position(position));
                let names =
                    crate::type_query::class_names(crate::type_query::type_at_offset(a, offset)?);
                // Join named classes with `|`; the downstream `type_candidates`
                // splits unions back apart for the declaration search.
                (!names.is_empty()).then(|| names.join("|"))
            });
            match from_mir {
                Some(joined) => joined,
                // Fallback: parameter *declarations* record no mir symbol (mir
                // records variable *uses* in bodies, not binding sites). Resolve
                // the declared hint straight from the AST.
                None => param_decl_type(source, doc, &imports, &word, position)?,
            }
        } else {
            match param_type_for(&doc.program().stmts, &word) {
                Some(raw) => resolve_fqn(doc, &raw, &imports),
                None => return None,
            }
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
        let names =
            crate::type_query::class_names(crate::type_query::type_at_offset(analysis, offset)?);
        if names.is_empty() {
            return None;
        }
        names.join("|")
    };

    Some((imports, class_name))
}

/// Given the cursor position, resolve the type of the symbol and return all
/// matching locations for that type's class/interface declarations.
/// Returns empty vec if no type found, single-element vec for simple types,
/// multiple elements for union types (e.g., Admin|User).
pub fn goto_type_definition(
    source: &str,
    doc: &ParsedDoc,
    analysis: Option<&FileAnalysis>,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Vec<Location> {
    let Some((imports, class_name)) = resolve_type_at_cursor(source, doc, analysis, position)
    else {
        return Vec::new();
    };

    let mut results = Vec::new();

    // Look only in files whose namespace + short class name matches the FQN.
    for candidate in type_candidates(&class_name) {
        let cand_short = fqn_short_name(candidate.trim_start_matches('\\'));
        let cand_fqn = candidate.trim_start_matches('\\');

        for (uri, other_doc) in all_docs {
            // Skip files whose namespace can't contain this FQN.
            if !cand_fqn.is_empty() && cand_fqn.contains('\\') {
                let ns_prefix = &cand_fqn[..cand_fqn.rfind('\\').unwrap_or(0)];
                let file_ns = file_namespace(other_doc);
                if file_ns.as_deref() != Some(ns_prefix) {
                    continue;
                }
            }
            let other_sv = other_doc.view();
            if let Some(range) = find_class_range(other_sv, &other_doc.program().stmts, cand_short)
            {
                results.push(Location {
                    uri: uri.clone(),
                    range,
                });
            }
        }
    }

    // If results found in FQN pass, return them
    if !results.is_empty() {
        dedup_locations(&mut results);
        return results;
    }

    // Fallback: short-name search across all docs.
    // Skip fallback if the class_name came from an import: imports take precedence,
    // so if not found in their declared namespace, don't fall back to short-name search.
    let is_from_import = imports.values().any(|v| v == &class_name);
    if !is_from_import {
        for candidate in type_candidates(&class_name) {
            let cand_short = fqn_short_name(candidate.trim_start_matches('\\'));
            for (uri, other_doc) in all_docs {
                let other_sv = other_doc.view();
                if let Some(range) =
                    find_class_range(other_sv, &other_doc.program().stmts, cand_short)
                {
                    results.push(Location {
                        uri: uri.clone(),
                        range,
                    });
                }
            }
        }

        dedup_locations(&mut results);
    }

    results
}

fn dedup_locations(results: &mut Vec<Location>) {
    results.sort_by(|a, b| {
        a.uri
            .as_str()
            .cmp(b.uri.as_str())
            .then_with(|| a.range.start.line.cmp(&b.range.start.line))
    });
    results.dedup_by(|a, b| a.uri == b.uri && a.range.start.line == b.range.start.line);
}

/// Return the namespace declared in a doc's top-level statements, if any.
fn file_namespace(doc: &ParsedDoc) -> Option<String> {
    for stmt in doc.program().stmts.iter() {
        if let StmtKind::Namespace(ns) = &stmt.kind {
            return ns.name.as_ref().map(|n| n.to_string_repr().to_string());
        }
    }
    None
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

/// Resolve the declared type of a parameter named `word` at `position`, to a
/// `|`-joined string of FQNs that the downstream `type_candidates` search can
/// consume. mir records variable *uses* in bodies but not binding sites, so
/// this AST fallback handles parameter declarations.
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
            "self" | "static" => crate::type_map::enclosing_class_at(source, doc, position)
                .map(|c| resolve_fqn(doc, &c, imports))
                .unwrap_or_else(|| cand.to_string()),
            "parent" => crate::type_map::enclosing_class_at(source, doc, position)
                .and_then(|c| crate::type_map::parent_class_name(doc, &c))
                .map(|p| resolve_fqn(doc, &p, imports))
                .unwrap_or_else(|| cand.to_string()),
            other => resolve_fqn(doc, other, imports),
        })
        .collect();
    (!resolved.is_empty()).then(|| resolved.join("|"))
}

/// Find the innermost method-call expression whose span contains `cursor` and
/// return the byte offset of its method-name identifier — a position mir
/// recorded a `ResolvedSymbol` for (the call's resolved receiver/return type).
///
/// Used for the chain-gap case: when the cursor sits between calls
/// (`$q->where()$0->next()`) there is no word under it, but the enclosing call's
/// identifier carries the type. Descends receiver-first so the deepest call
/// boundary that still contains the cursor wins, matching the previous
/// `chain_type_at_cursor` semantics.
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

/// Look up the declared type hint for a parameter named `word` in any function/method.
/// Note: Returns the type hint as-is from format_type_hint. Unqualified type names
/// in non-global namespaces are not automatically qualified with namespace context.
/// This is a known limitation: resolving `Logger` in `namespace App\Service` to
/// `App\Service\Logger` would require source context to extract namespace names.
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

/// Find the range of the class or interface declaration named `name`.
fn find_class_range(sv: SourceView<'_>, stmts: &[Stmt<'_, '_>], name: &str) -> Option<Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name.map(|n| n.or_error()) == Some(name) => {
                // Use statement span to find the name within the declaration context,
                // not the first occurrence in the file (which might be a different use).
                let stmt_range = sv.range_of(stmt.span);
                let name_in_source = c.name.expect("match guard ensures Some").or_error();
                if let Some(pos) = str_offset_in_range(sv.source(), stmt.span, name_in_source) {
                    return Some(Range {
                        start: sv.position_of(pos),
                        end: sv.position_of(pos + name_in_source.len() as u32),
                    });
                }
                return Some(stmt_range);
            }
            StmtKind::Interface(i) if i.name == name => {
                // Use statement span to find the name within the declaration context.
                let name_str = i.name.or_error();
                if let Some(pos) = str_offset_in_range(sv.source(), stmt.span, name_str) {
                    return Some(Range {
                        start: sv.position_of(pos),
                        end: sv.position_of(pos + name_str.len() as u32),
                    });
                }
                return Some(sv.range_of(stmt.span));
            }
            StmtKind::Trait(t) if t.name == name => {
                // Use statement span to find the name within the declaration context.
                let name_str = t.name.or_error();
                if let Some(pos) = str_offset_in_range(sv.source(), stmt.span, name_str) {
                    return Some(Range {
                        start: sv.position_of(pos),
                        end: sv.position_of(pos + name_str.len() as u32),
                    });
                }
                return Some(sv.range_of(stmt.span));
            }
            StmtKind::Enum(e) if e.name == name => {
                // Use statement span to find the name within the declaration context.
                let name_str = e.name.or_error();
                if let Some(pos) = str_offset_in_range(sv.source(), stmt.span, name_str) {
                    return Some(Range {
                        start: sv.position_of(pos),
                        end: sv.position_of(pos + name_str.len() as u32),
                    });
                }
                return Some(sv.range_of(stmt.span));
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = find_class_range(sv, &inner.stmts, name)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find type definition locations using `FileIndex` entries.
/// Returns all matching locations (multiple for union types).
pub fn goto_type_definition_from_index(
    source: &str,
    doc: &ParsedDoc,
    analysis: Option<&FileAnalysis>,
    indexes: &[(Url, std::sync::Arc<crate::file_index::FileIndex>)],
    position: Position,
) -> Vec<Location> {
    let Some((imports, class_name)) = resolve_type_at_cursor(source, doc, analysis, position)
    else {
        return Vec::new();
    };

    let mut results = Vec::new();

    // First pass: look for exact FQN match (high priority)
    for candidate in type_candidates(&class_name) {
        let cand_fqn = candidate.trim_start_matches('\\');
        for (uri, idx) in indexes {
            for cls in &idx.classes {
                let cls_fqn = cls.fqn.as_ref().trim_start_matches('\\');
                if cls_fqn == cand_fqn {
                    let range = zero_width_range(cls.start_line);
                    results.push(Location {
                        uri: uri.clone(),
                        range,
                    });
                }
            }
        }
    }

    // If found in first pass, deduplicate and return those
    if !results.is_empty() {
        dedup_locations(&mut results);
        return results;
    }

    // Second pass: look for short name match (lower priority, may be ambiguous)
    // Skip fallback if the class_name came from an import: imports take precedence,
    // so if not found in their declared namespace, don't fall back to short-name search.
    let is_from_import = imports.values().any(|v| v == &class_name);
    if !is_from_import {
        for candidate in type_candidates(&class_name) {
            let cn_short = fqn_short_name(candidate);
            for (uri, idx) in indexes {
                for cls in &idx.classes {
                    let short = fqn_short_name(cls.name.as_ref());
                    if short == cn_short {
                        let range = zero_width_range(cls.start_line);
                        results.push(Location {
                            uri: uri.clone(),
                            range,
                        });
                    }
                }
            }
        }

        dedup_locations(&mut results);
    }

    results
}
