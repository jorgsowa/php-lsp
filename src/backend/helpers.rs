use std::sync::Arc;

use tower_lsp::Client;
use tower_lsp::lsp_types::*;

use php_ast::{
    ClassMember, ClassMemberKind, EnumMember, EnumMemberKind, ExprKind, NamespaceBody, Stmt,
    StmtKind,
};

use crate::ast::{ParsedDoc, str_offset};
use crate::navigation::definition::find_declaration_range;
use crate::navigation::references::SymbolKind;

use crate::actions::generate_action::{
    generate_constructor_actions, generate_getters_setters_actions,
};
use crate::actions::implement_action::implement_missing_actions;
use crate::actions::phpdoc_action::phpdoc_actions;
use crate::actions::promote_action::promote_constructor_actions;
use crate::actions::type_action::add_return_type_actions;

use super::Backend;

pub(super) fn php_file_op() -> FileOperationRegistrationOptions {
    FileOperationRegistrationOptions {
        filters: vec![FileOperationFilter {
            scheme: Some("file".to_string()),
            pattern: FileOperationPattern {
                glob: "**/*.php".to_string(),
                matches: Some(FileOperationPatternKind::File),
                options: None,
            },
        }],
    }
}

/// Strip the `edit` from each `CodeAction` and attach a `data` payload so the
/// client can request the edit lazily via `codeAction/resolve`.
pub(super) fn defer_actions(
    actions: Vec<CodeActionOrCommand>,
    kind_tag: &str,
    uri: &Url,
    range: Range,
) -> Vec<CodeActionOrCommand> {
    actions
        .into_iter()
        .map(|a| match a {
            CodeActionOrCommand::CodeAction(mut ca) => {
                ca.edit = None;
                ca.data = Some(serde_json::json!({
                    "php_lsp_resolve": kind_tag,
                    "uri": uri.to_string(),
                    "range": range,
                }));
                CodeActionOrCommand::CodeAction(ca)
            }
            other => other,
        })
        .collect()
}

/// Returns `true` when the identifier at `position` is immediately preceded by `->`,
/// indicating it is a property or method name in an instance access expression.
pub(super) fn is_after_arrow(source: &str, position: Position) -> bool {
    let line = match source.lines().nth(position.line as usize) {
        Some(l) => l,
        None => return false,
    };
    let chars: Vec<char> = line.chars().collect();
    let col = position.character as usize;
    // Find the char index of the cursor (UTF-16 → char index).
    let mut utf16_col = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16_col >= col {
            break;
        }
        utf16_col += ch.len_utf16();
        char_idx += 1;
    }
    // Walk left past word chars to the start of the identifier.
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    while char_idx > 0 && is_word(chars[char_idx - 1]) {
        char_idx -= 1;
    }
    char_idx >= 2 && chars[char_idx - 1] == '>' && chars[char_idx - 2] == '-'
}

/// Classify the symbol at `position` so `find_references` can use the right walker.
///
/// Heuristics (in priority order):
/// 1. Preceded by `->` or `?->` → `Method`
/// 2. Preceded by `::` → `Method` (static)
/// 3. Word starts with `$` → variable (returns `None`; variables are handled separately)
/// 4. First character is uppercase AND not preceded by `->` or `::` → `Class`
/// 5. Otherwise → `Function`
///
/// Falls back to `None` when the context cannot be determined.
pub(super) fn symbol_kind_at(source: &str, position: Position, word: &str) -> Option<SymbolKind> {
    if word.starts_with('$') {
        return None; // variables handled elsewhere
    }
    let line = source.lines().nth(position.line as usize)?;
    let chars: Vec<char> = line.chars().collect();

    // Convert UTF-16 column to char index.
    let col = position.character as usize;
    let mut utf16_col = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16_col >= col {
            break;
        }
        utf16_col += ch.len_utf16();
        char_idx += 1;
    }

    // Walk left past identifier characters to find the first character before the word.
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    while char_idx > 0 && is_word_char(chars[char_idx - 1]) {
        char_idx -= 1;
    }

    // Look past the end of the word to distinguish `->method()` from `->prop`.
    let word_end = {
        let mut i = char_idx;
        while i < chars.len() && is_word_char(chars[i]) {
            i += 1;
        }
        // Skip spaces before the next token.
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        i
    };
    let next_is_call = word_end < chars.len() && chars[word_end] == '(';

    // Check for `->` or `?->`
    if char_idx >= 2 && chars[char_idx - 1] == '>' && chars[char_idx - 2] == '-' {
        return if next_is_call {
            Some(SymbolKind::Method)
        } else {
            Some(SymbolKind::Property)
        };
    }
    if char_idx >= 3
        && chars[char_idx - 1] == '>'
        && chars[char_idx - 2] == '-'
        && chars[char_idx - 3] == '?'
    {
        return if next_is_call {
            Some(SymbolKind::Method)
        } else {
            Some(SymbolKind::Property)
        };
    }

    // Check for `::`
    if char_idx >= 2 && chars[char_idx - 1] == ':' && chars[char_idx - 2] == ':' {
        return Some(SymbolKind::Method);
    }

    // If the word starts with an uppercase letter it is likely a class/interface/enum name.
    if word
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        return Some(SymbolKind::Class);
    }

    // Otherwise treat as a free function.
    Some(SymbolKind::Function)
}

