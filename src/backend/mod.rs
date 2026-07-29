use std::path::PathBuf;
use std::sync::Arc;

#[allow(unused_imports)]
use self::helpers::*;

use arc_swap::ArcSwap;

/// Sent to the client once Phase 3 (reference index build) finishes.
/// Allows tests and tooling to wait for the codebase fast path to be active.
enum IndexReadyNotification {}
impl tower_lsp::lsp_types::notification::Notification for IndexReadyNotification {
    type Params = ();
    const METHOD: &'static str = "$/php-lsp/indexReady";
}

/// A `$/progress` partial-result batch for `textDocument/references`.
///
/// `lsp_types::ProgressParamsValue` only has a `WorkDone` variant (it can't
/// carry an arbitrary partial-result payload), so partial results use this
/// hand-rolled notification instead — same `$/progress` method, but `value`
/// is the request's own result type unwrapped, per the LSP spec's
/// partial-result shape.
enum ReferencesPartialResult {}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReferencesPartialResultParams {
    token: tower_lsp::lsp_types::NumberOrString,
    value: Vec<tower_lsp::lsp_types::Location>,
}
impl tower_lsp::lsp_types::notification::Notification for ReferencesPartialResult {
    type Params = ReferencesPartialResultParams;
    const METHOD: &'static str = "$/progress";
}

/// Stream a batch of reference locations to the client as a `$/progress`
/// partial result. Best-effort: the final response is always sent
/// separately and is unaffected if the client ignores this.
pub(crate) async fn send_references_partial_result(
    client: &Client,
    token: tower_lsp::lsp_types::NumberOrString,
    locations: Vec<tower_lsp::lsp_types::Location>,
) {
    client
        .send_notification::<ReferencesPartialResult>(ReferencesPartialResultParams {
            token,
            value: locations,
        })
        .await;
}

use tower_lsp::Client;
use tower_lsp::lsp_types::*;

/// Response for the `$/php-lsp/debugStats` custom request.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct DebugStats {
    /// Cumulative count of real `ParsedDoc` parses (cache misses).
    pub parses: u64,
    /// Times mir's reference index was locked. Reads are per-key posting
    /// lookups and edits commit one file's postings, so this grows by a
    /// small bounded amount per operation — never per candidate file.
    pub ref_index_locks: u64,
    /// Analysis warm sweeps run to completion (not cancelled). Lets benches
    /// and tests wait for the post-index sweep before baselining.
    pub warm_sweeps_completed: u64,
}

use crate::document::ast::ParsedDoc;
use crate::document::document_store::DocumentStore;
use crate::document::open_files::OpenFiles;
use crate::lang::autoload::Psr4Map;
use crate::lang::config::LspConfig;
use crate::laravel::LaravelIndex;

use crate::analysis::diagnostics::merge_file_diagnostics;
use crate::document::open_files::compute_open_file_diagnostics;

