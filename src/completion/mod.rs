mod keyword;
pub use keyword::{keyword_completions, magic_constant_completions};

mod symbols;
pub use symbols::{
    builtin_completions, superglobal_completions, symbol_completions, symbol_completions_before,
};

mod member;
use member::{
    all_instance_members, all_static_members, magic_method_completions, resolve_receiver_class,
    resolve_static_receiver,
};

mod namespace;
use namespace::{
    collect_classes_with_ns, collect_fqns_with_prefix, current_file_namespace, typed_prefix,
    use_completion_prefix, use_insert_position,
};

use std::sync::Arc;

use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, InsertTextFormat, Position, Range, TextEdit, Url,
};

use tower_lsp::lsp_types::{Documentation, MarkupContent, MarkupKind};

use crate::ast::{MethodReturnsMap, ParsedDoc, format_type_hint};
use crate::docblock::find_docblock;
use crate::hover::format_params_str;
use crate::phpstorm_meta::PhpStormMeta;
use crate::type_map::{
    TypeMap, build_method_returns, enclosing_class_at, members_of_class, params_of_function,
    params_of_method,
};
use crate::util::{camel_sort_key, fuzzy_camel_match, utf16_offset_to_byte};
use std::collections::HashMap;

/// Build a `CompletionItem` for a callable (function or method).
///
/// If the function has parameters the item uses snippet format with `$1`
/// inside the parentheses so the cursor lands there.  Zero-parameter
/// callables insert `name()` as plain text.
fn callable_item(label: &str, kind: CompletionItemKind, has_params: bool) -> CompletionItem {
    if has_params {
        CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            insert_text: Some(format!("{}($1)", label)),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        }
    } else {
        CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            insert_text: Some(format!("{}()", label)),
            ..Default::default()
        }
    }
}

/// Build a named-argument `CompletionItem` for a callable when param names are
/// known.  Produces a label like `create(name:, age:)` and a snippet like
/// `create(name: $1, age: $2)`.  Returns `None` when the param list is empty
/// (no advantage over the positional item in that case).
fn named_arg_item(
    label: &str,
    kind: CompletionItemKind,
    params: &[php_ast::Param<'_, '_>],
) -> Option<CompletionItem> {
    if params.is_empty() {
        return None;
    }
    let named_label = format!(
        "{}({})",
        label,
        params
            .iter()
            .map(|p| format!("{}:", p.name))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let snippet = format!(
        "{}({})",
        label,
        params
            .iter()
            .enumerate()
            .map(|(i, p)| format!("{}: ${}", p.name, i + 1))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Some(CompletionItem {
        label: named_label,
        kind: Some(kind),
        insert_text: Some(snippet),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        detail: Some("named args".to_string()),
        ..Default::default()
    })
}

/// Build the full signature string for a callable, e.g.
/// `"function foo(string $bar, int $baz): bool"`.
fn build_function_sig(
    name: &str,
    params: &[php_ast::Param<'_, '_>],
    return_type: Option<&php_ast::TypeHint<'_, '_>>,
) -> String {
    let params_str = format_params_str(params);
    let ret = return_type
        .map(|r| format!(": {}", format_type_hint(r)))
        .unwrap_or_default();
    format!("function {}({}){}", name, params_str, ret)
}

/// Build a `Documentation` value from a docblock found before `sym_name` in `doc`.
fn docblock_docs(doc: &ParsedDoc, sym_name: &str) -> Option<Documentation> {
    let db = find_docblock(doc.source(), &doc.program().stmts, sym_name)?;
    let md = db.to_markdown();
    if md.is_empty() {
        None
    } else {
        Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: md,
        }))
    }
}

/// If the `(` trigger occurs inside an attribute like `#[ClassName(`, extract
/// the attribute class name so we can offer its `__construct` parameter names.
fn resolve_attribute_class(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = line[..col].trim_end_matches('(').trim_end();
    // Look backwards on the same line for `#[ClassName` or `#[\NS\ClassName`
    let hash_pos = before.rfind("#[")?;
    let after_bracket = before[hash_pos + 2..].trim_start();
    // Strip leading backslashes (FQN), keep the short name
    let name: String = after_bracket
        .trim_start_matches('\\')
        .rsplit('\\')
        .next()
        .unwrap_or("")
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() { None } else { Some(name) }
}

fn resolve_call_params(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    position: Position,
) -> Vec<String> {
    let line = match source.lines().nth(position.line as usize) {
        Some(l) => l,
        None => return vec![],
    };
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..col];
    let before = before.strip_suffix('(').unwrap_or(before);
    let func_name: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if func_name.is_empty() {
        return vec![];
    }
    let mut params = params_of_function(doc, &func_name);
    if params.is_empty() {
        for other in other_docs {
            params = params_of_function(other, &func_name);
            if !params.is_empty() {
                break;
            }
        }
    }
    params
}

/// Optional context for completion requests that enables richer results
/// (e.g. auto-import edits, `->` scoping to a class).
#[derive(Default)]
pub struct CompletionCtx<'a> {
    pub source: Option<&'a str>,
    pub position: Option<Position>,
    pub meta: Option<&'a PhpStormMeta>,
    pub doc_uri: Option<&'a Url>,
    pub file_imports: Option<&'a HashMap<String, String>>,
    /// Salsa-memoized method-return map for the primary doc. If `None`,
    /// `filtered_completions_at` builds one inline. Production callers
    /// pass the salsa-cached Arc to avoid recomputing per request.
    pub doc_returns: Option<&'a MethodReturnsMap>,
    /// Salsa-memoized method-return maps aligned with `other_docs`. Must be
    /// the same length as `other_docs` when set, or `None` to build inline.
    pub other_returns: Option<&'a [Arc<MethodReturnsMap>]>,
}

