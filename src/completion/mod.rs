mod attribute;
use attribute::attribute_completions;

mod include_path;
use include_path::{include_path_completions, include_path_prefix};

mod keyword;
pub use keyword::{
    keyword_completions, keyword_completions_matching, magic_constant_completions,
    magic_constant_completions_matching,
};

mod match_arm;
use match_arm::match_arm_completions;

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

mod symbols;
pub use symbols::{
    builtin_completions, builtin_completions_matching, superglobal_completions,
    superglobal_completions_matching, symbol_completions, symbol_completions_before,
};

use std::sync::Arc;

use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, InsertTextFormat, Position, Range, TextEdit, Uri,
};

use tower_lsp_server::ls_types::{Documentation, MarkupContent, MarkupKind};

use crate::document::ast::{ParsedDoc, format_type_hint};
use crate::hover::format_params_str;
use crate::lang::docblock::parse_docblock;
use crate::text::{camel_sort_key, utf16_offset_to_byte};
use crate::types::type_map::{enclosing_class_at, params_of_function, params_of_method};
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

/// Build a `Documentation` value for a caller that already has the declaration's
/// own `doc_comment` node in hand (e.g. while walking every symbol in a file
/// for completion) — skips `find_docblock`'s name-based tree re-search,
/// which is O(symbols-in-file) per call and made whole-file completion
/// O(n²) in the symbol count.
pub(super) fn documentation_from_comment(
    comment: Option<&php_ast::Comment<'_>>,
) -> Option<Documentation> {
    let md = parse_docblock(comment?.text).to_markdown();
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
    ctx: &CompletionCtx<'_>,
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

    // A receiver (`$obj->method(` / `Class::method(`) scopes the lookup to
    // that specific class. Without this, a bare name-only scan returns
    // whichever same-named method it finds first in the workspace — which
    // can belong to a completely unrelated class (e.g. `Logger::send` vs
    // `Mailer::send`) and offer that class's parameter names instead.
    let before_receiver = &before[..before.len() - func_name.len()];
    let receiver_col = crate::text::byte_to_utf16(line, before_receiver.len());
    let receiver_pos = Position {
        line: position.line,
        character: receiver_col,
    };
    if before_receiver.ends_with("->") || before_receiver.ends_with("?->") {
        return resolve_receiver_class(source, doc, receiver_pos, ctx.analysis)
            .map(|class_name| {
                params_of_method_anywhere(&class_name, &func_name, doc, other_docs, ctx)
            })
            .unwrap_or_default();
    }
    if before_receiver.ends_with("::") {
        let empty_imports = HashMap::new();
        let imports = ctx.file_imports.unwrap_or(&empty_imports);
        return resolve_static_receiver(source, doc, other_docs, receiver_pos, imports)
            .map(|class_name| {
                params_of_method_anywhere(&class_name, &func_name, doc, other_docs, ctx)
            })
            .unwrap_or_default();
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

/// Find `method_name`'s parameter names on exactly `class_name` — the
/// current doc, then the workspace-index-backed lookup, then every other
/// open doc. Deliberately does *not* fall back further (e.g. to an
/// unrelated same-named method, or to a superclass): a resolved receiver
/// class that doesn't declare the method directly should offer no
/// named-argument completions rather than risk a wrong class's parameters.
fn params_of_method_anywhere(
    class_name: &str,
    method_name: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    ctx: &CompletionCtx<'_>,
) -> Vec<String> {
    let params = params_of_method(doc, class_name, method_name);
    if !params.is_empty() {
        return params;
    }
    if let Some(find) = ctx.find_class_doc
        && let Some(class_doc) = find(class_name)
    {
        let params = params_of_method(&class_doc, class_name, method_name);
        if !params.is_empty() {
            return params;
        }
    }
    for other in other_docs {
        let params = params_of_method(other, class_name, method_name);
        if !params.is_empty() {
            return params;
        }
    }
    vec![]
}

/// Workspace-index-backed class lookup: maps a short class name to the
/// `ParsedDoc` that defines it. Used by `all_instance_members` and
/// `all_static_members` to avoid scanning all workspace docs linearly.
pub type ClassDocLookup<'a> = &'a dyn Fn(&str) -> Option<Arc<ParsedDoc>>;

/// Workspace-index-backed class name search: given a typed prefix, returns
/// `(short_name, kind, fqn)` for every workspace class whose short name
/// starts with it — including classes defined in files that are not open
/// in the editor (e.g. `vendor/`). Complements `other_docs`, which only
/// covers currently open documents.
pub type WorkspaceClassSearch<'a> = &'a dyn Fn(&str) -> Vec<(String, CompletionItemKind, String)>;

/// Optional context for completion requests that enables richer results
/// (e.g. auto-import edits, `->` scoping to a class).
#[derive(Default)]
pub struct CompletionCtx<'a> {
    pub source: Option<&'a str>,
    pub position: Option<Position>,
    pub doc_uri: Option<&'a Uri>,
    pub file_imports: Option<&'a HashMap<String, String>>,
    /// Optional O(1) class-document lookup backed by the workspace index.
    /// When `Some`, `all_instance_members` and `all_static_members` use it
    /// to find the defining doc directly instead of scanning `other_docs`
    /// linearly (O(n files × inheritance depth) → O(depth)).
    /// Pass `None` to fall back to the existing linear scan.
    pub find_class_doc: Option<ClassDocLookup<'a>>,
    /// Optional workspace-wide class name search backed by the workspace
    /// index (see [`WorkspaceClassSearch`]). Used to suggest — and
    /// auto-import — classes from files that aren't currently open, such as
    /// vendor code. `None` in unit tests that don't supply it.
    pub workspace_class_search: Option<WorkspaceClassSearch<'a>>,
    /// Retained mir body analysis for the primary doc. Receiver-variable types
    /// (`$obj->`, match subjects) are read from its `symbol_at`; `None` in unit
    /// tests that don't supply it.
    pub analysis: Option<&'a mir_analyzer::FileAnalysis>,
    /// mir-analyzer session for querying phpstorm-stubs member info on
    /// built-in PHP classes. `None` in unit tests that don't require stubs.
    pub session: Option<std::sync::Arc<mir_analyzer::AnalysisSession>>,
    /// Laravel string-key index (`env`/`config`/`view`/... — see
    /// `crate::laravel`), for completion inside those helper calls' string
    /// arguments. `None` in unit tests that don't supply it, and inert
    /// (empty) for non-Laravel workspaces.
    pub laravel: Option<&'a crate::laravel::LaravelIndex>,
}

/// Returns `true` when `cursor_byte` falls inside a PHP string literal or
/// comment. Scans the source from the beginning with a simple state machine;
/// handles single/double-quoted strings (with backslash escapes), `// …` and
/// `# …` line comments, and `/* … */` block comments. Heredoc/nowdoc are not
/// tracked — they are too rare in interactive editing contexts to warrant the
/// complexity, and missing them produces a false-negative (completions shown
/// inside a heredoc), not a false-positive (completions suppressed outside one).
pub(crate) fn cursor_in_string_or_comment(source: &str, cursor_byte: usize) -> bool {
    #[derive(PartialEq)]
    enum S {
        Normal,
        Single,
        Double,
        Line,
        Block,
    }
    let bytes = source.as_bytes();
    let limit = bytes.len().min(cursor_byte);
    let mut i = 0usize;
    let mut state = S::Normal;
    while i < limit {
        match state {
            S::Normal => match bytes[i] {
                b'\'' => {
                    state = S::Single;
                    i += 1;
                }
                b'"' => {
                    state = S::Double;
                    i += 1;
                }
                b'/' if i + 1 < limit && bytes[i + 1] == b'/' => {
                    state = S::Line;
                    i += 2;
                }
                // `#[` is a PHP 8 attribute — not a comment.
                b'#' if !(i + 1 < limit && bytes[i + 1] == b'[') => {
                    state = S::Line;
                    i += 1;
                }
                b'/' if i + 1 < limit && bytes[i + 1] == b'*' => {
                    state = S::Block;
                    i += 2;
                }
                _ => {
                    i += 1;
                }
            },
            S::Single => match bytes[i] {
                b'\\' => {
                    i += 2;
                }
                b'\'' => {
                    state = S::Normal;
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            },
            S::Double => match bytes[i] {
                b'\\' => {
                    i += 2;
                }
                b'"' => {
                    state = S::Normal;
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            },
            S::Line => {
                if bytes[i] == b'\n' {
                    state = S::Normal;
                }
                i += 1;
            }
            S::Block => {
                if bytes[i] == b'*' && i + 1 < limit && bytes[i + 1] == b'/' {
                    state = S::Normal;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        }
    }
    state != S::Normal
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

    let doc_uri = ctx.doc_uri;

    // `completions_for_string_key` checks `is_laravel` first (one bool read
    // on an already-loaded `Arc`) so non-Laravel workspaces never pay for the
    // line scan inside it. Computed once and reused by both the
    // string-suppression guard and Feature 10.
    let laravel_completions = source
        .zip(position)
        .and_then(|(src, pos)| crate::laravel::completions_for_string_key(src, pos, ctx.laravel));

    // Blade-specific completions (component/Livewire tags, view/Livewire
    // directive arguments) — bare helper calls inside `{{ }}` are already
    // covered by `laravel_completions` above, a pure text scan that doesn't
    // care whether it's running inside a Blade expression or plain PHP.
    let blade_completions = doc_uri
        .zip(source)
        .zip(position)
        .and_then(|((uri, src), pos)| {
            crate::laravel::blade::completions(uri, src, pos, ctx.laravel)
        });

    // Request-field completion inside `$request->input('...')`/`get`/`post`/
    // `query` — a naming-convention heuristic (see `laravel::request_fields`
    // module docs), gated the same way every other Laravel feature is.
    let request_field_completions = source.zip(position).and_then(|(src, pos)| {
        if !ctx.laravel.is_some_and(|l| l.is_laravel) {
            return None;
        }
        let (receiver, prefix) =
            crate::laravel::request_fields::method_call_string_prefix(src, pos)?;
        let fields =
            crate::laravel::request_fields::harvest_fields(&doc.program().stmts, &receiver);
        Some(
            fields
                .into_iter()
                .filter(|f| f.starts_with(&prefix))
                .map(|f| CompletionItem {
                    label: f.clone(),
                    kind: Some(CompletionItemKind::FIELD),
                    insert_text: Some(f),
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
        )
    });

    // Suppress all completions when the cursor is inside a string literal or
    // comment — except for include/require path strings and Laravel
    // string-key/request-field calls, where completions are legitimate
    // inside the string argument.
    if let (Some(src), Some(pos)) = (source, position) {
        let cursor_byte = doc.view().byte_of_position(pos) as usize;
        if cursor_in_string_or_comment(src, cursor_byte)
            && include_path_prefix(src, pos).is_none()
            && laravel_completions.is_none()
            && blade_completions.is_none()
            && request_field_completions.is_none()
        {
            return vec![];
        }
    }
    let empty_imports = HashMap::new();
    let imports = ctx.file_imports.unwrap_or(&empty_imports);

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
            if let (Some(src), Some(pos)) = (source, position)
                && let Some(class_names) = resolve_receiver_class(src, doc, pos, ctx.analysis)
            {
                // Feature 5: support union types (Foo|Bar)
                let mut items = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for class_name in class_names.split('|') {
                    let class_name = class_name.trim();
                    for item in all_instance_members(
                        class_name,
                        doc,
                        other_docs,
                        ctx.find_class_doc,
                        ctx.session.as_deref(),
                    ) {
                        if seen.insert(item.label.clone()) {
                            items.push(item);
                        }
                    }
                }
                if !items.is_empty() {
                    return items;
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
                && let Some(class_name) =
                    resolve_static_receiver(src, doc, other_docs, pos, imports)
            {
                let items = all_static_members(
                    &class_name,
                    doc,
                    other_docs,
                    ctx.find_class_doc,
                    ctx.session.as_deref(),
                );
                if !items.is_empty() {
                    return items;
                }
            }
            vec![]
        }
        Some("[") => {
            // PHP attribute: #[ — suggest only #[\Attribute]-annotated classes.
            if let (Some(src), Some(pos)) = (source, position) {
                let line = src.lines().nth(pos.line as usize).unwrap_or("");
                let col = utf16_offset_to_byte(line, pos.character as usize);
                let before = &line[..col];
                if before.trim_end_matches('[').trim_end().ends_with('#') {
                    return attribute_completions(src, pos, doc, other_docs, imports);
                }
            }
            vec![]
        }
        Some("(") => {
            // Named argument: funcName(
            if let (Some(src), Some(pos)) = (source, position) {
                let params = resolve_call_params(src, doc, other_docs, pos, ctx);
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
            // Static access context: ClassName::member (invoked without trigger char).
            // Strip any identifier chars being typed as the member prefix.
            if let (Some(src), Some(pos)) = (source, position) {
                let line = src.lines().nth(pos.line as usize).unwrap_or("");
                let col = utf16_offset_to_byte(line, pos.character as usize);
                let before = &line[..col];
                let pre_colon = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
                if pre_colon.ends_with("::") {
                    let colon_end_char = pre_colon.encode_utf16().count() as u32;
                    let colon_pos = tower_lsp_server::ls_types::Position {
                        line: pos.line,
                        character: colon_end_char,
                    };
                    if let Some(class_name) =
                        resolve_static_receiver(src, doc, other_docs, colon_pos, imports)
                    {
                        let items = all_static_members(
                            &class_name,
                            doc,
                            other_docs,
                            ctx.find_class_doc,
                            ctx.session.as_deref(),
                        );
                        if !items.is_empty() {
                            return items;
                        }
                    }
                }
            }

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
                    // Synthesise a cursor that sits right at the end of the arrow so
                    // that `resolve_receiver_class` — which strips the trailing `->` /
                    // `?->` itself — can locate the receiver.  This correctly handles
                    // simple variables ($obj->), `(new Foo())->`, method chains
                    // ($obj->getUser()->), and nullable operators ($obj?->).
                    let arrow_end_char = pre_arrow.encode_utf16().count() as u32;
                    let arrow_pos = tower_lsp_server::ls_types::Position {
                        line: pos.line,
                        character: arrow_end_char,
                    };
                    if let Some(cls) = resolve_receiver_class(src, doc, arrow_pos, ctx.analysis) {
                        let mut items = Vec::new();
                        let mut seen = std::collections::HashSet::new();
                        for class_name in cls.split('|') {
                            for item in all_instance_members(
                                class_name.trim(),
                                doc,
                                other_docs,
                                ctx.find_class_doc,
                                ctx.session.as_deref(),
                            ) {
                                if seen.insert(item.label.clone()) {
                                    items.push(item);
                                }
                            }
                        }
                        if !items.is_empty() {
                            // Apply fuzzy filtering based on the typed prefix.
                            let prefix = before.strip_prefix(pre_arrow).unwrap_or("").to_string();
                            if !prefix.is_empty() {
                                let fq = crate::text::FuzzyQuery::new(&prefix);
                                items.retain(|i| {
                                    let match_against = if i.label.starts_with('$') {
                                        i.label.strip_prefix('$').unwrap_or(&i.label)
                                    } else {
                                        &i.label
                                    };
                                    fq.camel_match(match_against)
                                });
                                for item in &mut items {
                                    let match_against = if item.label.starts_with('$') {
                                        item.label.strip_prefix('$').unwrap_or(&item.label)
                                    } else {
                                        &item.label
                                    };
                                    item.sort_text =
                                        Some(crate::text::camel_sort_key(&prefix, match_against));
                                    item.filter_text = Some(item.label.clone());
                                }
                            }
                            return items;
                        }
                    }
                }
            }

            // Attribute context: #[ or #[PartialName — invoked without trigger char.
            if let (Some(src), Some(pos)) = (source, position) {
                let line = src.lines().nth(pos.line as usize).unwrap_or("");
                let col = utf16_offset_to_byte(line, pos.character as usize);
                let before = &line[..col];
                let pre_ident =
                    before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '\\');
                if pre_ident.trim_end().ends_with("#[") || pre_ident.trim_end() == "#[" {
                    let items = attribute_completions(src, pos, doc, other_docs, imports);
                    if !items.is_empty() {
                        return items;
                    }
                }
            }

            // Feature 4: detect `use `/`use function `/`use const ` context and
            // suggest FQNs from other docs, scoped to the right symbol kind.
            if let (Some(src), Some(pos)) = (source, position)
                && let Some((use_kind, use_prefix)) = use_completion_prefix(src, pos)
            {
                let mut use_items: Vec<CompletionItem> = Vec::new();
                for other in other_docs {
                    collect_fqns_with_prefix(
                        &other.program().stmts,
                        "",
                        &use_prefix,
                        use_kind,
                        &mut use_items,
                    );
                }
                // Also check current doc
                collect_fqns_with_prefix(
                    &doc.program().stmts,
                    "",
                    &use_prefix,
                    use_kind,
                    &mut use_items,
                );
                if !use_items.is_empty() {
                    return use_items;
                }
            }

            // Blade directive/tag completions take priority over Feature 9
            // below: `@include('` textually ends in `include(` just like a
            // real PHP `include(...)` statement, and `blade_completions` is
            // always `None` outside `.blade.php` files, so this can't affect
            // plain-PHP `include`/`require` path completions.
            if let Some(items) = blade_completions {
                return items;
            }

            // Feature 9: include/require path completions
            if let (Some(src), Some(pos), Some(uri)) = (source, position, doc_uri)
                && let Some(prefix) = include_path_prefix(src, pos)
            {
                // When in include/require context, return path completions (even if empty)
                // instead of falling back to keywords/symbols
                let items = include_path_completions(uri, &prefix);
                return items;
            }

            // Feature 10: Laravel string-key completions (env/config/...)
            if let Some(items) = laravel_completions {
                return items;
            }
            if let Some(items) = request_field_completions {
                return items;
            }

            // Classes (label, kind, FQN) per other doc, collected lazily once
            // per request: the sub-namespace branch below falls through to the
            // default cross-file loop when nothing matches, which previously
            // re-collected the same lists from every doc's AST a second time.
            let other_classes_cell: std::cell::OnceCell<
                Vec<Vec<(String, CompletionItemKind, String)>>,
            > = std::cell::OnceCell::new();
            let other_classes = || {
                other_classes_cell.get_or_init(|| {
                    other_docs
                        .iter()
                        .map(|other| {
                            let mut classes = Vec::new();
                            collect_classes_with_ns(&other.program().stmts, "", &mut classes);
                            classes
                        })
                        .collect()
                })
            };

            // Feature 3: Sub-namespace \ completions outside use statement
            if let (Some(src), Some(pos)) = (source, position)
                && let Some(prefix) = typed_prefix(Some(src), Some(pos))
                && prefix.contains('\\')
            {
                // Check we're NOT in a use statement
                let is_use = use_completion_prefix(src, pos).is_some();
                if !is_use {
                    let prefix_lc = prefix.trim_start_matches('\\').to_lowercase();
                    let mut ns_items: Vec<CompletionItem> = Vec::new();
                    for classes in other_classes() {
                        for (label, kind, fqn) in classes {
                            if fqn
                                .get(..prefix_lc.len())
                                .is_some_and(|s| s.eq_ignore_ascii_case(&prefix_lc))
                            {
                                ns_items.push(CompletionItem {
                                    label: label.clone(),
                                    kind: Some(*kind),
                                    insert_text: Some(label.clone()),
                                    detail: Some(fqn.clone()),
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
                    other_docs,
                    pos,
                    ctx.analysis,
                    ctx.find_class_doc,
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

                // Deduplicate by label (first occurrence wins)
                let mut seen = std::collections::HashSet::new();
                all.retain(|i| seen.insert(i.label.clone()));

                return all;
            }

            // Feature 5: Magic method completions in class body
            let mut magic_items: Vec<CompletionItem> = Vec::new();
            if let (Some(src), Some(pos)) = (source, position)
                && enclosing_class_at(src, doc, pos).is_some()
            {
                magic_items.extend(magic_method_completions());
            }

            // Extract the typed prefix early: it bounds the workspace-wide
            // class search below, and — for a plain identifier prefix — the
            // static item sources apply the same camel-match the final retain
            // would, so hundreds of keyword/builtin items aren't materialized
            // per keystroke just to be dropped.
            let prefix = typed_prefix(source, position).unwrap_or_default();
            let ident_query = (!prefix.is_empty() && !prefix.contains('\\'))
                .then(|| crate::text::FuzzyQuery::new(&prefix));
            let keep_static =
                |label: &str| ident_query.as_ref().is_none_or(|fq| fq.camel_match(label));

            let mut items = keyword_completions_matching(&keep_static);
            items.extend(magic_constant_completions_matching(&keep_static));
            items.extend(builtin_completions_matching(&keep_static));
            items.extend(superglobal_completions_matching(&keep_static));
            // Feature 2: scope variable completions to before cursor line
            let sym_items = if let (Some(_src), Some(pos)) = (source, position) {
                symbol_completions_before(doc, pos.line)
            } else {
                symbol_completions(doc)
            };
            items.extend(sym_items);
            items.extend(magic_items);

            let cur_ns = current_file_namespace(&doc.program().stmts);

            // Same for every class candidate in this request (depends only on
            // `source`) — computed once instead of per candidate, since a single
            // request may build up to dozens of class items (open-doc classes
            // plus up to `WORKSPACE_CLASS_SEARCH_LIMIT` workspace-index matches).
            let use_pos = source.map(use_insert_position);

            // Builds a class completion item, adding a `use` insertion edit
            // unless the class is already reachable (same namespace, global,
            // or already imported).
            let push_class_item = |items: &mut Vec<CompletionItem>,
                                   label: &str,
                                   kind: CompletionItemKind,
                                   fqn: &str| {
                let additional_text_edits = if let Some(pos) = use_pos {
                    let in_same_ns = !cur_ns.is_empty() && fqn == format!("{}\\{}", cur_ns, label);
                    let is_global = !fqn.contains('\\');
                    let already = imports.contains_key(label);
                    if !in_same_ns && !is_global && !already {
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
                    label: label.to_string(),
                    kind: Some(kind),
                    detail: if fqn.contains('\\') {
                        Some(fqn.to_string())
                    } else {
                        None
                    },
                    additional_text_edits,
                    ..Default::default()
                });
            };

            for (other, classes) in other_docs.iter().zip(other_classes()) {
                // Class-like symbols: add `use` insertion when needed.
                for (label, kind, fqn) in classes {
                    push_class_item(&mut items, label, *kind, fqn);
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

            // Classes from files that aren't open in the editor (e.g.
            // vendor/), found via the workspace index. Only fires once a
            // prefix is typed, both to bound the search and because an
            // empty-prefix workspace-wide dump would swamp the list.
            if !prefix.is_empty()
                && !prefix.contains('\\')
                && let Some(search) = ctx.workspace_class_search
            {
                for (label, kind, fqn) in search(&prefix) {
                    push_class_item(&mut items, &label, kind, &fqn);
                }
            }

            let mut seen = std::collections::HashSet::new();
            items.retain(|i| seen.insert(i.label.clone()));

            if prefix.contains('\\') {
                // Namespace-qualified prefix: filter by FQN prefix match.
                let ns_prefix = prefix.trim_start_matches('\\').to_lowercase();
                items.retain(|i| {
                    let fqn = i.detail.as_deref().unwrap_or(&i.label);
                    fqn.get(..ns_prefix.len())
                        .is_some_and(|s| s.eq_ignore_ascii_case(&ns_prefix))
                });
            } else if !prefix.is_empty() {
                let fq = crate::text::FuzzyQuery::new(&prefix);
                items.retain(|i| fq.camel_match(&i.label));
                for item in &mut items {
                    item.sort_text = Some(camel_sort_key(&prefix, &item.label));
                    item.filter_text = Some(item.label.clone());
                }
            }
            items
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn type_keywords_present() {
        let kws = keyword_completions();
        let ls = labels(&kws);
        for expected in &[
            "bool", "float", "int", "iterable", "mixed", "never", "object", "string", "void",
            "parent",
        ] {
            assert!(ls.contains(expected), "missing type keyword: {expected}");
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
}