pub struct Backend {
    client: Client,
    docs: Arc<DocumentStore>,
    /// Open-file state: text, version token, parse diagnostics.
    /// Files that are only background-indexed (never opened in the editor)
    /// do not appear here; they live only in `DocumentStore`'s salsa layer.
    open_files: OpenFiles,
    root_paths: Arc<ArcSwap<Vec<PathBuf>>>,
    psr4: Arc<ArcSwap<Psr4Map>>,
    laravel: Arc<ArcSwap<LaravelIndex>>,
    config: Arc<ArcSwap<LspConfig>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        // No imperative Codebase field anymore — analysis reads the
        // salsa-memoized `codebase` query, which composes bundled stubs + every
        // file's StubSlice and returns a fresh `Arc<Codebase>` (or the memoized
        // one when inputs are unchanged).
        let docs = Arc::new(DocumentStore::new());
        let psr4 = docs.psr4_arc();
        Backend {
            client,
            docs,
            open_files: OpenFiles::new(),
            root_paths: Arc::new(ArcSwap::from_pointee(Vec::new())),
            psr4,
            laravel: Arc::new(ArcSwap::from_pointee(LaravelIndex::default())),
            config: Arc::new(ArcSwap::from_pointee(LspConfig::default())),
        }
    }

    /// `$/php-lsp/debugStats` — internal observability counters, used by the
    /// stress tests to assert the read path doesn't parse the whole workspace
    /// and that reference-index lock traffic stays bounded per operation.
    pub async fn debug_stats(&self) -> tower_lsp::jsonrpc::Result<DebugStats> {
        Ok(DebugStats {
            parses: self.docs.parse_count(),
            ref_index_locks: self.docs.ref_index_lock_count(),
            warm_sweeps_completed: self.docs.warm_sweeps_completed(),
        })
    }

    fn set_open_text(&self, uri: Url, text: String) -> u64 {
        self.open_files.set_open_text(&self.docs, uri, text)
    }

    fn close_open_file(&self, uri: &Url) {
        self.open_files.close(&self.docs, uri);
    }

    /// Background-index a file from disk, but only if it isn't currently
    /// open in the editor — the editor's buffer is authoritative while a
    /// file is open, and we must not overwrite it with disk contents.
    fn ingest_if_not_open(&self, uri: Url, text: &str) {
        if !self.open_files.contains(&uri) {
            self.docs.ingest(uri, text);
        }
    }

    /// Variant of [`ingest_if_not_open`] that reuses an already-parsed doc.
    fn ingest_from_doc_if_not_open(&self, uri: Url, doc: &ParsedDoc) {
        if !self.open_files.contains(&uri) {
            self.docs.ingest_from_doc(uri, doc);
        }
    }

    fn get_open_text(&self, uri: &Url) -> Option<String> {
        self.open_files.text(uri)
    }

    fn set_parse_diagnostics(&self, uri: &Url, diagnostics: Vec<Diagnostic>) {
        self.open_files.set_parse_diagnostics(uri, diagnostics);
    }

    fn get_parse_diagnostics(&self, uri: &Url) -> Option<Vec<Diagnostic>> {
        self.open_files.parse_diagnostics(uri)
    }

    fn all_open_files_with_diagnostics(&self) -> Vec<(Url, Vec<Diagnostic>, Option<i64>)> {
        self.open_files.all_with_diagnostics()
    }

    fn open_urls(&self) -> Vec<Url> {
        self.open_files.urls()
    }

    fn get_doc(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
        self.open_files.get_doc(&self.docs, uri)
    }

    fn get_doc_stale(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
        self.open_files.get_doc_stale(&self.docs, uri)
    }

    /// `use Foo as Bar;` map for a single file, read directly from the AST.
    fn file_imports(&self, uri: &Url) -> std::collections::HashMap<String, String> {
        self.docs
            .get_doc_salsa(uri)
            .map(|doc| crate::navigation::references::collect_file_imports(&doc))
            .unwrap_or_default()
    }

    /// Resolve the FQN of the symbol at the cursor so reference lookups can match
    /// by exact FQN instead of short name (fixes cross-namespace overmatch for
    /// Function/Class and unrelated-class overmatch for Method via the owning
    /// FQCN). Returns `None` when the kind doesn't carry an FQN or it can't be
    /// resolved. For class constants, returns the owning class short name.
    fn resolve_reference_target_fqn(
        &self,
        uri: &Url,
        doc_opt: Option<&Arc<ParsedDoc>>,
        word: &str,
        kind: Option<crate::navigation::references::SymbolKind>,
        position: Position,
        constant_owner: Option<String>,
    ) -> Option<String> {
        use crate::navigation::references::SymbolKind;
        let doc = doc_opt?;
        let imports = self.file_imports(uri);
        match kind {
            Some(SymbolKind::Function) | Some(SymbolKind::Class) => {
                let resolved = crate::navigation::moniker::resolve_fqn(doc, word, &imports);
                resolved.contains('\\').then_some(resolved)
            }
            Some(SymbolKind::Method) => {
                // Owning FQCN: the class/interface/trait/enum that contains the cursor.
                let short_owner =
                    crate::types::type_map::enclosing_class_at(doc.source(), doc, position)?;
                // `resolve_fqn` walks the doc and applies the namespace prefix if any.
                Some(crate::navigation::moniker::resolve_fqn(
                    doc,
                    &short_owner,
                    &imports,
                ))
            }
            Some(SymbolKind::Property) => {
                // Resolve the owning class when the cursor is on a property
                // declaration (including promoted constructor parameters) —
                // for access sites (`$obj->prop`) the resolved usage symbol
                // from `FileAnalysis::symbol_at` carries the owner instead.
                let stmts = &doc.program().stmts;
                if crate::backend::helpers::cursor_is_on_property_decl(
                    doc.source(),
                    stmts,
                    position,
                )
                .is_none()
                    && promoted_property_at_cursor(doc.source(), stmts, position).is_none()
                {
                    return None;
                }
                let short_owner =
                    crate::types::type_map::enclosing_class_at(doc.source(), doc, position)?;
                Some(crate::navigation::moniker::resolve_fqn(
                    doc,
                    &short_owner,
                    &imports,
                ))
            }
            Some(SymbolKind::Constant) => {
                if let Some(owner) = constant_owner {
                    // Class constant: resolve the owning class to its FQCN —
                    // mir's index keys are `cnst:{fqcn}::{NAME}`.
                    if owner.contains('\\') {
                        Some(owner.trim_start_matches('\\').to_string())
                    } else {
                        Some(crate::navigation::moniker::resolve_fqn(
                            doc, &owner, &imports,
                        ))
                    }
                } else {
                    // Global/namespace constant: compute FQN so cross-namespace
                    // references like `\Config\DB_HOST` can be found.
                    let fqn = crate::navigation::moniker::resolve_fqn(doc, word, &imports);
                    fqn.contains('\\').then_some(fqn)
                }
            }
            _ => None,
        }
    }

    /// Resolve the PHP version to use. See `autoload::resolve_php_version_from_roots`
    /// for the full priority order.
    fn resolve_php_version(&self, explicit: Option<&str>) -> (String, &'static str) {
        let roots = self.root_paths.load();
        crate::lang::autoload::resolve_php_version_from_roots(&roots, explicit)
    }
}

