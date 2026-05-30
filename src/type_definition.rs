/// `textDocument/typeDefinition` — jump to the class declaration of the type
/// of the symbol under the cursor.
///
/// Works for variables assigned via `$var = new ClassName()` (leverages `TypeMap`)
/// and for function parameters with a declared type hint.
use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::ast::{MethodReturnsMap, ParsedDoc, SourceView, format_type_hint, str_offset_in_range};
use crate::moniker::resolve_fqn;
use crate::references::collect_class_imports;
use crate::type_map::{TypeMap, build_method_returns};
use crate::util::{word_at_position, zero_width_range};

/// Given the cursor position, resolve the type of the symbol and return all
/// matching locations for that type's class/interface declarations.
/// Returns empty vec if no type found, single-element vec for simple types,
/// multiple elements for union types (e.g., Admin|User).
pub fn goto_type_definition(
    source: &str,
    doc: &ParsedDoc,
    doc_returns: Option<&MethodReturnsMap>,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Vec<Location> {
    let imports = collect_class_imports(doc);
    let type_map = TypeMap::from_doc_with_meta(doc, None, doc_returns);

    let class_name = if let Some(word) = word_at_position(source, position) {
        // Named symbol (variable or parameter)
        if word.starts_with('$') {
            // TypeMap stores the short class name; resolve it to FQN using the
            // current file's namespace + use imports so that `User` in
            // `namespace App\Service` resolves to `App\Service\User`.
            match type_map.get(&word) {
                Some(short) => resolve_fqn(doc, short, &imports),
                None => return Vec::new(),
            }
        } else {
            match param_type_for(&doc.program().stmts, &word) {
                Some(raw) => resolve_fqn(doc, &raw, &imports),
                None => return Vec::new(),
            }
        }
    } else {
        // Cursor is not on a word — try resolving the type from a method-call chain.
        let cursor_byte = doc.view().byte_of_position(position);
        let owned_returns;
        let chain_returns: &MethodReturnsMap = match doc_returns {
            Some(r) => r,
            None => {
                owned_returns = build_method_returns(doc);
                &owned_returns
            }
        };
        match type_map.chain_type_at_cursor(
            &doc.program().stmts,
            cursor_byte,
            std::slice::from_ref(&chain_returns),
        ) {
            Some(ty) => resolve_fqn(doc, &ty, &imports),
            None => return Vec::new(),
        }
    };

    let mut results = Vec::new();

    // Look only in files whose namespace + short class name matches the FQN.
    for candidate in type_candidates(&class_name) {
        let cand_short = candidate
            .trim_start_matches('\\')
            .rsplit('\\')
            .next()
            .unwrap_or(candidate);
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
            let cand_short = candidate
                .trim_start_matches('\\')
                .rsplit('\\')
                .next()
                .unwrap_or(candidate);
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
    doc_returns: Option<&MethodReturnsMap>,
    indexes: &[(Url, std::sync::Arc<crate::file_index::FileIndex>)],
    position: Position,
) -> Vec<Location> {
    let imports = collect_class_imports(doc);
    let type_map = TypeMap::from_doc_with_meta(doc, None, doc_returns);
    let class_name = if let Some(word) = word_at_position(source, position) {
        if word.starts_with('$') {
            match type_map.get(&word) {
                Some(short) => resolve_fqn(doc, short, &imports),
                None => return Vec::new(),
            }
        } else {
            match param_type_for(&doc.program().stmts, &word) {
                Some(raw) => resolve_fqn(doc, &raw, &imports),
                None => return Vec::new(),
            }
        }
    } else {
        let cursor_byte = doc.view().byte_of_position(position);
        let owned_returns;
        let chain_returns: &MethodReturnsMap = match doc_returns {
            Some(r) => r,
            None => {
                owned_returns = build_method_returns(doc);
                &owned_returns
            }
        };
        match type_map.chain_type_at_cursor(
            &doc.program().stmts,
            cursor_byte,
            std::slice::from_ref(&chain_returns),
        ) {
            Some(ty) => resolve_fqn(doc, &ty, &imports),
            None => return Vec::new(),
        }
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
            let cn_short = candidate.rsplit('\\').next().unwrap_or(candidate);
            for (uri, idx) in indexes {
                for cls in &idx.classes {
                    let short = cls
                        .name
                        .as_ref()
                        .rsplit('\\')
                        .next()
                        .unwrap_or(cls.name.as_ref());
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