/// Convert an LSP `Position` to a byte offset within `source`.
/// Returns `None` if the position is beyond the end of the source.
/// Returns `true` when `inner` is fully contained inside `outer` (the LSP
/// half-open `[start, end)` convention is irrelevant here — a range with
/// the exact same bounds counts as contained).
pub(super) fn range_within(inner: Range, outer: Range) -> bool {
    let start_ok =
        (inner.start.line, inner.start.character) >= (outer.start.line, outer.start.character);
    let end_ok = (inner.end.line, inner.end.character) <= (outer.end.line, outer.end.character);
    start_ok && end_ok
}

pub(super) fn position_to_byte_offset(source: &str, position: Position) -> Option<u32> {
    let mut byte_offset = 0usize;
    for (idx, line) in source.split('\n').enumerate() {
        if idx as u32 == position.line {
            // Strip trailing \r so CRLF lines don't affect column counting.
            let line_content = line.trim_end_matches('\r');
            let mut col = 0u32;
            for (byte_idx, ch) in line_content.char_indices() {
                if col >= position.character {
                    return Some((byte_offset + byte_idx) as u32);
                }
                col += ch.len_utf16() as u32;
            }
            return Some((byte_offset + line_content.len()) as u32);
        }
        byte_offset += line.len() + 1; // +1 for the '\n'
    }
    None
}