/// Refine the cursor's `(word, kind)` for a references request using
/// declaration-aware heuristics, returning the (possibly rewritten) word, its
/// symbol kind, and — for class constants — the owning class short name.
///
/// Checks, in order: promoted constructor property params (so `$name` in
/// `__construct(public string $name)` resolves to the `->name` property, not
/// `$name` variable occurrences), then method / property / constant
/// declarations, falling back to the character-based `symbol_kind_at` heuristic.
fn resolve_reference_symbol(
    doc_opt: Option<&Arc<ParsedDoc>>,
    source: &str,
    position: Position,
    word: String,
) -> (
    String,
    Option<crate::navigation::references::SymbolKind>,
    Option<String>,
) {
    use crate::navigation::references::SymbolKind;
    let mut constant_owner: Option<String> = None;
    let (word, kind) = if let Some(doc) = doc_opt
        && let Some(prop_name) =
            promoted_property_at_cursor(doc.source(), &doc.program().stmts, position)
    {
        (prop_name, Some(SymbolKind::Property))
    } else if let Some(doc) = doc_opt {
        let stmts = &doc.program().stmts;
        if cursor_is_on_method_decl(doc.source(), stmts, position) {
            (word, Some(SymbolKind::Method))
        } else if let Some(prop_name) = cursor_is_on_property_decl(doc.source(), stmts, position) {
            (prop_name, Some(SymbolKind::Property))
        } else if let Some((const_name, owner)) =
            cursor_is_on_constant_decl(doc.source(), stmts, position)
        {
            constant_owner = owner;
            (const_name, Some(SymbolKind::Constant))
        } else {
            let k = symbol_kind_at(source, position, &word);
            // For constant access sites (`ClassName::CONST`, `self::CONST`),
            // extract the owning class so the constant walker is scoped to
            // the right class rather than falling back to the global-constant
            // path (which would look for a top-level constant named `CONST`).
            if matches!(k, Some(SymbolKind::Constant))
                && let Some(raw) = class_before_double_colon(source, position)
            {
                constant_owner = Some(match raw.as_str() {
                    "self" | "static" => {
                        crate::types::type_map::enclosing_class_at(doc.source(), doc, position)
                            .unwrap_or(raw)
                    }
                    _ => raw,
                });
            }
            (word, k)
        }
    } else {
        let k = symbol_kind_at(source, position, &word);
        (word, k)
    };
    (word, kind, constant_owner)
}