/// Completions filtered by trigger character, with optional context
/// so that `->` completions can be scoped to the variable's class.
pub fn filtered_completions_at(
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    trigger_character: Option<&str>,
    ctx: &CompletionCtx<'_>,
) -> Vec<CompletionItem> {
    let source = ctx.source;
    let position = ctx.position;
    let meta = ctx.meta;
    let doc_uri = ctx.doc_uri;
    let empty_imports = HashMap::new();
    let imports = ctx.file_imports.unwrap_or(&empty_imports);

    // Materialize method-return maps either from the salsa-provided context
    // or by building them inline (tests / callers that don't pass them).
    let doc_returns_owned: Option<MethodReturnsMap> =
        ctx.doc_returns.is_none().then(|| build_method_returns(doc));
    let doc_returns_ref: &MethodReturnsMap = ctx
        .doc_returns
        .unwrap_or_else(|| doc_returns_owned.as_ref().expect("initialized above"));
    let other_returns_owned: Option<Vec<MethodReturnsMap>> = ctx
        .other_returns
        .is_none()
        .then(|| other_docs.iter().map(|d| build_method_returns(d)).collect());
    let other_returns_refs: Vec<&MethodReturnsMap> = match ctx.other_returns {
        Some(arcs) => arcs.iter().map(|a| a.as_ref()).collect(),
        None => other_returns_owned
            .as_ref()
            .expect("initialized above")
            .iter()
            .collect(),
    };
    let others_with_returns: Vec<(&ParsedDoc, &MethodReturnsMap)> = other_docs
        .iter()
        .map(|d| d.as_ref())
        .zip(other_returns_refs.iter().copied())
        .collect();
    match trigger_character {
        Some("$") => {
            let mut items = superglobal_completions();
            items.extend(
                symbol_completions(doc)
                    .into_iter()
                    .filter(|i| i.kind == Some(CompletionItemKind::VARIABLE)),
            );
            items
        }
        Some(">") => {
            // Arrow: $obj->  or  $this->
            if let (Some(src), Some(pos)) = (source, position) {
                let type_map = TypeMap::from_docs_with_meta(
                    doc,
                    doc_returns_ref,
                    others_with_returns.iter().copied(),
                    meta,
                );
                if let Some(class_names) = resolve_receiver_class(src, doc, pos, &type_map) {
                    // Feature 5: support union types (Foo|Bar)
                    let mut items = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for class_name in class_names.split('|') {
                        let class_name = class_name.trim();
                        for item in all_instance_members(class_name, doc, other_docs) {
                            if seen.insert(item.label.clone()) {
                                items.push(item);
                            }
                        }
                    }
                    if !items.is_empty() {
                        return items;
                    }
                }
            }
            // Fallback: all methods from current doc
            symbol_completions(doc)
                .into_iter()
                .filter(|i| i.kind == Some(CompletionItemKind::METHOD))
                .collect()
        }
        Some(":") => {
            // Static access: ClassName:: / self:: / static:: / parent::
            if let (Some(src), Some(pos)) = (source, position)
                && let Some(class_name) = resolve_static_receiver(src, doc, other_docs, pos)
            {
                let items = all_static_members(&class_name, doc, other_docs);
                if !items.is_empty() {
                    return items;
                }
            }
            vec![]
        }
        Some("[") => {
            // PHP attribute: #[ — suggest attribute classes
            if let (Some(src), Some(pos)) = (source, position) {
                let line = src.lines().nth(pos.line as usize).unwrap_or("");
                let col = utf16_offset_to_byte(line, pos.character as usize);
                let before = &line[..col];
                if before.trim_end_matches('[').trim_end().ends_with('#') {
                    let mut items: Vec<CompletionItem> = Vec::new();
                    let cur_ns = current_file_namespace(&doc.program().stmts);
                    let mut seen = std::collections::HashSet::new();

                    // Current doc: no auto-import needed (same file).
                    let mut cur_classes = Vec::new();
                    collect_classes_with_ns(&doc.program().stmts, "", &mut cur_classes);
                    for (label, _kind, _fqn) in cur_classes {
                        if seen.insert(label.clone()) {
                            items.push(CompletionItem {
                                label,
                                kind: Some(CompletionItemKind::CLASS),
                                ..Default::default()
                            });
                        }
                    }

                    // Other docs: add `use` statement when crossing namespaces.
                    for other in other_docs {
                        let mut classes = Vec::new();
                        collect_classes_with_ns(&other.program().stmts, "", &mut classes);
                        for (label, _kind, fqn) in classes {
                            if !seen.insert(label.clone()) {
                                continue;
                            }
                            let in_same_ns =
                                !cur_ns.is_empty() && fqn == format!("{}\\{}", cur_ns, label);
                            let is_global = !fqn.contains('\\');
                            let already = imports.contains_key(&label);
                            let additional_text_edits = if !in_same_ns && !is_global && !already {
                                let insert_pos = use_insert_position(src);
                                Some(vec![TextEdit {
                                    range: Range {
                                        start: insert_pos,
                                        end: insert_pos,
                                    },
                                    new_text: format!("use {};\n", fqn),
                                }])
                            } else {
                                None
                            };
                            items.push(CompletionItem {
                                label,
                                kind: Some(CompletionItemKind::CLASS),
                                detail: if fqn.contains('\\') { Some(fqn) } else { None },
                                additional_text_edits,
                                ..Default::default()
                            });
                        }
                    }
                    return items;
                }
            }
            vec![]
        }
        Some("(") => {
            // Named argument: funcName(
            if let (Some(src), Some(pos)) = (source, position) {
                let params = resolve_call_params(src, doc, other_docs, pos);
                if !params.is_empty() {
                    return params
                        .into_iter()
                        .map(|p| CompletionItem {
                            label: format!("{p}:"),
                            kind: Some(CompletionItemKind::VARIABLE),
                            ..Default::default()
                        })
                        .collect();
                }
                // Attribute constructor: #[ClassName(
                if let Some(attr_class) = resolve_attribute_class(src, pos) {
                    let mut attr_params = params_of_method(doc, &attr_class, "__construct");
                    if attr_params.is_empty() {
                        for other in other_docs {
                            attr_params = params_of_method(other, &attr_class, "__construct");
                            if !attr_params.is_empty() {
                                break;
                            }
                        }
                    }
                    if !attr_params.is_empty() {
                        return attr_params
                            .into_iter()
                            .map(|p| CompletionItem {
                                label: format!("{p}:"),
                                kind: Some(CompletionItemKind::VARIABLE),
                                detail: Some(format!("#{attr_class} argument")),
                                ..Default::default()
                            })
                            .collect();
                    }
                }
            }
            vec![]
        }
        _ => {
            // Detect $obj->member context (invoked completion without trigger char).
            // Returns only the receiver class's instance members so unrelated class
            // methods don't pollute the list.
            if let (Some(src), Some(pos)) = (source, position) {
                let line = src.lines().nth(pos.line as usize).unwrap_or("");
                let col = utf16_offset_to_byte(line, pos.character as usize);
                let before = &line[..col];
                // Strip any identifier chars the user is typing as the member prefix.
                let pre_arrow = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
                let has_arrow = pre_arrow.ends_with("->") || pre_arrow.ends_with("?->");
                if has_arrow {
                    let type_map = TypeMap::from_docs_with_meta(
                        doc,
                        doc_returns_ref,
                        others_with_returns.iter().copied(),
                        meta,
                    );
                    // Extract receiver var from text before the arrow.
                    let arrow_stripped = pre_arrow
                        .strip_suffix("->")
                        .or_else(|| pre_arrow.strip_suffix("?->"))
                        .unwrap_or(pre_arrow);
                    let receiver: String = arrow_stripped
                        .chars()
                        .rev()
                        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '$')
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                    let receiver = if receiver.starts_with('$') {
                        receiver
                    } else if !receiver.is_empty() {
                        format!("${receiver}")
                    } else {
                        String::new()
                    };
                    let class_name = if receiver == "$this" {
                        enclosing_class_at(src, doc, pos)
                            .or_else(|| type_map.get("$this").map(|s| s.to_string()))
                    } else if !receiver.is_empty() {
                        type_map.get(&receiver).map(|s| s.to_string())
                    } else {
                        None
                    };
                    if let Some(cls) = class_name {
                        let mut items = Vec::new();
                        let mut seen = std::collections::HashSet::new();
                        for class_name in cls.split('|') {
                            for item in all_instance_members(class_name.trim(), doc, other_docs) {
                                if seen.insert(item.label.clone()) {
                                    items.push(item);
                                }
                            }
                        }
                        if !items.is_empty() {
                            return items;
                        }
                    }
                }
            }

            // Feature 4: detect `use ` context and suggest FQNs from other docs
            if let (Some(src), Some(pos)) = (source, position)
                && let Some(use_prefix) = use_completion_prefix(src, pos)
            {
                let mut use_items: Vec<CompletionItem> = Vec::new();
                for other in other_docs {
                    collect_fqns_with_prefix(
                        &other.program().stmts,
                        "",
                        &use_prefix,
                        &mut use_items,
                    );
                }
                // Also check current doc
                collect_fqns_with_prefix(&doc.program().stmts, "", &use_prefix, &mut use_items);
                if !use_items.is_empty() {
                    return use_items;
                }
            }

            // Feature 9: include/require path completions
            if let (Some(src), Some(pos), Some(uri)) = (source, position, doc_uri)
                && let Some(prefix) = include_path_prefix(src, pos)
            {
                let items = include_path_completions(uri, &prefix);
                if !items.is_empty() {
                    return items;
                }
            }

            // Feature 3: Sub-namespace \ completions outside use statement
            if let (Some(src), Some(pos)) = (source, position)
                && let Some(prefix) = typed_prefix(Some(src), Some(pos))
                && prefix.contains('\\')
            {
                // Check we're NOT in a use statement
                let is_use = use_completion_prefix(src, pos).is_some();
                if !is_use {
                    let prefix_lc = prefix.to_lowercase();
                    let mut ns_items: Vec<CompletionItem> = Vec::new();
                    for other in other_docs {
                        let mut classes = Vec::new();
                        collect_classes_with_ns(&other.program().stmts, "", &mut classes);
                        for (label, kind, fqn) in classes {
                            if fqn
                                .get(..prefix_lc.len())
                                .is_some_and(|s| s.eq_ignore_ascii_case(&prefix_lc))
                            {
                                ns_items.push(CompletionItem {
                                    label: label.clone(),
                                    kind: Some(kind),
                                    insert_text: Some(label),
                                    detail: Some(fqn),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                    let mut classes = Vec::new();
                    collect_classes_with_ns(&doc.program().stmts, "", &mut classes);
                    for (label, kind, fqn) in classes {
                        if fqn
                            .get(..prefix_lc.len())
                            .is_some_and(|s| s.eq_ignore_ascii_case(&prefix_lc))
                        {
                            ns_items.push(CompletionItem {
                                label: label.clone(),
                                kind: Some(kind),
                                insert_text: Some(label),
                                detail: Some(fqn),
                                ..Default::default()
                            });
                        }
                    }
                    if !ns_items.is_empty() {
                        return ns_items;
                    }
                }
            }

            // Feature 7: match arm completions
            if let (Some(src), Some(pos)) = (source, position)
                && let Some(match_items) = match_arm_completions(
                    src,
                    doc,
                    doc_returns_ref,
                    other_docs,
                    &others_with_returns,
                    pos,
                    meta,
                )
                && !match_items.is_empty()
            {
                let mut all = match_items;
                // extend with normal items below, but return early here
                let mut normal_items = keyword_completions();
                normal_items.extend(magic_constant_completions());
                normal_items.extend(builtin_completions());
                normal_items.extend(superglobal_completions());
                normal_items.extend(symbol_completions(doc));
                all.extend(normal_items);
                return all;
            }

            // Feature 5: Magic method completions in class body
            let mut magic_items: Vec<CompletionItem> = Vec::new();
            if let (Some(src), Some(pos)) = (source, position)
                && enclosing_class_at(src, doc, pos).is_some()
            {
                magic_items.extend(magic_method_completions());
            }

            let mut items = keyword_completions();
            items.extend(magic_constant_completions());
            items.extend(builtin_completions());
            items.extend(superglobal_completions());
            // Feature 2: scope variable completions to before cursor line
            let sym_items = if let (Some(_src), Some(pos)) = (source, position) {
                symbol_completions_before(doc, pos.line)
            } else {
                symbol_completions(doc)
            };
            items.extend(sym_items);
            items.extend(magic_items);

            let cur_ns = current_file_namespace(&doc.program().stmts);

            for other in other_docs {
                // Class-like symbols: add `use` insertion when needed.
                let mut classes: Vec<(String, CompletionItemKind, String)> = Vec::new();
                collect_classes_with_ns(&other.program().stmts, "", &mut classes);
                for (label, kind, fqn) in classes {
                    let additional_text_edits = if let Some(src) = source {
                        let in_same_ns =
                            !cur_ns.is_empty() && fqn == format!("{}\\{}", cur_ns, label);
                        let is_global = !fqn.contains('\\');
                        let already = imports.contains_key(&label);
                        if !in_same_ns && !is_global && !already {
                            let pos = use_insert_position(src);
                            Some(vec![TextEdit {
                                range: Range {
                                    start: pos,
                                    end: pos,
                                },
                                new_text: format!("use {};\n", fqn),
                            }])
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    items.push(CompletionItem {
                        label,
                        kind: Some(kind),
                        detail: if fqn.contains('\\') { Some(fqn) } else { None },
                        additional_text_edits,
                        ..Default::default()
                    });
                }
                // Non-class symbols (functions, methods, constants) need no use statement.
                let cross: Vec<CompletionItem> = symbol_completions(other)
                    .into_iter()
                    .filter(|i| {
                        !matches!(
                            i.kind,
                            Some(CompletionItemKind::CLASS)
                                | Some(CompletionItemKind::INTERFACE)
                                | Some(CompletionItemKind::ENUM)
                        ) && i.kind != Some(CompletionItemKind::VARIABLE)
                    })
                    .collect();
                items.extend(cross);
            }
            let mut seen = std::collections::HashSet::new();
            items.retain(|i| seen.insert(i.label.clone()));

            // Extract the typed prefix for fuzzy camel/underscore filtering.
            let prefix = typed_prefix(source, position).unwrap_or_default();
            if prefix.contains('\\') {
                // Namespace-qualified prefix: filter by FQN prefix match.
                let ns_prefix = prefix.trim_start_matches('\\').to_lowercase();
                items.retain(|i| {
                    let fqn = i.detail.as_deref().unwrap_or(&i.label);
                    fqn.get(..ns_prefix.len())
                        .is_some_and(|s| s.eq_ignore_ascii_case(&ns_prefix))
                });
            } else if !prefix.is_empty() {
                items.retain(|i| fuzzy_camel_match(&prefix, &i.label));
                for item in &mut items {
                    item.sort_text = Some(camel_sort_key(&prefix, &item.label));
                    item.filter_text = Some(item.label.clone());
                }
            }
            items
        }
    }
}

fn match_arm_completions(
    source: &str,
    doc: &ParsedDoc,
    doc_returns: &MethodReturnsMap,
    other_docs: &[Arc<ParsedDoc>],
    others_with_returns: &[(&ParsedDoc, &MethodReturnsMap)],
    position: Position,
    meta: Option<&PhpStormMeta>,
) -> Option<Vec<CompletionItem>> {
    let start_line = position.line as usize;
    let end_line = start_line.saturating_sub(5);
    let all_lines: Vec<&str> = source.lines().collect();
    let type_map_cell: std::cell::OnceCell<TypeMap> = std::cell::OnceCell::new();
    for line_idx in (end_line..=start_line).rev() {
        let line = all_lines.get(line_idx).copied()?;
        if let Some(cap) = extract_match_subject(line) {
            let class_name = if cap == "this" {
                enclosing_class_at(source, doc, position)?
            } else {
                let type_map = type_map_cell.get_or_init(|| {
                    TypeMap::from_docs_with_meta(
                        doc,
                        doc_returns,
                        others_with_returns.iter().copied(),
                        meta,
                    )
                });
                type_map.get(&format!("${cap}"))?.to_string()
            };
            let all_docs: Vec<&ParsedDoc> = std::iter::once(doc)
                .chain(other_docs.iter().map(|d| d.as_ref()))
                .collect();
            for d in &all_docs {
                let members = members_of_class(d, &class_name);
                if !members.constants.is_empty() {
                    return Some(
                        members
                            .constants
                            .iter()
                            .map(|c| CompletionItem {
                                label: format!("{class_name}::{c}"),
                                kind: Some(CompletionItemKind::CONSTANT),
                                ..Default::default()
                            })
                            .collect(),
                    );
                }
            }
        }
    }
    None
}

/// Returns the path prefix typed inside a string on an include/require line, or None.
/// Only triggers for relative paths (starting with `./`, `../`, or empty after the quote)
/// so that absolute-path strings are left alone.
fn include_path_prefix(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let trimmed = line.trim_start();
    if !trimmed.starts_with("include") && !trimmed.starts_with("require") {
        return None;
    }
    // Find the string being typed
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..col];
    let quote_pos = before.rfind(['\'', '"'])?;
    let typed = &before[quote_pos + 1..];
    // Only offer completions for relative paths (./  ../  or empty start)
    // and not for absolute paths (starting with /) or PHP stream wrappers.
    if typed.starts_with('/') || typed.contains("://") {
        return None;
    }
    Some(typed.to_string())
}

/// Build completion items for include/require path strings.
///
/// `prefix` is the partial path typed so far (e.g. `"../lib/"` or `"./"`).
/// The returned `insert_text` for each item is the full replacement text
/// from the opening quote to the end of the completed entry, so that the
/// LSP client can replace the whole typed path (not just the last segment).
fn include_path_completions(doc_uri: &Url, prefix: &str) -> Vec<CompletionItem> {
    use std::path::Path;

    let doc_path = match doc_uri.to_file_path() {
        Ok(p) => p,
        Err(_) => return vec![],
    };
    let doc_dir = match doc_path.parent() {
        Some(d) => d.to_path_buf(),
        None => return vec![],
    };

    // Split prefix into a directory part (already traversed) and the partial filename.
    let (dir_prefix, typed_file) = if prefix.ends_with('/') || prefix.ends_with('\\') {
        (prefix.to_string(), String::new())
    } else {
        let p = Path::new(prefix);
        let parent = p
            .parent()
            .map(|p| {
                let s = p.to_string_lossy();
                if s.is_empty() {
                    String::new()
                } else {
                    format!("{}/", s)
                }
            })
            .unwrap_or_default();
        let file = p
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        (parent, file)
    };

    let dir_to_list = doc_dir.join(&dir_prefix);

    let entries = match std::fs::read_dir(&dir_to_list) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut items = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Skip hidden files/dirs unless the prefix already starts with a dot.
        if name.starts_with('.') && !typed_file.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let is_php = name.ends_with(".php") || name.ends_with(".inc") || name.ends_with(".phtml");
        if !is_dir && !is_php {
            continue;
        }
        let entry_name = if is_dir {
            format!("{}/", name)
        } else {
            name.clone()
        };
        // insert_text is the full path from the opening quote so the whole
        // typed prefix (e.g. "../lib/") is preserved in the replacement.
        let insert_text = format!("{}{}", dir_prefix, entry_name);
        items.push(CompletionItem {
            label: name,
            kind: Some(if is_dir {
                CompletionItemKind::FOLDER
            } else {
                CompletionItemKind::FILE
            }),
            insert_text: Some(insert_text),
            ..Default::default()
        });
    }
    items.sort_by(|a, b| {
        // Directories first, then files
        let a_dir = a.kind == Some(CompletionItemKind::FOLDER);
        let b_dir = b.kind == Some(CompletionItemKind::FOLDER);
        b_dir.cmp(&a_dir).then(a.label.cmp(&b.label))
    });
    items
}

fn extract_match_subject(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let after = trimmed.strip_prefix("match")?.trim_start();
    let after = after.strip_prefix('(')?;
    let inner: String = after.chars().take_while(|&c| c != ')').collect();
    let var = inner.trim().trim_start_matches('$');
    if var.is_empty() {
        None
    } else {
        Some(var.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(source: &str) -> ParsedDoc {
        ParsedDoc::parse(source.to_string())
    }

    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn keywords_list_is_non_empty() {
        let kws = keyword_completions();
        assert!(
            kws.len() >= 20,
            "expected at least 20 keywords, got {}",
            kws.len()
        );
    }

    #[test]
    fn keywords_contain_common_php_keywords() {
        let kws = keyword_completions();
        let ls = labels(&kws);
        for expected in &[
            "function",
            "class",
            "return",
            "foreach",
            "match",
            "namespace",
        ] {
            assert!(ls.contains(expected), "missing keyword: {expected}");
        }
    }

    #[test]
    fn all_keyword_items_have_keyword_kind() {
        for item in keyword_completions() {
            assert_eq!(item.kind, Some(CompletionItemKind::KEYWORD));
        }
    }

    #[test]
    fn magic_constants_all_present() {
        let items = magic_constant_completions();
        let ls: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for name in &[
            "__FILE__",
            "__DIR__",
            "__LINE__",
            "__CLASS__",
            "__FUNCTION__",
            "__METHOD__",
            "__NAMESPACE__",
            "__TRAIT__",
        ] {
            assert!(ls.contains(name), "missing magic constant: {name}");
        }
    }

    #[test]
    fn magic_constants_have_constant_kind() {
        for item in magic_constant_completions() {
            assert_eq!(
                item.kind,
                Some(CompletionItemKind::CONSTANT),
                "{} should have CONSTANT kind",
                item.label
            );
        }
    }

    #[test]
    fn resolve_attribute_class_extracts_name() {
        let src = "<?php\n#[Route(\n";
        // Position right after the '(' on line 1
        let pos = Position {
            line: 1,
            character: 8,
        };
        let result = resolve_attribute_class(src, pos);
        assert_eq!(result.as_deref(), Some("Route"));
    }

    #[test]
    fn resolve_attribute_class_fqn_extracts_short_name() {
        let src = "<?php\n#[\\Symfony\\Component\\Routing\\Route(\n";
        let pos = Position {
            line: 1,
            character: 38,
        };
        let result = resolve_attribute_class(src, pos);
        assert_eq!(result.as_deref(), Some("Route"));
    }

    #[test]
    fn resolve_attribute_class_returns_none_for_regular_call() {
        let src = "<?php\nsomeFunction(\n";
        let pos = Position {
            line: 1,
            character: 14,
        };
        let result = resolve_attribute_class(src, pos);
        assert!(result.is_none(), "should not match regular function call");
    }

    #[test]
    fn extracts_top_level_function_name() {
        let d = doc("<?php\nfunction greet() {}");
        let items = symbol_completions(&d);
        assert!(labels(&items).contains(&"greet"));
        let greet = items.iter().find(|i| i.label == "greet").unwrap();
        assert_eq!(greet.kind, Some(CompletionItemKind::FUNCTION));
    }

    #[test]
    fn extracts_top_level_class_name() {
        let d = doc("<?php\nclass MyService {}");
        let items = symbol_completions(&d);
        assert!(labels(&items).contains(&"MyService"));
        let cls = items.iter().find(|i| i.label == "MyService").unwrap();
        assert_eq!(cls.kind, Some(CompletionItemKind::CLASS));
    }

    #[test]
    fn extracts_class_method_names() {
        let d = doc("<?php\nclass Calc { public function add() {} public function sub() {} }");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(ls.contains(&"add"), "missing 'add'");
        assert!(ls.contains(&"sub"), "missing 'sub'");
        for item in items
            .iter()
            .filter(|i| i.label == "add" || i.label == "sub")
        {
            assert_eq!(item.kind, Some(CompletionItemKind::METHOD));
        }
    }

    #[test]
    fn extracts_function_parameters_as_variables() {
        let d = doc("<?php\nfunction process($input, $count) {}");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(ls.contains(&"$input"), "missing '$input'");
        assert!(ls.contains(&"$count"), "missing '$count'");
    }

    #[test]
    fn extracts_symbols_inside_namespace() {
        let d = doc("<?php\nnamespace App {\nfunction render() {}\nclass View {}\n}");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(ls.contains(&"render"), "missing 'render'");
        assert!(ls.contains(&"View"), "missing 'View'");
    }

    #[test]
    fn extracts_interface_name() {
        let d = doc("<?php\ninterface Serializable {}");
        let items = symbol_completions(&d);
        let item = items.iter().find(|i| i.label == "Serializable");
        assert!(item.is_some(), "missing 'Serializable'");
        assert_eq!(item.unwrap().kind, Some(CompletionItemKind::INTERFACE));
    }

    #[test]
    fn variable_assignment_produces_variable_item() {
        let d = doc("<?php\n$name = 'Alice';");
        let items = symbol_completions(&d);
        assert!(labels(&items).contains(&"$name"), "missing '$name'");
    }

    #[test]
    fn class_property_appears_in_completions() {
        let d = doc("<?php\nclass User { public string $name; private int $age; }");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(ls.contains(&"$name"), "missing '$name'");
        assert!(ls.contains(&"$age"), "missing '$age'");
        for item in items
            .iter()
            .filter(|i| i.label == "$name" || i.label == "$age")
        {
            assert_eq!(item.kind, Some(CompletionItemKind::PROPERTY));
        }
    }

    #[test]
    fn class_constant_appears_in_completions() {
        let d = doc("<?php\nclass Status { const ACTIVE = 1; const INACTIVE = 0; }");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(ls.contains(&"ACTIVE"), "missing 'ACTIVE'");
        assert!(ls.contains(&"INACTIVE"), "missing 'INACTIVE'");
    }

    #[test]
    fn dollar_trigger_returns_only_variables() {
        let d = doc("<?php\nfunction greet($name) {}\nclass Foo {}\n$bar = 1;");
        let items = filtered_completions_at(&d, &[], Some("$"), &CompletionCtx::default());
        assert!(!items.is_empty(), "should have variable items");
        for item in &items {
            assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
        }
        let ls = labels(&items);
        assert!(!ls.contains(&"greet"), "should not contain function");
        assert!(!ls.contains(&"Foo"), "should not contain class");
    }

    #[test]
    fn arrow_trigger_returns_only_methods() {
        let d = doc("<?php\nclass Calc { public function add() {} public function sub() {} }");
        let items = filtered_completions_at(&d, &[], Some(">"), &CompletionCtx::default());
        assert!(!items.is_empty(), "should have method items");
        for item in &items {
            assert_eq!(item.kind, Some(CompletionItemKind::METHOD));
        }
    }

    #[test]
    fn none_trigger_returns_keywords_functions_classes() {
        let d = doc("<?php\nfunction greet() {}\nclass MyApp {}");
        let items = filtered_completions_at(&d, &[], None, &CompletionCtx::default());
        let ls = labels(&items);
        assert!(
            ls.contains(&"function"),
            "should contain keyword 'function'"
        );
        assert!(ls.contains(&"greet"), "should contain function 'greet'");
        assert!(ls.contains(&"MyApp"), "should contain class 'MyApp'");
    }

    #[test]
    fn builtins_appear_in_default_completions() {
        let d = doc("<?php");
        let items = filtered_completions_at(&d, &[], None, &CompletionCtx::default());
        let ls = labels(&items);
        assert!(ls.contains(&"strlen"), "missing strlen");
        assert!(ls.contains(&"array_map"), "missing array_map");
        assert!(ls.contains(&"json_encode"), "missing json_encode");
    }

    #[test]
    fn colon_trigger_returns_static_members() {
        let src = "<?php\nclass Cfg { public static function load(): void {} public static int $debug = 0; const VERSION = '1'; }\nCfg::";
        let d = doc(src);
        let pos = Position {
            line: 2,
            character: 5,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(":"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(ls.contains(&"load"), "missing static method");
        assert!(ls.contains(&"VERSION"), "missing constant");
    }

    #[test]
    fn inherited_methods_appear_in_arrow_completion() {
        let src = "<?php\nclass Base { public function baseMethod() {} }\nclass Child extends Base { public function childMethod() {} }\n$c = new Child();\n$c->";
        let d = doc(src);
        let pos = Position {
            line: 4,
            character: 4,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(ls.contains(&"baseMethod"), "missing inherited baseMethod");
        assert!(ls.contains(&"childMethod"), "missing childMethod");
    }

    #[test]
    fn param_named_arg_completion() {
        let src = "<?php\nfunction connect(string $host, int $port): void {}\nconnect(";
        let d = doc(src);
        let pos = Position {
            line: 2,
            character: 8,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some("("),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(ls.contains(&"host:"), "missing host:");
        assert!(ls.contains(&"port:"), "missing port:");
    }

    #[test]
    fn cross_file_symbols_appear_in_default_completions() {
        let d = doc("<?php\nfunction localFn() {}");
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nclass RemoteService {}\nfunction remoteHelper() {}".to_string(),
        ));
        let items = filtered_completions_at(&d, &[other], None, &CompletionCtx::default());
        let ls = labels(&items);
        assert!(ls.contains(&"localFn"), "missing local function");
        assert!(ls.contains(&"RemoteService"), "missing cross-file class");
        assert!(ls.contains(&"remoteHelper"), "missing cross-file function");
    }

    #[test]
    fn cross_file_variables_not_included_in_default_completions() {
        let d = doc("<?php\n$localVar = 1;");
        let other = Arc::new(ParsedDoc::parse("<?php\n$remoteVar = 2;".to_string()));
        let items = filtered_completions_at(&d, &[other], None, &CompletionCtx::default());
        let ls = labels(&items);
        assert!(
            !ls.contains(&"$remoteVar"),
            "cross-file variable should not appear"
        );
    }

    #[test]
    fn cross_file_class_gets_use_insertion() {
        let current_src = "<?php\nnamespace App;\n\n$x = new ";
        let d = doc(current_src);
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nnamespace Lib;\nclass Mailer {}".to_string(),
        ));
        let pos = Position {
            line: 3,
            character: 9,
        };
        let items = filtered_completions_at(
            &d,
            &[other],
            None,
            &CompletionCtx {
                source: Some(current_src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let mailer = items.iter().find(|i| i.label == "Mailer");
        assert!(mailer.is_some(), "Mailer should appear in completions");
        let edits = mailer.unwrap().additional_text_edits.as_ref();
        assert!(edits.is_some(), "Mailer should have additionalTextEdits");
        let edit_text = &edits.unwrap()[0].new_text;
        assert!(
            edit_text.contains("use Lib\\Mailer;"),
            "edit should insert 'use Lib\\Mailer;', got: {edit_text}"
        );
    }

    #[test]
    fn same_namespace_class_gets_no_use_insertion() {
        let current_src = "<?php\nnamespace Lib;\n$x = new ";
        let d = doc(current_src);
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nnamespace Lib;\nclass Mailer {}".to_string(),
        ));
        let pos = Position {
            line: 2,
            character: 9,
        };
        let items = filtered_completions_at(
            &d,
            &[other],
            None,
            &CompletionCtx {
                source: Some(current_src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let mailer = items.iter().find(|i| i.label == "Mailer");
        assert!(mailer.is_some(), "Mailer should appear in completions");
        assert!(
            mailer.unwrap().additional_text_edits.is_none(),
            "same-namespace class should not get a use edit"
        );
    }

    #[test]
    fn function_with_params_gets_snippet() {
        let d = doc("<?php\nfunction process($input) {}");
        let items = symbol_completions(&d);
        let item = items.iter().find(|i| i.label == "process").unwrap();
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert_eq!(item.insert_text.as_deref(), Some("process($1)"));
    }

    #[test]
    fn function_without_params_gets_plain_call() {
        let d = doc("<?php\nfunction doThing() {}");
        let items = symbol_completions(&d);
        let item = items.iter().find(|i| i.label == "doThing").unwrap();
        // No snippet format needed for zero-arg functions.
        assert_eq!(item.insert_text.as_deref(), Some("doThing()"));
        assert_ne!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
    }

    #[test]
    fn builtin_functions_get_snippet() {
        let items = builtin_completions();
        let strlen = items.iter().find(|i| i.label == "strlen").unwrap();
        assert_eq!(strlen.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert_eq!(strlen.insert_text.as_deref(), Some("strlen($1)"));
    }

    #[test]
    fn enum_arrow_completion_includes_name_property() {
        let src = "<?php\nenum Suit { case Hearts; }\n$s = new Suit();\n$s->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 4,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        assert!(
            items.iter().any(|i| i.label == "name"),
            "enum should have ->name"
        );
    }

    #[test]
    fn backed_enum_arrow_completion_includes_value_property() {
        let src =
            "<?php\nenum Status: string { case Active = 'active'; }\n$s = new Status();\n$s->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 4,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        assert!(
            items.iter().any(|i| i.label == "name"),
            "backed enum should have ->name"
        );
        assert!(
            items.iter().any(|i| i.label == "value"),
            "backed enum should have ->value"
        );
    }

    #[test]
    fn pure_enum_arrow_completion_has_no_value_property() {
        let src = "<?php\nenum Suit { case Hearts; }\n$s = new Suit();\n$s->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 4,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        assert!(
            !items.iter().any(|i| i.label == "value"),
            "pure enum should not have ->value"
        );
    }

    #[test]
    fn superglobals_appear_on_dollar_trigger() {
        let d = doc("<?php\n");
        let items = filtered_completions_at(&d, &[], Some("$"), &CompletionCtx::default());
        let ls = labels(&items);
        assert!(ls.contains(&"$_SERVER"), "missing $_SERVER");
        assert!(ls.contains(&"$_GET"), "missing $_GET");
        assert!(ls.contains(&"$_POST"), "missing $_POST");
        assert!(ls.contains(&"$_SESSION"), "missing $_SESSION");
        assert!(ls.contains(&"$GLOBALS"), "missing $GLOBALS");
    }

    #[test]
    fn superglobals_appear_in_default_completions() {
        let d = doc("<?php\n");
        let items = filtered_completions_at(&d, &[], None, &CompletionCtx::default());
        let ls = labels(&items);
        assert!(
            ls.contains(&"$_SERVER"),
            "missing $_SERVER in default completions"
        );
    }

    #[test]
    fn instanceof_narrowing_provides_arrow_completions() {
        // $x instanceof Foo should narrow $x to Foo inside the if body
        let src =
            "<?php\nclass Foo { public function doFoo() {} }\nif ($x instanceof Foo) {\n    $x->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 8,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(
            ls.contains(&"doFoo"),
            "instanceof narrowing should make Foo methods available"
        );
    }

    #[test]
    fn constructor_chain_arrow_completion() {
        let src = "<?php\nclass Builder { public function build() {} public function reset() {} }\n(new Builder())->";
        let d = doc(src);
        let pos = Position {
            line: 2,
            character: 16,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(
            ls.contains(&"build"),
            "constructor chain should complete Builder methods"
        );
        assert!(
            ls.contains(&"reset"),
            "constructor chain should complete Builder methods"
        );
    }

    // Feature 4: use statement FQN completions
    #[test]
    fn use_statement_suggests_fqns() {
        let d = doc("<?php\nuse ");
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nnamespace App\\Services;\nclass Mailer {}".to_string(),
        ));
        let pos = Position {
            line: 1,
            character: 4,
        };
        let items = filtered_completions_at(
            &d,
            &[other],
            None,
            &CompletionCtx {
                source: Some("<?php\nuse "),
                position: Some(pos),
                ..Default::default()
            },
        );
        assert!(
            items.iter().any(|i| i.label.contains("Mailer")),
            "use completion should suggest Mailer"
        );
    }

    // Feature 5: union type param completions
    #[test]
    fn union_type_param_completes_both_classes() {
        let src = "<?php\nclass Foo { public function fooMethod() {} }\nclass Bar { public function barMethod() {} }\n/**\n * @param Foo|Bar $x\n */\nfunction handle($x) {\n    $x->";
        let d = doc(src);
        let pos = Position {
            line: 7,
            character: 8,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(
            ls.contains(&"fooMethod"),
            "should complete Foo methods from union"
        );
        assert!(
            ls.contains(&"barMethod"),
            "should complete Bar methods from union"
        );
    }

    // Feature 6: attribute bracket completions
    #[test]
    fn attribute_bracket_suggests_classes() {
        let d = doc("<?php\nclass Route {}\nclass Middleware {}\n#[");
        let pos = Position {
            line: 3,
            character: 2,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some("["),
            &CompletionCtx {
                source: Some("<?php\nclass Route {}\nclass Middleware {}\n#["),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(ls.contains(&"Route"), "should suggest Route as attribute");
        assert!(
            ls.contains(&"Middleware"),
            "should suggest Middleware as attribute"
        );
    }

    #[test]
    fn attribute_bracket_cross_ns_gets_use_insertion() {
        let current_src = "<?php\nnamespace App\\Controllers;\n\n#[";
        let d = doc(current_src);
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nnamespace App\\Attributes;\nclass Route {}".to_string(),
        ));
        let pos = Position {
            line: 3,
            character: 2,
        };
        let items = filtered_completions_at(
            &d,
            &[other],
            Some("["),
            &CompletionCtx {
                source: Some(current_src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let route = items.iter().find(|i| i.label == "Route");
        assert!(
            route.is_some(),
            "Route should appear in attribute completions"
        );
        let edits = route.unwrap().additional_text_edits.as_ref();
        assert!(
            edits.is_some(),
            "Route attribute should have additionalTextEdits for auto-import"
        );
        let edit_text = &edits.unwrap()[0].new_text;
        assert!(
            edit_text.contains("use App\\Attributes\\Route;"),
            "edit should insert 'use App\\Attributes\\Route;', got: {edit_text}"
        );
    }

    #[test]
    fn attribute_bracket_same_ns_no_use_insertion() {
        let current_src = "<?php\nnamespace App\\Attributes;\n\n#[";
        let d = doc(current_src);
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nnamespace App\\Attributes;\nclass Route {}".to_string(),
        ));
        let pos = Position {
            line: 3,
            character: 2,
        };
        let items = filtered_completions_at(
            &d,
            &[other],
            Some("["),
            &CompletionCtx {
                source: Some(current_src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let route = items.iter().find(|i| i.label == "Route");
        assert!(
            route.is_some(),
            "Route should appear in attribute completions"
        );
        assert!(
            route.unwrap().additional_text_edits.is_none(),
            "same-namespace attribute class should not get a use edit"
        );
    }

    // Feature 7: match arm completions
    #[test]
    fn match_arm_suggests_enum_cases() {
        let src = "<?php\nenum Status { case Active; case Inactive; case Pending; }\n$s = new Status();\nmatch ($s) {\n    ";
        let d = doc(src);
        let pos = Position {
            line: 4,
            character: 4,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            None,
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(
            ls.iter().any(|l| l.contains("Active")),
            "match should suggest Status::Active"
        );
    }

    // Feature 10: readonly property recognition
    #[test]
    fn readonly_property_has_detail_tag() {
        let src = "<?php\nclass Config { public readonly string $name; }\n$c = new Config();\n$c->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 4,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let name_item = items.iter().find(|i| i.label == "$name");
        assert!(name_item.is_some(), "should have $name in completions");
        assert_eq!(
            name_item.unwrap().detail.as_deref(),
            Some("readonly"),
            "$name should be tagged readonly"
        );
    }

    // Feature 2: variables scoped to cursor line
    #[test]
    fn variables_after_cursor_not_suggested() {
        let src = "<?php\n$early = new Foo();\n// cursor here\n$late = new Bar();";
        let d = doc(src);
        let pos = Position {
            line: 2,
            character: 0,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            None,
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(ls.contains(&"$early"), "$early should be suggested");
        assert!(
            !ls.contains(&"$late"),
            "$late declared after cursor should not be suggested"
        );
    }

    // Feature 3: sub-namespace backslash completions
    #[test]
    fn backslash_prefix_suggests_matching_classes() {
        let d = doc("<?php\n$x = new App\\");
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nnamespace App\\Services;\nclass Mailer {}\nclass Logger {}".to_string(),
        ));
        let pos = Position {
            line: 1,
            character: 18,
        };
        let items = filtered_completions_at(
            &d,
            &[other],
            None,
            &CompletionCtx {
                source: Some("<?php\n$x = new App\\"),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(
            ls.contains(&"Mailer"),
            "should suggest Mailer under App\\Services"
        );
    }

    // Feature 1: nullsafe ?-> completions
    #[test]
    fn nullsafe_arrow_triggers_member_completions() {
        let src = "<?php\nclass Service { public function run() {} public string $status; }\n$s = new Service();\n$s?->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 5,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(ls.contains(&"run"), "?-> should complete Service::run()");
        assert!(
            ls.iter().any(|l| l.contains("status")),
            "?-> should complete Service::$status"
        );
    }

    // Feature 5: magic methods in class body
    #[test]
    fn magic_methods_suggested_in_class_body() {
        let src = "<?php\nclass Foo {\n    __\n}";
        let d = doc(src);
        let pos = Position {
            line: 2,
            character: 6,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            None,
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(ls.contains(&"__construct"), "should suggest __construct");
        assert!(ls.contains(&"__toString"), "should suggest __toString");
    }

    #[test]
    fn arrow_trigger_does_not_complete_on_unknown_receiver() {
        // $unknown-> has no type info, so no class members should be returned.
        // The fallback returns methods from the current doc, but since the doc
        // has no class, the result should be empty (no methods available).
        let src = "<?php\n$unknown->";
        let d = doc(src);
        let pos = Position {
            line: 1,
            character: 10,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        // No class is defined in this doc, so the fallback method list is empty.
        assert!(
            items.is_empty(),
            "unknown receiver should yield no completions, got: {:?}",
            labels(&items)
        );
    }

    #[test]
    fn static_trigger_shows_only_static_members() {
        // ClassName:: should only return static methods/constants, NOT instance methods.
        let src = concat!(
            "<?php\n",
            "class MyClass {\n",
            "    public static function staticMethod(): void {}\n",
            "    public function instanceMethod(): void {}\n",
            "    public static int $staticProp = 0;\n",
            "    const MY_CONST = 42;\n",
            "}\n",
            "MyClass::",
        );
        let d = doc(src);
        let pos = Position {
            line: 7,
            character: 9,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(":"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(ls.contains(&"staticMethod"), "should include static method");
        assert!(ls.contains(&"MY_CONST"), "should include constant");
        assert!(
            !ls.contains(&"instanceMethod"),
            "should NOT include instance method in static completion, got: {:?}",
            ls
        );
    }

    // ── Snapshot tests ───────────────────────────────────────────────────────

    use expect_test::expect;

    #[test]
    fn snapshot_keyword_completions_present() {
        // Verify a handful of core PHP keywords appear in the default completion list.
        let items = keyword_completions();
        let mut ls: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        ls.sort_unstable();
        // Snapshot just the first 10 sorted keywords so the test is stable even
        // if new keywords are added later.
        let first_ten = ls[..10.min(ls.len())].join("\n");
        expect![[r#"
            abstract
            and
            array
            as
            break
            callable
            case
            catch
            class
            clone"#]]
        .assert_eq(&first_ten);
    }

    #[test]
    fn snapshot_symbol_completions_for_simple_class() {
        let d = doc(
            "<?php\nclass Counter { public function increment(): void {} public function reset(): void {} }",
        );
        let items = symbol_completions(&d);
        let mut ls: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        ls.sort_unstable();
        expect![[r#"
            Counter
            increment
            reset"#]]
        .assert_eq(&ls.join("\n"));
    }

    #[test]
    fn snapshot_symbol_completions_for_function_with_params() {
        let d = doc("<?php\nfunction connect(string $host, int $port): void {}");
        let items = symbol_completions(&d);
        let mut ls: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        ls.sort_unstable();
        expect![[r#"
            $host
            $port
            connect
            connect(host:, port:)"#]]
        .assert_eq(&ls.join("\n"));
    }

    #[test]
    fn snapshot_arrow_completions_for_typed_var() {
        let src = "<?php\nclass Greeter { public function sayHello(): void {} public function sayBye(): void {} }\n$g = new Greeter();\n$g->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 4,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some(">"),
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let mut ls: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        ls.sort_unstable();
        expect![[r#"
            sayBye
            sayHello"#]]
        .assert_eq(&ls.join("\n"));
    }

    // ── Array destructuring variable suggestions ─────────────────────────────

    #[test]
    fn array_destructuring_short_syntax_produces_variables() {
        // [$a, $b] = someFunction() — both variables should be suggested.
        let d = doc("<?php\n[$first, $second] = getSomething();");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(
            ls.contains(&"$first"),
            "$first from array destructuring should be in completions"
        );
        assert!(
            ls.contains(&"$second"),
            "$second from array destructuring should be in completions"
        );
    }

    #[test]
    fn array_destructuring_variables_have_variable_kind() {
        let d = doc("<?php\n[$x, $y, $z] = getData();");
        let items = symbol_completions(&d);
        for name in &["$x", "$y", "$z"] {
            let item = items.iter().find(|i| i.label.as_str() == *name);
            assert!(item.is_some(), "{name} should be in completions");
            assert_eq!(
                item.unwrap().kind,
                Some(CompletionItemKind::VARIABLE),
                "{name} should have VARIABLE kind"
            );
        }
    }

    #[test]
    fn array_destructuring_respects_cursor_line_scope() {
        // Variables from array destructuring after the cursor line should not appear.
        let src = "<?php\n// cursor here\n[$early] = getA();\n[$late] = getB();";
        let d = doc(src);
        // cursor at line 1 (the comment line)
        let pos = Position {
            line: 1,
            character: 0,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            None,
            &CompletionCtx {
                source: Some(src),
                position: Some(pos),
                ..Default::default()
            },
        );
        let ls = labels(&items);
        assert!(
            !ls.contains(&"$early"),
            "$early declared after cursor should not appear"
        );
        assert!(
            !ls.contains(&"$late"),
            "$late declared after cursor should not appear"
        );
    }

    // ── Include/require path completions ────────────────────────────────────

    #[test]
    fn include_path_prefix_returns_none_for_non_include_line() {
        let src = "<?php\n$x = 'some string';";
        let pos = Position {
            line: 1,
            character: 14,
        };
        assert!(
            include_path_prefix(src, pos).is_none(),
            "should not trigger on non-include line"
        );
    }

    #[test]
    fn include_path_prefix_returns_none_for_absolute_path() {
        let src = "<?php\nrequire '/absolute/path/file.php';";
        let pos = Position {
            line: 1,
            character: 30,
        };
        assert!(
            include_path_prefix(src, pos).is_none(),
            "should not trigger for absolute paths"
        );
    }

    #[test]
    fn include_path_prefix_returns_none_for_stream_wrapper() {
        let src = "<?php\nrequire 'phar://archive.phar/file.php';";
        let pos = Position {
            line: 1,
            character: 35,
        };
        assert!(
            include_path_prefix(src, pos).is_none(),
            "should not trigger for stream wrappers"
        );
    }

    #[test]
    fn include_path_prefix_returns_relative_dot_slash() {
        let src = "<?php\nrequire './lib/Helper";
        let pos = Position {
            line: 1,
            character: 23,
        };
        let result = include_path_prefix(src, pos);
        assert_eq!(
            result.as_deref(),
            Some("./lib/Helper"),
            "should return the typed relative path prefix"
        );
    }

    #[test]
    fn include_path_prefix_returns_double_dot_prefix() {
        let src = "<?php\ninclude '../utils/";
        let pos = Position {
            line: 1,
            character: 22,
        };
        let result = include_path_prefix(src, pos);
        assert_eq!(
            result.as_deref(),
            Some("../utils/"),
            "should return ../utils/ prefix"
        );
    }

    #[test]
    fn include_path_prefix_returns_empty_for_bare_quote() {
        let src = "<?php\nrequire '";
        let pos = Position {
            line: 1,
            character: 10,
        };
        let result = include_path_prefix(src, pos);
        assert_eq!(
            result.as_deref(),
            Some(""),
            "bare quote should return empty prefix (list current dir)"
        );
    }

    #[test]
    fn include_path_completions_lists_relative_directory() {
        use std::fs;

        let tmp = tempfile::tempdir().expect("tmpdir");
        let subdir = tmp.path().join("lib");
        fs::create_dir_all(&subdir).expect("create lib dir");
        fs::write(subdir.join("Helper.php"), "<?php").expect("write Helper.php");
        fs::write(subdir.join("Utils.php"), "<?php").expect("write Utils.php");
        // Non-PHP file that should be excluded
        fs::write(subdir.join("README.md"), "# readme").expect("write README.md");

        let doc_path = tmp.path().join("index.php");
        let doc_uri = Url::from_file_path(&doc_path).expect("doc uri");

        // Prefix "./lib/" — should list the lib directory contents
        let items = include_path_completions(&doc_uri, "./lib/");
        let ls: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(ls.contains(&"Helper.php"), "should list Helper.php");
        assert!(ls.contains(&"Utils.php"), "should list Utils.php");
        assert!(
            !ls.contains(&"README.md"),
            "non-PHP files should be excluded"
        );
    }

    #[test]
    fn include_path_completions_insert_text_includes_directory_prefix() {
        use std::fs;

        let tmp = tempfile::tempdir().expect("tmpdir");
        let subdir = tmp.path().join("src");
        fs::create_dir_all(&subdir).expect("create src dir");
        fs::write(subdir.join("Boot.php"), "<?php").expect("write Boot.php");

        let doc_path = tmp.path().join("main.php");
        let doc_uri = Url::from_file_path(&doc_path).expect("doc uri");

        let items = include_path_completions(&doc_uri, "./src/");
        let boot = items.iter().find(|i| i.label == "Boot.php");
        assert!(boot.is_some(), "Boot.php should be in completions");
        assert_eq!(
            boot.unwrap().insert_text.as_deref(),
            Some("./src/Boot.php"),
            "insert_text should include the directory prefix"
        );
    }

    #[test]
    fn include_path_completions_is_empty_for_non_existent_directory() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let doc_path = tmp.path().join("index.php");
        let doc_uri = Url::from_file_path(&doc_path).expect("doc uri");

        let items = include_path_completions(&doc_uri, "./nonexistent/");
        assert!(
            items.is_empty(),
            "should return empty list for non-existent directory"
        );
    }

    #[test]
    fn include_path_completions_dir_entries_have_folder_kind() {
        use std::fs;

        let tmp = tempfile::tempdir().expect("tmpdir");
        let subdir = tmp.path().join("modules");
        fs::create_dir_all(&subdir).expect("create modules dir");

        let doc_path = tmp.path().join("index.php");
        let doc_uri = Url::from_file_path(&doc_path).expect("doc uri");

        let items = include_path_completions(&doc_uri, "");
        let modules = items.iter().find(|i| i.label == "modules");
        assert!(modules.is_some(), "modules dir should be in completions");
        assert_eq!(
            modules.unwrap().kind,
            Some(CompletionItemKind::FOLDER),
            "directory should have FOLDER kind"
        );
        assert_eq!(
            modules.unwrap().insert_text.as_deref(),
            Some("modules/"),
            "directory insert_text should end with /"
        );
    }
}