/// Returns `true` if the cursor is positioned on a method name inside a class,
/// interface, trait, or enum declaration in the AST.
///
/// This is a pre-pass used before the character-based `symbol_kind_at` heuristic
/// so that method *declarations* (`public function add() {}`) are classified as
/// `SymbolKind::Method` rather than falling through to `SymbolKind::Function`.
pub(super) fn cursor_is_on_method_decl(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    position: Position,
) -> bool {
    let Some(cursor) = position_to_byte_offset(source, position) else {
        return false;
    };

    // Locate `name` within `member_span` rather than searching the whole
    // source — the global `str_offset` returns the first occurrence in the
    // file, which causes a method named `status` to also match a property
    // named `$status` (cursor on the `$status` declaration falsely tests
    // positive for "on method decl").
    fn name_offset_in_member(source: &str, member_span: php_ast::Span, name: &str) -> Option<u32> {
        let s = member_span.start as usize;
        let e = (member_span.end as usize).min(source.len());
        source
            .get(s..e)?
            .find(name)
            .map(|off| member_span.start + off as u32)
    }
    fn check(source: &str, stmts: &[Stmt<'_, '_>], cursor: u32) -> bool {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Class(c) => {
                    for member in c.body.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let name = m.name.to_string();
                            let start =
                                name_offset_in_member(source, member.span, &name).unwrap_or(0);
                            let end = start + name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Interface(i) => {
                    for member in i.body.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let name = m.name.to_string();
                            let start =
                                name_offset_in_member(source, member.span, &name).unwrap_or(0);
                            let end = start + name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Trait(t) => {
                    for member in t.body.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let name = m.name.to_string();
                            let start =
                                name_offset_in_member(source, member.span, &name).unwrap_or(0);
                            let end = start + name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Enum(e) => {
                    for member in e.body.members.iter() {
                        if let EnumMemberKind::Method(m) = &member.kind {
                            let name = m.name.to_string();
                            let start =
                                name_offset_in_member(source, member.span, &name).unwrap_or(0);
                            let end = start + name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Namespace(ns) => {
                    if let NamespaceBody::Braced(inner) = &ns.body
                        && check(source, &inner.stmts, cursor)
                    {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    check(source, stmts, cursor)
}

/// If the cursor is on a class or trait property *declaration* name (e.g.
/// `public string $status`), return the property name without the leading `$`
/// so the caller can search for `status` via `SymbolKind::Property`.  Returns
/// `None` when the cursor is elsewhere.
pub(super) fn cursor_is_on_property_decl(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    position: Position,
) -> Option<String> {
    let cursor = position_to_byte_offset(source, position)?;

    fn name_offset_in_member(source: &str, member_span: php_ast::Span, name: &str) -> Option<u32> {
        let s = member_span.start as usize;
        let e = (member_span.end as usize).min(source.len());
        source
            .get(s..e)?
            .find(name)
            .map(|off| member_span.start + off as u32)
    }
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
pub(super) fn cursor_is_on_constant_decl(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    position: Position,
) -> Option<(String, Option<String>)> {
    let cursor = position_to_byte_offset(source, position)?;

    fn name_offset_in_member(source: &str, member_span: php_ast::Span, name: &str) -> Option<u32> {
        let s = member_span.start as usize;
        let e = (member_span.end as usize).min(source.len());
        source
            .get(s..e)?
            .find(name)
            .map(|off| member_span.start + off as u32)
    }

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
            if let EnumMemberKind::ClassConst(c) = &member.kind {
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
                        && let ExprKind::String(s) = &first_arg.value.kind
                    {
                        // String content starts one byte after the opening quote.
                        let start = first_arg.value.span.start + 1;
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

/// When the cursor sits on a `__construct` method name declaration, return
/// the owning class FQN (namespace-qualified when inside a namespace). Returns
/// `None` otherwise (including when the cursor is on a non-constructor method,
/// inside a trait/interface, or inside a namespaced enum — constructors on
/// those don't drive class instantiation call sites the way class constructors
/// do).
pub(super) fn class_name_at_construct_decl(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    position: Position,
) -> Option<String> {
    let cursor = position_to_byte_offset(source, position)?;

    fn name_offset_in_member(source: &str, member_span: php_ast::Span, name: &str) -> Option<u32> {
        let s = member_span.start as usize;
        let e = (member_span.end as usize).min(source.len());
        source
            .get(s..e)?
            .find(name)
            .map(|off| member_span.start + off as u32)
    }
    fn check(source: &str, stmts: &[Stmt<'_, '_>], cursor: u32, ns_prefix: &str) -> Option<String> {
        let mut current_ns = ns_prefix.to_owned();
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Class(c) => {
                    for member in c.body.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == "__construct"
                        {
                            // Scope the name search to this member's own span:
                            // a global `str_offset` returns the FIRST
                            // `__construct` in the file, so when two classes
                            // both define `__construct` every cursor lands on
                            // the first one regardless of which class the
                            // cursor is actually inside.
                            let name = m.name.to_string();
                            let start =
                                name_offset_in_member(source, member.span, &name).unwrap_or(0);
                            let end = start + name.len() as u32;
                            if cursor >= start && cursor < end {
                                let short = c.name?;
                                return Some(if current_ns.is_empty() {
                                    short.to_string()
                                } else {
                                    format!("{}\\{}", current_ns, short)
                                });
                            }
                        }
                    }
                }
                StmtKind::Namespace(ns) => {
                    let ns_name = ns
                        .name
                        .as_ref()
                        .map(|n| n.to_string_repr().to_string())
                        .unwrap_or_default();
                    match &ns.body {
                        NamespaceBody::Braced(inner) => {
                            if let Some(name) = check(source, &inner.stmts, cursor, &ns_name) {
                                return Some(name);
                            }
                        }
                        NamespaceBody::Simple => {
                            current_ns = ns_name;
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    check(source, stmts, cursor, "")
}

/// If the cursor sits on a promoted constructor property parameter (one that
/// has a visibility modifier like `public`/`protected`/`private`), return the
/// property name without the leading `$` so the caller can search for
/// `->name` property accesses (`SymbolKind::Property`).
///
/// Returns `None` for regular (non-promoted) params and for any cursor position
/// not on a constructor param name.
pub(super) fn promoted_property_at_cursor(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    position: Position,
) -> Option<String> {
    let cursor = position_to_byte_offset(source, position)?;

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
                                let name_start =
                                    str_offset(source, &param.name.to_string()).unwrap_or(0);
                                let name_end = name_start + param.name.to_string().len() as u32;
                                if cursor >= name_start && cursor < name_end {
                                    return Some(
                                        param.name.to_string().trim_start_matches('$').to_string(),
                                    );
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

/// Tags for deferred code actions (resolved lazily via `codeAction/resolve`).
/// Iteration order controls the order items appear in the client menu.
pub(super) const DEFERRED_ACTION_TAGS: &[&str] = &[
    "phpdoc",
    "implement",
    "constructor",
    "getters_setters",
    "return_type",
    "promote",
];

impl Backend {
    /// Run [`crate::document_store::DocumentStore::cached_analysis`] without
    /// blocking the async executor. The warm path (cache entry current for the
    /// file's text) resolves synchronously; the cold path — mir Pass 1 + Pass 2,
    /// which can take hundreds of ms on large files and is hit after every
    /// keystroke because edits clear the analysis cache — runs on the blocking
    /// pool so it doesn't stall other in-flight requests.
    pub(super) async fn cached_analysis_async(
        &self,
        uri: &Url,
    ) -> Option<Arc<mir_analyzer::FileAnalysis>> {
        if let Some(hit) = self.docs.cached_analysis_if_fresh(uri) {
            return Some(hit);
        }
        let docs = Arc::clone(&self.docs);
        let uri = uri.clone();
        tokio::task::spawn_blocking(move || docs.cached_analysis(&uri))
            .await
            .unwrap_or(None)
    }

    /// Fetch the salsa-memoized workspace aggregate without blocking the async
    /// executor. A warm memo returns quickly, but the cold rebuild after any
    /// file change walks every `FileIndex` in the workspace — run it on the
    /// blocking pool.
    pub(super) async fn workspace_index_async(
        &self,
    ) -> Arc<crate::db::workspace_index::WorkspaceIndexData> {
        let docs = Arc::clone(&self.docs);
        match tokio::task::spawn_blocking(move || docs.get_workspace_index_salsa()).await {
            Ok(wi) => wi,
            // JoinError (panicked/cancelled blocking task): retry inline so a
            // panic surfaces through the caller's panic guard.
            Err(_) => self.docs.get_workspace_index_salsa(),
        }
    }

    /// Tag → generator mapping for deferred code actions.
    pub(super) fn generate_deferred_actions(
        &self,
        tag: &str,
        source: &str,
        doc: &Arc<ParsedDoc>,
        range: Range,
        uri: &Url,
    ) -> Vec<CodeActionOrCommand> {
        match tag {
            "phpdoc" => phpdoc_actions(uri, doc, source, range),
            "implement" => {
                let imports = self.file_imports(uri);
                implement_missing_actions(
                    source,
                    doc,
                    &self
                        .docs
                        .doc_with_others(uri, Arc::clone(doc), &self.open_urls()),
                    range,
                    uri,
                    &imports,
                )
            }
            "constructor" => generate_constructor_actions(source, doc, range, uri),
            "getters_setters" => generate_getters_setters_actions(source, doc, range, uri),
            "return_type" => add_return_type_actions(source, doc, range, uri),
            "promote" => promote_constructor_actions(source, doc, range, uri),
            _ => Vec::new(),
        }
    }

    /// Try to resolve a fully-qualified name via the PSR-4 map.
    /// Indexes the file on-demand if it is not already in the document store.
    pub(super) async fn psr4_goto(&self, fqn: &str) -> Option<Location> {
        let path = self.psr4.load().resolve(fqn)?;

        let file_uri = Url::from_file_path(&path).ok()?;

        // Index on-demand if the file was not picked up by the workspace scan.
        // Use `get_doc_salsa_any` (ignores open-file gating): after `index()`
        // the file is mirrored but background-only, and the call site needs
        // the AST regardless of whether the editor has the file open.
        if self.docs.get_doc_salsa(&file_uri).is_none() {
            let text = tokio::fs::read_to_string(&path).await.ok()?;
            self.index_if_not_open(file_uri.clone(), &text);
        }

        let doc = self.docs.get_doc_salsa(&file_uri)?;

        // Classes are declared by their short (unqualified) name, e.g. `class Foo`
        // not `class App\Services\Foo`.
        let short_name = fqn.split('\\').next_back()?;
        let range = find_declaration_range(doc.source(), &doc, short_name)?;

        Some(Location {
            uri: file_uri,
            range,
        })
    }

    /// Request the client to apply a workspace edit.
    /// Returns true if the edit was successfully applied, false otherwise.
    pub async fn apply_workspace_edit(&self, edit: WorkspaceEdit) -> bool {
        self.client
            .apply_edit(edit)
            .await
            .ok()
            .map(|result| result.applied)
            .unwrap_or(false)
    }
}

/// Run `vendor/bin/phpunit --filter <filter>` and show the result via
/// `window/showMessageRequest`.  Offers "Run Again" on both success and
/// failure, and additionally "Open File" on failure so the user can jump
/// straight to the test source.  Selecting "Run Again" re-executes the test
/// in the same task without returning to the client first.
pub(super) async fn run_phpunit(
    client: &Client,
    filter: &str,
    root: Option<&std::path::Path>,
    file_uri: Option<&Url>,
) {
    let output = tokio::process::Command::new("vendor/bin/phpunit")
        .arg("--filter")
        .arg(filter)
        .current_dir(root.unwrap_or(std::path::Path::new(".")))
        .output()
        .await;

    let (success, message) = match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            let last_line = text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("(no output)")
                .to_string();
            let ok = out.status.success();
            let msg = if ok {
                format!("✓ {filter}: {last_line}")
            } else {
                format!("✗ {filter}: {last_line}")
            };
            (ok, msg)
        }
        Err(e) => (
            false,
            format!("php-lsp.runTest: failed to spawn phpunit — {e}"),
        ),
    };

    let msg_type = if success {
        MessageType::INFO
    } else {
        MessageType::ERROR
    };
    let mut actions = vec![MessageActionItem {
        title: "Run Again".to_string(),
        properties: Default::default(),
    }];
    if !success && file_uri.is_some() {
        actions.push(MessageActionItem {
            title: "Open File".to_string(),
            properties: Default::default(),
        });
    }

    let chosen = client
        .show_message_request(msg_type, message, Some(actions))
        .await;

    match chosen {
        Ok(Some(ref action)) if action.title == "Run Again" => {
            // Re-run once; result shown as a plain message to avoid infinite recursion.
            let output2 = tokio::process::Command::new("vendor/bin/phpunit")
                .arg("--filter")
                .arg(filter)
                .current_dir(root.unwrap_or(std::path::Path::new(".")))
                .output()
                .await;
            let msg2 = match output2 {
                Ok(out) => {
                    let text = String::from_utf8_lossy(&out.stdout).into_owned()
                        + &String::from_utf8_lossy(&out.stderr);
                    let last_line = text
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("(no output)")
                        .to_string();
                    if out.status.success() {
                        format!("✓ {filter}: {last_line}")
                    } else {
                        format!("✗ {filter}: {last_line}")
                    }
                }
                Err(e) => format!("php-lsp.runTest: failed to spawn phpunit — {e}"),
            };
            client.show_message(MessageType::INFO, msg2).await;
        }
        Ok(Some(ref action)) if action.title == "Open File" => {
            if let Some(uri) = file_uri {
                client
                    .show_document(ShowDocumentParams {
                        uri: uri.clone(),
                        external: Some(false),
                        take_focus: Some(true),
                        selection: None,
                    })
                    .await
                    .ok();
            }
        }
        _ => {}
    }
}