/// Extract the class name (or pseudo-keyword) immediately to the left of `::` at
/// the cursor position. Returns `None` when the cursor is not on an identifier
/// preceded by `::`.
///
/// Used to populate `constant_owner` for constant access sites so that
/// `Status::ACTIVE` (cursor on `ACTIVE`) scopes the constant walker to `Status`
/// rather than treating `ACTIVE` as a global constant.
fn class_before_double_colon(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
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

    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    while char_idx > 0 && is_word(chars[char_idx - 1]) {
        char_idx -= 1;
    }

    if char_idx < 2 || chars[char_idx - 1] != ':' || chars[char_idx - 2] != ':' {
        return None;
    }

    let class_end = char_idx - 2;
    let mut class_start = class_end;
    while class_start > 0 && (is_word(chars[class_start - 1]) || chars[class_start - 1] == '\\') {
        class_start -= 1;
    }

    let name: String = chars[class_start..class_end].iter().collect();
    if name.is_empty() { None } else { Some(name) }
}

/// Off-`self` variant of `Backend::compute_dependent_publishes`. Needed
/// because did_change's blocking republish runs inside a detached
/// `tokio::spawn` that captures `Arc<Backend>` indirectly via clones of
/// `docs` / `open_files` rather than `&self`.
async fn compute_dependent_publishes_owned(
    docs: Arc<DocumentStore>,
    open_files: OpenFiles,
    changed_uri: Url,
    diag_cfg: crate::lang::config::DiagnosticsConfig,
) -> Vec<(Url, Vec<Diagnostic>)> {
    tokio::task::spawn_blocking(move || {
        // rust-analyzer model: we only ever publish for files the editor has
        // open, so re-analyze exactly that set and let salsa memoization make
        // the unaffected ones ~free. No dependent set is computed at all —
        // the old path rebuilt mir's dependency_graph() on every keystroke,
        // an O(all-ingested-files) walk whose cost grew with session age.
        let open_set: Vec<std::sync::Arc<str>> = open_files
            .urls()
            .into_iter()
            .filter(|u| u != &changed_uri)
            .map(|u| std::sync::Arc::from(u.as_str()))
            .collect();
        if open_set.is_empty() {
            return Vec::new();
        }

        let php_version = docs.workspace_php_version();
        let session = docs.analysis_session(php_version);
        // The user is typing: pause the background scan's write storm so this
        // sweep's snapshots aren't repeatedly cancelled while indexing runs.
        let _interactive = docs.interactive_read_guard();
        // Cancel any older in-flight sweep and take a fresh token: when the
        // next edit starts its own sweep, this one stops at its next file
        // boundary instead of blocking typing behind the previous sweep.
        let cancel = docs.begin_reanalyze();
        let analyses = session.reanalyze_files_cancellable(&open_set, &cancel);
        // A newer edit that flipped `cancel` mid-sweep is now authoritative and
        // will republish; drop this sweep's partial results so they can't land
        // out of order and leave stale diagnostics as the final state.
        if cancel.is_cancelled() || analyses.is_empty() {
            return Vec::new();
        }

        let dependents: Vec<(Url, mir_analyzer::FileAnalysis)> = analyses
            .into_iter()
            .filter_map(|(file, analysis)| {
                let url = Url::parse(file.as_ref()).ok()?;
                Some((url, analysis))
            })
            .collect();
        if dependents.is_empty() {
            return Vec::new();
        }

        // Workspace-level class issues (circular inheritance, override
        // violations, abstract-method gaps) aren't in `FileAnalysis` —
        // pull them in one batched call covering every affected file.
        let dep_files: Vec<Arc<str>> = dependents
            .iter()
            .map(|(u, _)| Arc::from(u.as_str()))
            .collect();
        let class_issues = session.class_issues(&dep_files);
        let mut class_issues_by_file: std::collections::HashMap<Arc<str>, Vec<mir_issues::Issue>> =
            std::collections::HashMap::new();
        for issue in class_issues {
            if issue.suppressed {
                continue;
            }
            let file = issue.location.file.clone();
            class_issues_by_file.entry(file).or_default().push(issue);
        }

        let mut out: Vec<(Url, Vec<Diagnostic>)> = Vec::with_capacity(dependents.len());
        for (url, analysis) in dependents {
            let parse = open_files.parse_diagnostics(&url).unwrap_or_default();
            let mut issues: Vec<mir_issues::Issue> = analysis
                .issues
                .into_iter()
                .filter(|i| !i.suppressed)
                .collect();
            if let Some(extra) = class_issues_by_file.remove(&Arc::<str>::from(url.as_str())) {
                issues.extend(extra);
            }
            let semantic =
                crate::semantic_diagnostics::issues_to_diagnostics(&issues, &url, &diag_cfg);
            out.push((url, merge_file_diagnostics(parse, semantic)));
        }
        out
    })
    .await
    .unwrap_or_default()
}

/// Content hash of a diagnostics set, for skipping republishes the client
/// already displays. In-process only — never persisted.
pub(super) fn diagnostics_content_hash(diagnostics: &[Diagnostic]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    serde_json::to_string(diagnostics)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

/// Compute and publish diagnostics for `uri`, then republish any open files
/// that depend on it. Requires `open_files.set_parse_diagnostics` to be up to
/// date for `uri` before this is called.
///
/// The edited file always publishes (editors rely on one publish per change).
/// Dependents publish only when their content differs from the last publish —
/// the sweep re-analyzes every open file on every edit, and unchanged results
/// would otherwise flood the client with no-op notifications.
pub(super) async fn publish_with_dependents(
    client: Client,
    docs: Arc<DocumentStore>,
    open_files: OpenFiles,
    uri: Url,
    diag_cfg: crate::lang::config::DiagnosticsConfig,
) {
    let docs_ref = Arc::clone(&docs);
    let open_files_ref = open_files.clone();
    let uri_ref = uri.clone();
    let diag_cfg_ref = diag_cfg.clone();
    let all_diags = tokio::task::spawn_blocking(move || {
        compute_open_file_diagnostics(&docs_ref, &open_files_ref, &uri_ref, &diag_cfg_ref)
    })
    .await
    .unwrap_or_default();
    open_files.note_published(&uri, diagnostics_content_hash(&all_diags));
    client
        .publish_diagnostics(uri.clone(), all_diags, None)
        .await;
    let dependents =
        compute_dependent_publishes_owned(docs, open_files.clone(), uri, diag_cfg).await;
    for (dep_uri, dep_diags) in dependents {
        let hash = diagnostics_content_hash(&dep_diags);
        if open_files.published_hash(&dep_uri) == Some(hash) {
            continue;
        }
        open_files.note_published(&dep_uri, hash);
        client.publish_diagnostics(dep_uri, dep_diags, None).await;
    }
}

/// Generate a stable result_id for diagnostics. Uses the count and position of diagnostics
/// to create a stable identifier. Same diagnostics = same result_id.
fn compute_diagnostic_result_id(diagnostics: &[Diagnostic], uri: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    uri.hash(&mut hasher);
    diagnostics.len().hash(&mut hasher);

    for diag in diagnostics {
        diag.range.start.line.hash(&mut hasher);
        diag.range.start.character.hash(&mut hasher);
        diag.range.end.line.hash(&mut hasher);
        diag.range.end.character.hash(&mut hasher);
        diag.message.hash(&mut hasher);
        let severity_val = match diag.severity {
            Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR) => 1,
            Some(tower_lsp::lsp_types::DiagnosticSeverity::WARNING) => 2,
            Some(tower_lsp::lsp_types::DiagnosticSeverity::INFORMATION) => 3,
            Some(tower_lsp::lsp_types::DiagnosticSeverity::HINT) => 4,
            None => 0,
            _ => 5, // Unknown variants
        };
        severity_val.hash(&mut hasher);
        if let Some(code) = &diag.code {
            format!("{:?}", code).hash(&mut hasher);
        }
        if let Some(source) = &diag.source {
            source.hash(&mut hasher);
        }
        if let Some(tags) = &diag.tags {
            for tag in tags {
                let tag_val = match *tag {
                    tower_lsp::lsp_types::DiagnosticTag::UNNECESSARY => 1,
                    tower_lsp::lsp_types::DiagnosticTag::DEPRECATED => 2,
                    _ => 3,
                };
                tag_val.hash(&mut hasher);
            }
        }
    }

    format!("v1:{:x}", hasher.finish())
}

mod handlers;
mod helpers;
pub mod panic_guard;
mod server;
#[cfg(test)]
mod tests;
