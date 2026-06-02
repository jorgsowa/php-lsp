use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::notification::Progress as ProgressNotification;

/// Sent to the client once Phase 3 (reference index build) finishes.
/// Allows tests and tooling to wait for the codebase fast path to be active.
enum IndexReadyNotification {}
impl tower_lsp::lsp_types::notification::Notification for IndexReadyNotification {
    type Params = ();
    const METHOD: &'static str = "$/php-lsp/indexReady";
}
use tower_lsp::lsp_types::request::WorkDoneProgressCreate;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, async_trait};

use php_ast::{
    ClassMember, ClassMemberKind, EnumMember, EnumMemberKind, ExprKind, NamespaceBody, Stmt,
    StmtKind,
};

use crate::ast::{ParsedDoc, str_offset};
use crate::autoload::Psr4Map;
use crate::call_hierarchy::{incoming_calls, outgoing_calls, prepare_call_hierarchy};
use crate::code_lens::code_lenses;
use crate::completion::{CompletionCtx, filtered_completions_at};
use crate::config::LspConfig;
use crate::declaration::{goto_declaration, goto_declaration_from_index};
use crate::definition::{
    find_declaration_in_indexes, find_declaration_range, find_method_in_class_hierarchy,
    goto_definition,
};
use crate::diagnostics::{merge_file_diagnostics, parse_document, parse_document_no_diags};
use crate::document_highlight::document_highlights;
use crate::document_link::document_links;
use crate::document_store::DocumentStore;
use crate::extract_action::extract_variable_actions;
use crate::extract_constant_action::extract_constant_actions;
use crate::extract_method_action::extract_method_actions;
use crate::file_rename::{use_edits_for_delete, use_edits_for_rename};
use crate::folding::folding_ranges;
use crate::formatting::{format_document, format_range};
use crate::generate_action::{generate_constructor_actions, generate_getters_setters_actions};
use crate::hover::{
    class_hover_from_index, docs_for_symbol_from_index, hover_info, signature_for_symbol_from_index,
};
use crate::implement_action::implement_missing_actions;
use crate::implementation::{find_implementations, find_implementations_from_workspace};
use crate::inlay_hints::inlay_hints;
use crate::inline_action::inline_variable_actions;
use crate::inline_value::inline_values_in_range;
use crate::moniker::moniker_at;
use crate::on_type_format::on_type_format;
use crate::open_files::{OpenFiles, compute_open_file_diagnostics};
use crate::organize_imports::organize_imports_action;
use crate::panic_guard::{guard_async, guard_async_result};
use crate::phpdoc_action::phpdoc_actions;
use crate::phpstorm_meta::PhpStormMeta;
use crate::promote_action::promote_constructor_actions;
use crate::references::{
    SymbolKind, find_constructor_references, find_references, find_references_with_target,
};
use crate::rename::{prepare_rename, rename, rename_property, rename_variable};
use crate::selection_range::selection_ranges;
use crate::semantic_diagnostics::duplicate_declaration_diagnostics;
use crate::semantic_tokens::{
    compute_token_delta, legend, semantic_tokens, semantic_tokens_range, token_hash,
};
use crate::signature_help::signature_help;
use crate::symbols::{
    document_symbols, resolve_workspace_symbol, workspace_symbols_from_workspace,
};
use crate::type_action::add_return_type_actions;
use crate::type_definition::{goto_type_definition, goto_type_definition_from_index};
use crate::type_hierarchy::{
    prepare_type_hierarchy_from_workspace, subtypes_of_from_workspace, supertypes_of_from_workspace,
};
use crate::use_import::{build_use_import_edit, find_fqn_for_class};
use crate::util::{fqn_short_name, word_at_position};
use crate::workspace_scan::{scan_workspace, send_refresh_requests};

pub struct Backend {
    client: Client,
    docs: Arc<DocumentStore>,
    /// Open-file state: text, version token, parse diagnostics.
    /// Files that are only background-indexed (never opened in the editor)
    /// do not appear here; they live only in `DocumentStore`'s salsa layer.
    open_files: OpenFiles,
    root_paths: Arc<ArcSwap<Vec<PathBuf>>>,
    psr4: Arc<ArcSwap<Psr4Map>>,
    meta: Arc<ArcSwap<PhpStormMeta>>,
    config: Arc<ArcSwap<LspConfig>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        // No imperative Codebase field anymore — `self.codebase()` below
        // delegates to the salsa-memoized `codebase` query, which composes
        // bundled stubs + every file's StubSlice and returns a fresh
        // `Arc<Codebase>` (or the memoized one when inputs are unchanged).
        let docs = Arc::new(DocumentStore::new());
        let psr4 = docs.psr4_arc();
        Backend {
            client,
            docs,
            open_files: OpenFiles::new(),
            root_paths: Arc::new(ArcSwap::from_pointee(Vec::new())),
            psr4,
            meta: Arc::new(ArcSwap::from_pointee(PhpStormMeta::default())),
            config: Arc::new(ArcSwap::from_pointee(LspConfig::default())),
        }
    }

    // ── Open-file state convenience wrappers (Phase E4) ──────────────────────

    fn set_open_text(&self, uri: Url, text: String) -> u64 {
        self.open_files.set_open_text(&self.docs, uri, text)
    }

    fn close_open_file(&self, uri: &Url) {
        self.open_files.close(&self.docs, uri);
    }

    /// Background-index a file from disk, but only if it isn't currently
    /// open in the editor — the editor's buffer is authoritative while a
    /// file is open, and we must not overwrite it with disk contents.
    fn index_if_not_open(&self, uri: Url, text: &str) {
        if !self.open_files.contains(&uri) {
            self.docs.index(uri, text);
        }
    }

    /// Variant of [`index_if_not_open`] that reuses an already-parsed doc.
    fn index_from_doc_if_not_open(&self, uri: Url, doc: &ParsedDoc) {
        if !self.open_files.contains(&uri) {
            self.docs.index_from_doc(uri, doc);
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

    /// Current MirDb snapshot for the workspace, owned by the
    /// `AnalysisSession`. Cheap clone (Arc-wrapped internals).
    fn codebase(&self) -> mir_analyzer::db::MirDbStorage {
        let php_version = self.docs.workspace_php_version();
        let session = self.docs.analysis_session(php_version);
        session.snapshot_db()
    }

    /// `use Foo as Bar;` map for a single file, read directly from the AST.
    fn file_imports(&self, uri: &Url) -> std::collections::HashMap<String, String> {
        self.docs
            .get_doc_salsa(uri)
            .map(|doc| crate::references::collect_file_imports(&doc))
            .unwrap_or_default()
    }

    /// Reference call sites for a class's `__construct`.
    ///
    /// The constructor's call sites are `new OwningClass(...)`, not
    /// `->__construct()`, so name-only matching would return every class's
    /// constructor declaration. We search for `new` expressions only, scoped to
    /// the owning class.
    ///
    /// `class_name` is the FQN when the constructor is inside a namespace
    /// (e.g. `"Shop\\Order"`). The AST walker searches for the *short* name
    /// (`"Order"`) since that's what appears at call sites, while the FQN is
    /// used only to scope the search and prevent collisions between two classes
    /// with the same short name in different namespaces.
    fn construct_references(
        &self,
        uri: &Url,
        source: &str,
        position: Position,
        class_name: &str,
        include_declaration: bool,
    ) -> Vec<Location> {
        let all_docs = self.docs.all_docs_for_scan();
        let short_name = fqn_short_name(class_name).to_owned();
        let class_fqn = class_name.contains('\\').then_some(class_name);
        // `find_constructor_references` walks `new` expressions directly —
        // bypasses the codebase/salsa index whose `ClassReference` key is too
        // broad (covers type hints, `instanceof`, `extends`, `implements`).
        let mut locations = find_constructor_references(&short_name, &all_docs, class_fqn);
        // The cursor is already on the `__construct` name, so derive the span
        // from the identifier under the cursor rather than re-searching via
        // str_offset (which finds the first occurrence in the file and would
        // point at the wrong constructor in files with more than one class).
        if include_declaration && let Some(range) = crate::util::word_range_at(source, position) {
            locations.push(Location {
                uri: uri.clone(),
                range,
            });
        }
        locations
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
        kind: Option<crate::references::SymbolKind>,
        position: Position,
        constant_owner: Option<String>,
    ) -> Option<String> {
        use crate::references::SymbolKind;
        let doc = doc_opt?;
        let imports = self.file_imports(uri);
        match kind {
            Some(SymbolKind::Function) | Some(SymbolKind::Class) => {
                let resolved = crate::moniker::resolve_fqn(doc, word, &imports);
                resolved.contains('\\').then_some(resolved)
            }
            Some(SymbolKind::Method) => {
                // Owning FQCN: the class/interface/trait/enum that contains the cursor.
                let short_owner = crate::type_map::enclosing_class_at(doc.source(), doc, position)?;
                // `resolve_fqn` walks the doc and applies the namespace prefix if any.
                Some(crate::moniker::resolve_fqn(doc, &short_owner, &imports))
            }
            Some(SymbolKind::Constant) => {
                if constant_owner.is_some() {
                    // Class constant: the owning class short name as-is.
                    constant_owner
                } else {
                    // Global/namespace constant: compute FQN so cross-namespace
                    // references like `\Config\DB_HOST` can be found.
                    let fqn = crate::moniker::resolve_fqn(doc, word, &imports);
                    fqn.contains('\\').then_some(fqn)
                }
            }
            _ => None,
        }
    }

    /// Type-aware method call sites from the mir session.
    ///
    /// Method refs need type-aware filtering: `$mailer->process()` and
    /// `$queue->process()` share a name, but only the one whose receiver type
    /// matches the cursor's owning class is a real ref. Mir's `references_to` is
    /// type-aware; use it as the primary source for Method+`target_fqn`.
    ///
    /// Returns `None` when the kind isn't `Method` or no mir symbol can be built;
    /// otherwise the (possibly empty) call-site set, filtered to files that
    /// actually mention `owner_short` (drops untyped/Mixed receivers — any file
    /// where a receiver is legitimately typed as the owner must reference it by
    /// name somewhere via import, `new`, or type hint).
    fn session_method_references(
        &self,
        word: &str,
        kind: Option<crate::references::SymbolKind>,
        target_fqn: Option<&str>,
        owner_short: Option<&str>,
    ) -> Option<Vec<Location>> {
        if !matches!(kind, Some(crate::references::SymbolKind::Method)) {
            return None;
        }
        let sym = build_mir_symbol(word, kind, target_fqn)?;
        let locs = self
            .docs
            .session_references_to(&sym)
            .into_iter()
            .filter_map(|tuple| {
                let loc = crate::references::session_tuple_to_location(tuple)?;
                if let Some(short) = owner_short {
                    let mentions = self
                        .docs
                        .source_text(&loc.uri)
                        .as_ref()
                        .map(|src| src.contains(short))
                        .unwrap_or(true);
                    if !mentions {
                        return None;
                    }
                }
                Some(loc)
            })
            .collect();
        Some(locs)
    }

    /// Resolve the PHP version to use. See `autoload::resolve_php_version_from_roots`
    /// for the full priority order.
    fn resolve_php_version(&self, explicit: Option<&str>) -> (String, &'static str) {
        let roots = self.root_paths.load();
        crate::autoload::resolve_php_version_from_roots(&roots, explicit)
    }

    /// Compute diagnostic publishes for every open dependent of `changed_uri`.
    /// Uses `session.analyze_dependents_of` to scope work to files whose
    /// Pass-2 results actually changed; merges LSP-side parse + duplicate-decl
    /// diagnostics so the publish reflects the full picture per file.
    async fn compute_dependent_publishes(
        &self,
        changed_uri: &Url,
        diag_cfg: &crate::config::DiagnosticsConfig,
    ) -> Vec<(Url, Vec<Diagnostic>)> {
        compute_dependent_publishes_owned(
            Arc::clone(&self.docs),
            self.open_files.clone(),
            changed_uri.clone(),
            diag_cfg.clone(),
        )
        .await
    }
}

/// Build a `mir_analyzer::Name` from the cursor-resolved `(word, kind,
/// target_fqn)` triple, when there's enough information to construct one.
/// Returns `None` when:
/// - `kind` is `None` (cursor not on a recognizable symbol) or `Property`
///   (mir doesn't track property refs at the session-API level), or
/// - the required FQN piece isn't available.
fn build_mir_symbol(
    word: &str,
    kind: Option<crate::references::SymbolKind>,
    target_fqn: Option<&str>,
) -> Option<mir_analyzer::Name> {
    use crate::references::SymbolKind;
    use std::sync::Arc as StdArc;
    match kind {
        Some(SymbolKind::Function) => {
            target_fqn.map(|fqn| mir_analyzer::Name::Function(StdArc::from(fqn)))
        }
        Some(SymbolKind::Class) => {
            target_fqn.map(|fqn| mir_analyzer::Name::Class(StdArc::from(fqn)))
        }
        Some(SymbolKind::Method) => target_fqn.map(|owning| mir_analyzer::Name::Method {
            class: StdArc::from(owning),
            // PHP method dispatch is case-insensitive — Symbol::method
            // normalizes the name. The constructor function does this for us.
            name: StdArc::from(word.to_ascii_lowercase()),
        }),
        Some(SymbolKind::Property) | Some(SymbolKind::Constant) | None => None,
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
    Option<crate::references::SymbolKind>,
    Option<String>,
) {
    use crate::references::SymbolKind;
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
            (word, k)
        }
    } else {
        let k = symbol_kind_at(source, position, &word);
        (word, k)
    };
    (word, kind, constant_owner)
}

/// Off-`self` variant of `Backend::compute_dependent_publishes`. Needed
/// because did_change's blocking republish runs inside a detached
/// `tokio::spawn` that captures `Arc<Backend>` indirectly via clones of
/// `docs` / `open_files` rather than `&self`.
async fn compute_dependent_publishes_owned(
    docs: Arc<DocumentStore>,
    open_files: OpenFiles,
    changed_uri: Url,
    diag_cfg: crate::config::DiagnosticsConfig,
) -> Vec<(Url, Vec<Diagnostic>)> {
    tokio::task::spawn_blocking(move || {
        // Ask mir which files actually depend on `changed_uri` and let it
        // re-run Pass 2 for them in parallel. mir 0.25's dependency graph
        // covers every reference kind that can produce a cross-file
        // diagnostic (imports, class hierarchy, type hints, instanceof,
        // catch, ::class, ::CONST, `new`, static and instance calls) and
        // tracks symbols-deleted-from-a-file so renames / deletions still
        // surface the orphaned dependents.
        let php_version = docs.workspace_php_version();
        let session = docs.analysis_session(php_version);
        let analyses = session.reanalyze_dependents(changed_uri.as_str());
        if analyses.is_empty() {
            return Vec::new();
        }

        // We only publish for files the editor has open. Filter the
        // session-wide dependent set down to open URLs.
        let open_urls: std::collections::HashSet<Url> = open_files
            .urls()
            .into_iter()
            .filter(|u| u != &changed_uri)
            .collect();
        let dependents: Vec<(Url, mir_analyzer::FileAnalysis)> = analyses
            .into_iter()
            .filter_map(|(file, analysis)| {
                let url = Url::parse(file.as_ref()).ok()?;
                open_urls.contains(&url).then_some((url, analysis))
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
            let dup_decl = open_files
                .get_doc(&docs, &url)
                .map(|d| {
                    let source = open_files.text(&url).unwrap_or_default();
                    crate::semantic_diagnostics::duplicate_declaration_diagnostics(
                        &source, &d, &diag_cfg,
                    )
                })
                .unwrap_or_default();
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
            out.push((url, merge_file_diagnostics(parse, dup_decl, semantic)));
        }
        out
    })
    .await
    .unwrap_or_default()
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

#[async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Collect all workspace roots. Prefer workspace_folders (multi-root) over
        // the deprecated root_uri (single root).
        {
            let mut roots: Vec<PathBuf> = params
                .workspace_folders
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .filter_map(|f| f.uri.to_file_path().ok())
                .collect();
            if roots.is_empty()
                && let Some(path) = params.root_uri.as_ref().and_then(|u| u.to_file_path().ok())
            {
                roots.push(path);
            }
            self.root_paths.store(Arc::new(roots));
        }

        // Pre-load PSR-4 map synchronously during initialize so it is available
        // before the first didOpen arrives. The initialized handler reloads it too
        // (after workspace folders may be updated), but doing it here eliminates
        // the race where didOpen runs before the initialized handler finishes its
        // register_capability round-trip.
        {
            let roots = self.root_paths.load_full();
            if !roots.is_empty() {
                let mut merged = Psr4Map::empty();
                for root in roots.iter() {
                    merged.extend(Psr4Map::load(root));
                }
                self.psr4.store(Arc::new(merged));
            }
        }

        {
            let opts = params.initialization_options.as_ref();
            let roots = self.root_paths.load_full();
            let file_cfg = crate::autoload::load_project_config_json(&roots);

            if matches!(file_cfg, Some(serde_json::Value::Null)) {
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        "php-lsp: .php-lsp.json contains invalid JSON — ignoring",
                    )
                    .await;
            }

            if let Some(serde_json::Value::Object(ref obj)) = file_cfg
                && let Some(ver) = obj.get("phpVersion").and_then(|v| v.as_str())
                && !crate::autoload::is_valid_php_version(ver)
            {
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: .php-lsp.json unsupported phpVersion {ver:?} — valid values: {}",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }

            if let Some(ver) = opts
                .and_then(|o| o.get("phpVersion"))
                .and_then(|v| v.as_str())
                && !crate::autoload::is_valid_php_version(ver)
            {
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: unsupported phpVersion {ver:?} — valid values: {}",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }

            // Merge: file config is the base; editor initializationOptions override per-key.
            // excludePaths arrays are concatenated rather than replaced.
            let file_obj = file_cfg.as_ref().filter(|v| v.is_object());
            let merged = LspConfig::merge_project_configs(file_obj, opts);
            let mut cfg = LspConfig::from_value(&merged);

            // Resolve the PHP version and log what was chosen and why.
            // phpVersion from initializationOptions is already in cfg.php_version (editor wins).
            // If neither editor nor .php-lsp.json set it, resolve_php_version falls through
            // to composer.json / php binary / default.
            let (ver, source) = self.resolve_php_version(cfg.php_version.as_deref());
            self.client
                .log_message(
                    tower_lsp::lsp_types::MessageType::INFO,
                    format!("php-lsp: using PHP {ver} ({source})"),
                )
                .await;
            // Show a visible warning when auto-detection yields a version outside
            // our supported range (e.g. a legacy project with ">=5.6" in composer.json),
            // then clamp to the nearest supported version so analysis stays meaningful.
            let ver = if source != "set by editor" && !crate::autoload::is_valid_php_version(&ver) {
                let clamped = crate::autoload::clamp_php_version(&ver);
                self.client
                    .show_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: detected PHP {ver} is outside the supported range ({}); \
                             using PHP {clamped} for analysis",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
                clamped.to_string()
            } else {
                ver
            };
            cfg.php_version = Some(ver.clone());
            if let Ok(pv) = ver.parse::<mir_analyzer::PhpVersion>() {
                self.docs.set_php_version(pv);
            }
            self.config.store(Arc::new(cfg));
        }

        let feat = self.config.load().features.clone();
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        will_save: Some(true),
                        will_save_wait_until: Some(true),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                    },
                )),
                completion_provider: feat.completion.then(|| CompletionOptions {
                    trigger_characters: Some(vec![
                        "$".to_string(),
                        ">".to_string(),
                        ":".to_string(),
                        "(".to_string(),
                        "[".to_string(),
                    ]),
                    resolve_provider: Some(true),
                    ..Default::default()
                }),
                hover_provider: feat.hover.then_some(HoverProviderCapability::Simple(true)),
                definition_provider: feat.definition.then_some(OneOf::Left(true)),
                references_provider: feat.references.then_some(OneOf::Left(true)),
                document_symbol_provider: feat.document_symbols.then_some(OneOf::Left(true)),
                workspace_symbol_provider: feat.workspace_symbols.then(|| {
                    OneOf::Right(WorkspaceSymbolOptions {
                        resolve_provider: Some(true),
                        work_done_progress_options: Default::default(),
                    })
                }),
                rename_provider: feat.rename.then(|| {
                    OneOf::Right(RenameOptions {
                        prepare_provider: Some(true),
                        work_done_progress_options: Default::default(),
                    })
                }),
                signature_help_provider: feat.signature_help.then(|| SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                inlay_hint_provider: feat.inlay_hints.then(|| {
                    OneOf::Right(InlayHintServerCapabilities::Options(InlayHintOptions {
                        resolve_provider: Some(true),
                        work_done_progress_options: Default::default(),
                    }))
                }),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                semantic_tokens_provider: feat.semantic_tokens.then(|| {
                    SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                        legend: legend(),
                        full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
                        range: Some(true),
                        ..Default::default()
                    })
                }),
                selection_range_provider: feat
                    .selection_range
                    .then_some(SelectionRangeProviderCapability::Simple(true)),
                call_hierarchy_provider: feat
                    .call_hierarchy
                    .then_some(CallHierarchyServerCapability::Simple(true)),
                document_highlight_provider: feat.document_highlight.then_some(OneOf::Left(true)),
                implementation_provider: feat
                    .implementation
                    .then_some(ImplementationProviderCapability::Simple(true)),
                code_action_provider: feat.code_action.then(|| {
                    CodeActionProviderCapability::Options(CodeActionOptions {
                        resolve_provider: Some(true),
                        ..Default::default()
                    })
                }),
                declaration_provider: feat
                    .declaration
                    .then_some(DeclarationCapability::Simple(true)),
                type_definition_provider: feat
                    .type_definition
                    .then_some(TypeDefinitionProviderCapability::Simple(true)),
                code_lens_provider: feat.code_lens.then_some(CodeLensOptions {
                    resolve_provider: Some(true),
                }),
                document_formatting_provider: feat.formatting.then_some(OneOf::Left(true)),
                document_range_formatting_provider: feat
                    .range_formatting
                    .then_some(OneOf::Left(true)),
                document_on_type_formatting_provider: feat.on_type_formatting.then(|| {
                    DocumentOnTypeFormattingOptions {
                        first_trigger_character: "}".to_string(),
                        more_trigger_character: Some(vec!["\n".to_string()]),
                    }
                }),
                document_link_provider: feat.document_link.then(|| DocumentLinkOptions {
                    resolve_provider: Some(true),
                    work_done_progress_options: Default::default(),
                }),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["php-lsp.runTest".to_string()],
                    work_done_progress_options: Default::default(),
                }),
                diagnostic_provider: Some(DiagnosticServerCapabilities::Options(
                    DiagnosticOptions {
                        identifier: None,
                        inter_file_dependencies: true,
                        workspace_diagnostics: true,
                        work_done_progress_options: Default::default(),
                    },
                )),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                        will_rename: Some(php_file_op()),
                        did_rename: Some(php_file_op()),
                        did_create: Some(php_file_op()),
                        will_delete: Some(php_file_op()),
                        did_delete: Some(php_file_op()),
                        ..Default::default()
                    }),
                }),
                linked_editing_range_provider: feat
                    .linked_editing_range
                    .then_some(LinkedEditingRangeServerCapabilities::Simple(true)),
                moniker_provider: Some(OneOf::Left(true)),
                inline_value_provider: feat.inline_values.then(|| {
                    OneOf::Right(InlineValueServerCapabilities::Options(InlineValueOptions {
                        work_done_progress_options: Default::default(),
                    }))
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        // Register dynamic capabilities: file watcher + type hierarchy
        let php_selector = serde_json::json!([{"language": "php"}]);
        let registrations = vec![
            Registration {
                id: "php-lsp-file-watcher".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(serde_json::json!({
                    "watchers": [{"globPattern": "**/*.php"}]
                })),
            },
            // Type hierarchy has no static ServerCapabilities field in lsp-types 0.94,
            // so register it dynamically here.
            Registration {
                id: "php-lsp-type-hierarchy".to_string(),
                method: "textDocument/prepareTypeHierarchy".to_string(),
                register_options: Some(serde_json::json!({"documentSelector": php_selector})),
            },
            Registration {
                id: "php-lsp-config-change".to_string(),
                method: "workspace/didChangeConfiguration".to_string(),
                register_options: Some(serde_json::json!({"section": "php-lsp"})),
            },
        ];
        self.client.register_capability(registrations).await.ok();

        let roots: Vec<PathBuf> = (**self.root_paths.load()).clone();
        if !roots.is_empty() {
            {
                let mut merged = Psr4Map::empty();
                for root in &roots {
                    merged.extend(Psr4Map::load(root));
                }
                self.psr4.store(Arc::new(merged));
            }
            self.meta.store(Arc::new(PhpStormMeta::load(&roots[0])));

            let token = NumberOrString::String("php-lsp/indexing".to_string());
            self.client
                .send_request::<WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                    token: token.clone(),
                })
                .await
                .ok();

            let docs = Arc::clone(&self.docs);
            let open_files = self.open_files.clone();
            let client = self.client.clone();
            let (exclude_paths, include_paths, max_indexed_files) = {
                let cfg = self.config.load();
                let mut exclude = cfg.exclude_paths.clone();
                // Lazy vendor: by default we skip `vendor/` during the eager
                // scan and rely on PSR-4 resolution (`psr4_goto`) to index
                // vendor files on demand. Users with workspaces small enough
                // to want full-vendor symbol search can opt in via
                // `indexVendor: true`.
                if !cfg.index_vendor && !exclude.iter().any(|p| p == "vendor" || p == "vendor/") {
                    exclude.push("vendor/".to_string());
                }
                (exclude, cfg.include_paths.clone(), cfg.max_indexed_files)
            };
            tokio::spawn(async move {
                client
                    .send_notification::<ProgressNotification>(ProgressParams {
                        token: token.clone(),
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                            WorkDoneProgressBegin {
                                title: "php-lsp: indexing workspace".to_string(),
                                cancellable: Some(false),
                                message: None,
                                percentage: None,
                            },
                        )),
                    })
                    .await;

                let mut total = 0usize;
                for root in roots {
                    // Phase K2b: open the on-disk cache for this root. If the
                    // system has no usable cache dir (weird XDG env, sandboxed
                    // runner, read-only home), `new` returns None and every
                    // per-file `cache.as_ref()` guard below no-ops — scan still
                    // runs, just without persistence.
                    let cache = crate::cache::WorkspaceCache::new(&root);
                    total += scan_workspace(
                        root,
                        Arc::clone(&docs),
                        open_files.clone(),
                        cache,
                        &exclude_paths,
                        &include_paths,
                        max_indexed_files,
                    )
                    .await;
                }

                client
                    .send_notification::<ProgressNotification>(ProgressParams {
                        token,
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(
                            WorkDoneProgressEnd {
                                message: Some(format!("Indexed {total} files")),
                            },
                        )),
                    })
                    .await;

                client
                    .log_message(
                        MessageType::INFO,
                        format!("php-lsp: indexed {total} workspace files"),
                    )
                    .await;

                // Ask clients to re-request tokens/lenses/hints/diagnostics now
                // that the index is populated. Without this, editors that opened
                // files before indexing finished would show stale information.
                send_refresh_requests(&client).await;

                // Phase D: reference index is lazy. `textDocument/references`
                // drives `symbol_refs(ws, key)` on demand; salsa memoizes the
                // per-file `file_refs` across requests. Invalidation is
                // automatic on edits.
                //
                // Phase L: warm the memo in the background so the first real
                // reference lookup doesn't pay the full-workspace walk.
                // `symbol_refs(ws, <any key>)` iterates every file's
                // `file_refs` to build its result — even with a sentinel key
                // that matches nothing, the per-file walk runs and populates
                // salsa's memo. Fire-and-forget: a reference request that
                // arrives mid-warmup just retries through
                // `snapshot_query`'s `salsa::Cancelled` handling.
                let warm_docs = Arc::clone(&docs);
                tokio::task::spawn_blocking(move || {
                    // Pre-compute file_index for every workspace file so the first
                    // hover/completion does not pay the full parse cost at request time.
                    warm_docs.get_workspace_index_salsa();
                })
                .await
                .ok();
                drop(docs);
                client.send_notification::<IndexReadyNotification>(()).await;
            });
        }

        self.client
            .log_message(MessageType::INFO, "php-lsp ready")
            .await;
    }

    async fn did_change_configuration(&self, _params: DidChangeConfigurationParams) {
        // Pull the current configuration from the client rather than parsing the
        // (often-null) params.settings, which not all clients populate.
        let items = vec![ConfigurationItem {
            scope_uri: None,
            section: Some("php-lsp".to_string()),
        }];
        if let Ok(values) = self.client.configuration(items).await
            && let Some(value) = values.into_iter().next()
        {
            let roots = self.root_paths.load_full();

            // Re-read .php-lsp.json so a user who edits the file and then
            // triggers a configuration reload picks up the latest values.
            let file_cfg = crate::autoload::load_project_config_json(&roots);

            if let Some(ver) = value.get("phpVersion").and_then(|v| v.as_str())
                && !crate::autoload::is_valid_php_version(ver)
            {
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: unsupported phpVersion {ver:?} — valid values: {}",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }

            let file_obj = file_cfg.as_ref().filter(|v| v.is_object());
            let merged = LspConfig::merge_project_configs(file_obj, Some(&value));
            let mut cfg = LspConfig::from_value(&merged);

            // Resolve the PHP version and log what was chosen and why.
            let (ver, source) = self.resolve_php_version(cfg.php_version.as_deref());
            self.client
                .log_message(
                    tower_lsp::lsp_types::MessageType::INFO,
                    format!("php-lsp: using PHP {ver} ({source})"),
                )
                .await;
            // Clamp unsupported versions to the nearest supported one and warn.
            let ver = if source != "set by editor" && !crate::autoload::is_valid_php_version(&ver) {
                let clamped = crate::autoload::clamp_php_version(&ver);
                self.client
                    .show_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: detected PHP {ver} is outside the supported range ({}); \
                             using PHP {clamped} for analysis",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
                clamped.to_string()
            } else {
                ver
            };
            cfg.php_version = Some(ver.clone());
            if let Ok(pv) = ver.parse::<mir_analyzer::PhpVersion>() {
                self.docs.set_php_version(pv);
            }
            self.config.store(Arc::new(cfg));
            send_refresh_requests(&self.client).await;
        }
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        // Remove folders from our tracked roots.
        {
            let mut roots = (**self.root_paths.load()).clone();
            for removed in &params.event.removed {
                if let Ok(path) = removed.uri.to_file_path() {
                    roots.retain(|r| r != &path);
                }
            }
            self.root_paths.store(Arc::new(roots));
        }

        // Add new folders and kick off background scans for each.
        let (exclude_paths, include_paths, max_indexed_files) = {
            let cfg = self.config.load();
            (
                cfg.exclude_paths.clone(),
                cfg.include_paths.clone(),
                cfg.max_indexed_files,
            )
        };
        for added in &params.event.added {
            if let Ok(path) = added.uri.to_file_path() {
                let is_new = {
                    let mut roots = (**self.root_paths.load()).clone();
                    if !roots.contains(&path) {
                        roots.push(path.clone());
                        self.root_paths.store(Arc::new(roots));
                        true
                    } else {
                        false
                    }
                };
                if is_new {
                    let docs = Arc::clone(&self.docs);
                    let open_files = self.open_files.clone();
                    let ex = exclude_paths.clone();
                    let ip = include_paths.clone();
                    let path_clone = path.clone();
                    let client = self.client.clone();
                    tokio::spawn(async move {
                        let cache = crate::cache::WorkspaceCache::new(&path_clone);
                        scan_workspace(
                            path_clone,
                            docs,
                            open_files,
                            cache,
                            &ex,
                            &ip,
                            max_indexed_files,
                        )
                        .await;
                        send_refresh_requests(&client).await;
                    });
                }
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        guard_async("did_open", async move {
            let uri = params.text_document.uri;
            let text = params.text_document.text;

            // Store text immediately so other features work while parsing.
            // This also mirrors the new text into salsa, so the codebase query
            // sees it when semantic_diagnostics runs below.
            self.set_open_text(uri.clone(), text.clone());

            let docs_for_spawn = Arc::clone(&self.docs);
            let diag_cfg = self.config.load().diagnostics.clone();

            // Phase I: semantic analysis on the blocking pool (CPU-bound, hundreds
            // of ms on cold files).  We skip the redundant standalone parse_document
            // call that used to precede this — get_semantic_issues_salsa already
            // parses via salsa internally and caches the ParsedDoc.  Parse errors
            // are read from that cached doc after the blocking task completes.
            let uri_sem = uri.clone();
            let sem_issues = tokio::task::spawn_blocking(move || {
                docs_for_spawn.get_semantic_issues_salsa(&uri_sem)
            })
            .await
            .unwrap_or(None);

            // Extract parse errors from the salsa-cached ParsedDoc.  On the fast
            // path this is a lock-free DashMap lookup — no re-parse.
            let parse_diags = self
                .docs
                .get_doc_salsa(&uri)
                .map(|doc| crate::diagnostics::diagnostics_from_doc(&doc))
                .unwrap_or_default();

            self.set_parse_diagnostics(&uri, parse_diags.clone());
            let stored_source = self.get_open_text(&uri).unwrap_or_default();
            let doc2 = self.get_doc(&uri);
            let dup_decl = doc2
                .as_ref()
                .map(|d| duplicate_declaration_diagnostics(&stored_source, d, &diag_cfg))
                .unwrap_or_default();
            let semantic = sem_issues
                .map(|issues| {
                    crate::semantic_diagnostics::issues_to_diagnostics(&issues, &uri, &diag_cfg)
                })
                .unwrap_or_default();
            let all_diags = merge_file_diagnostics(parse_diags, dup_decl, semantic);
            // Publish for the opened file FIRST — see did_change for why ordering matters.
            self.client
                .publish_diagnostics(uri.clone(), all_diags, None)
                .await;

            // Cross-file republish via the session's parallel re-analysis API.
            // Only files whose Pass-2 actually changed appear in the result —
            // we don't blast every open file with a publish like the old loop.
            let dependents = self.compute_dependent_publishes(&uri, &diag_cfg).await;
            for (dep_uri, dep_diags) in dependents {
                self.client
                    .publish_diagnostics(dep_uri, dep_diags, None)
                    .await;
            }
        })
        .await
    }

    #[tracing::instrument(skip_all)]
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        guard_async("did_change", async move {
            let uri = params.text_document.uri;
            let text = match params.content_changes.into_iter().last() {
                Some(c) => c.text,
                None => return,
            };

            // Store text immediately and capture the version token.
            // Features (completion, hover, …) see the new text instantly while
            // the parse runs in the background.
            let version = self.set_open_text(uri.clone(), text.clone());

            let docs = Arc::clone(&self.docs);
            let open_files = self.open_files.clone();
            let client = self.client.clone();
            let diag_cfg = self.config.load().diagnostics.clone();
            tokio::spawn(async move {
                // 100 ms debounce: if another edit arrives before we parse,
                // the version gate in Backend below will discard this result.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                let (_doc, diagnostics) =
                    tokio::task::spawn_blocking(move || parse_document(&text))
                        .await
                        .unwrap_or_else(|_| (ParsedDoc::default(), vec![]));

                // Only apply if no newer edit arrived while we were parsing.
                // Backend-level gate replaces the old `apply_parse` version check.
                if open_files.current_version(&uri) == Some(version) {
                    open_files.set_parse_diagnostics(&uri, diagnostics.clone());

                    // Phase I: the salsa `semantic_issues` walk is synchronous
                    // and CPU-bound on a cold file — run it on the blocking
                    // pool so the async runtime stays responsive. Returns the
                    // full diagnostic bundle (semantic + dup-decl + deprecated
                    // calls), all computed off-thread.
                    let docs_sem = Arc::clone(&docs);
                    let open_files_sem = open_files.clone();
                    let uri_sem = uri.clone();
                    let diag_cfg_sem = diag_cfg.clone();
                    let (extra_dup, extra_sem) = tokio::task::spawn_blocking(move || {
                        let Some(d) = open_files_sem.get_doc(&docs_sem, &uri_sem) else {
                            return (Vec::<Diagnostic>::new(), Vec::<Diagnostic>::new());
                        };
                        let source = open_files_sem.text(&uri_sem).unwrap_or_default();
                        let dup = duplicate_declaration_diagnostics(&source, &d, &diag_cfg_sem);
                        let sem = docs_sem
                            .get_semantic_issues_salsa(&uri_sem)
                            .map(|issues| {
                                crate::semantic_diagnostics::issues_to_diagnostics(
                                    &issues,
                                    &uri_sem,
                                    &diag_cfg_sem,
                                )
                            })
                            .unwrap_or_default();
                        (dup, sem)
                    })
                    .await
                    .unwrap_or_default();

                    let all_diags = merge_file_diagnostics(diagnostics, extra_dup, extra_sem);
                    // Publish for the changed file FIRST. Test harnesses (and
                    // some clients) consume publishDiagnostics for unrelated
                    // URIs while waiting for one specific URI; reversing this
                    // order would silently swallow the changed file's publish.
                    client
                        .publish_diagnostics(uri.clone(), all_diags, None)
                        .await;

                    // Cross-file republish via the session's parallel
                    // re-analysis API. Only files whose Pass-2 changed are
                    // returned — the old loop blasted every open file.
                    //
                    // Race window: if `other` is being edited concurrently,
                    // its own debounced did_change will still fire a republish,
                    // so any briefly-stale publish here self-corrects within
                    // ~100 ms.
                    let dependents = compute_dependent_publishes_owned(
                        Arc::clone(&docs),
                        open_files.clone(),
                        uri.clone(),
                        diag_cfg.clone(),
                    )
                    .await;
                    for (dep_uri, dep_diags) in dependents {
                        client.publish_diagnostics(dep_uri, dep_diags, None).await;
                    }
                }
            });
        })
        .await
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.close_open_file(&uri);
        // Clear editor diagnostics; the file stays indexed for cross-file features
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn will_save(&self, _params: WillSaveTextDocumentParams) {}

    async fn will_save_wait_until(
        &self,
        params: WillSaveTextDocumentParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let source = self
            .get_open_text(&params.text_document.uri)
            .unwrap_or_default();
        Ok(format_document(&source))
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        // Re-publish diagnostics on save so editors that defer diagnostics
        // until save (rather than on every keystroke) see up-to-date results.
        // Must include semantic diagnostics — publishDiagnostics replaces the
        // prior set entirely, so omitting them would clear errors the editor
        // showed after the last did_change.
        let diag_cfg = self.config.load().diagnostics.clone();
        let all = compute_open_file_diagnostics(&self.docs, &self.open_files, &uri, &diag_cfg);
        self.client.publish_diagnostics(uri, all, None).await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            match change.typ {
                FileChangeType::CREATED | FileChangeType::CHANGED => {
                    if let Ok(path) = change.uri.to_file_path()
                        && let Ok(text) = tokio::fs::read_to_string(&path).await
                    {
                        // Salsa path: index_from_doc mirrors the new text into
                        // the SourceFile input. On the next codebase() call,
                        // salsa re-runs file_definitions for this file and the
                        // aggregator re-folds — no manual remove/collect/finalize.
                        let doc = parse_document_no_diags(&text);
                        self.index_from_doc_if_not_open(change.uri.clone(), &doc);
                    }
                }
                FileChangeType::DELETED => {
                    self.docs.remove(&change.uri);
                }
                _ => {}
            }
        }
        // File changes may affect cross-file features — refresh all live editors.
        send_refresh_requests(&self.client).await;
    }

    #[tracing::instrument(skip_all)]
    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        guard_async_result("completion", async move {
            let uri = &params.text_document_position.text_document.uri;
            let position = params.text_document_position.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(Some(CompletionResponse::Array(vec![]))),
            };
            let other_docs: Vec<Arc<ParsedDoc>> = self
                .docs
                .other_docs(uri, &self.open_urls())
                .into_iter()
                .map(|(_, d)| d)
                .collect();
            let trigger = params
                .context
                .as_ref()
                .and_then(|c| c.trigger_character.as_deref());
            let meta_loaded = self.meta.load();
            let meta_opt = if meta_loaded.is_empty() {
                None
            } else {
                Some(&**meta_loaded)
            };
            let imports = self.file_imports(uri);
            let wi = self.docs.get_workspace_index_salsa();
            let docs_for_lookup = Arc::clone(&self.docs);
            let find_class_doc_fn = move |name: &str| -> Option<Arc<ParsedDoc>> {
                let cr = *wi.classes_by_name.get(name)?.first()?;
                let (uri, _) = wi.at(cr)?;
                docs_for_lookup.get_doc_salsa(uri)
            };
            let analysis = self.docs.cached_analysis(uri);
            let ctx = CompletionCtx {
                source: Some(&source),
                position: Some(position),
                meta: meta_opt,
                doc_uri: Some(uri),
                file_imports: Some(&imports),
                find_class_doc: Some(&find_class_doc_fn),
                analysis: analysis.as_deref(),
            };
            Ok(Some(CompletionResponse::Array(filtered_completions_at(
                &doc,
                &other_docs,
                trigger,
                &ctx,
            ))))
        })
        .await
    }

    async fn completion_resolve(&self, mut item: CompletionItem) -> Result<CompletionItem> {
        if item.documentation.is_some() && item.detail.is_some() {
            return Ok(item);
        }
        // Strip trailing ':' from named-argument labels (e.g. "param:") before lookup.
        let name = item.label.trim_end_matches(':');
        let all_indexes = self.docs.all_indexes();
        if item.detail.is_none()
            && let Some(sig) = signature_for_symbol_from_index(name, &all_indexes)
        {
            item.detail = Some(sig);
        }
        if item.documentation.is_none()
            && let Some(md) = docs_for_symbol_from_index(name, &all_indexes)
        {
            item.documentation = Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }));
        }
        Ok(item)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        guard_async_result("goto_definition", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            // Search current file's ParsedDoc first (fast), then fall back to index search.
            if let Some(loc) = goto_definition(uri, &source, &doc, &[], position) {
                return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
            }
            // Receiver-aware method dispatch: `$var->method()` must jump to the
            // method defined in `$var`'s class hierarchy, not the first `method`
            // found in any indexed file (which would return a wrong class).
            if let Some(line_text) = source.lines().nth(position.line as usize)
                && let Some(word) = crate::util::word_at_position(&source, position)
                && let Some(receiver) = crate::hover::extract_receiver_var_before_cursor(
                    line_text,
                    position.character as usize,
                )
            {
                let class_name = if receiver == "$this" {
                    crate::type_map::enclosing_class_at(&source, &doc, position)
                } else {
                    let tm = crate::type_map::TypeMap::from_doc_at_position(&doc, None, position);
                    tm.get(&receiver).map(|s| s.to_string())
                };
                if let Some(cls) = class_name {
                    let first_cls = cls.split('|').next().unwrap_or(&cls).to_owned();
                    let all_indexes = self.docs.all_indexes();
                    if let Some(loc) =
                        find_method_in_class_hierarchy(&first_cls, &word, &all_indexes)
                    {
                        let refined = self
                            .docs
                            .get_doc_salsa(&loc.uri)
                            .and_then(|doc| {
                                find_declaration_range(doc.source(), &doc, &word).map(|range| {
                                    Location {
                                        uri: loc.uri.clone(),
                                        range,
                                    }
                                })
                            })
                            .unwrap_or(loc);
                        return Ok(Some(GotoDefinitionResponse::Scalar(refined)));
                    }
                }
            }

            // Cross-file: use FileIndex (no disk I/O for background files).
            let other_indexes = self.docs.other_indexes(uri);
            if let Some(word) = crate::util::word_at_position(&source, position)
                && let Some(loc) = find_declaration_in_indexes(&word, &other_indexes)
            {
                let refined = self
                    .docs
                    .get_doc_salsa(&loc.uri)
                    .and_then(|doc| {
                        find_declaration_range(doc.source(), &doc, &word).map(|range| Location {
                            uri: loc.uri.clone(),
                            range,
                        })
                    })
                    .unwrap_or(loc);
                return Ok(Some(GotoDefinitionResponse::Scalar(refined)));
            }

            // PSR-4 fallback: only useful for fully-qualified names (contain `\`)
            if let Some(word) = word_at_position(&source, position)
                && word.contains('\\')
                && let Some(loc) = self.psr4_goto(&word).await
            {
                return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
            }

            Ok(None)
        })
        .await
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        guard_async_result("references", async move {
            let uri = &params.text_document_position.text_document.uri;
            let position = params.text_document_position.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let word = match word_at_position(&source, position) {
                Some(w) => w,
                None => return Ok(None),
            };
            let include_declaration = params.context.include_declaration;

            // Special case: cursor on a class's `__construct` declaration — its
            // call sites are `new OwningClass(...)`, so name-only matching would
            // otherwise return every class's constructor declaration.
            if word == "__construct"
                && let Some(doc) = self.get_doc(uri)
                && let Some(class_name) =
                    class_name_at_construct_decl(doc.source(), &doc.program().stmts, position)
            {
                let locations = self.construct_references(
                    uri,
                    &source,
                    position,
                    &class_name,
                    include_declaration,
                );
                return Ok((!locations.is_empty()).then_some(locations));
            }

            let doc_opt = self.get_doc(uri);
            let (word, kind, constant_owner) =
                resolve_reference_symbol(doc_opt.as_ref(), &source, position, word);
            let all_docs = self.docs.all_docs_for_scan();
            let target_fqn = self.resolve_reference_target_fqn(
                uri,
                doc_opt.as_ref(),
                &word,
                kind,
                position,
                constant_owner,
            );

            // Ensure all workspace files are ingested before querying the
            // session — it only sees files that have been opened, so
            // background-indexed files would otherwise be invisible to
            // `references_to`.
            if matches!(kind, Some(SymbolKind::Method)) {
                self.docs.ensure_all_files_ingested();
            }
            let owner_short: Option<String> = if matches!(kind, Some(SymbolKind::Method)) {
                target_fqn
                    .as_deref()
                    .map(|fqn| fqn_short_name(fqn.trim_start_matches('\\')).to_string())
            } else {
                None
            };

            let session_method_refs = self.session_method_references(
                &word,
                kind,
                target_fqn.as_deref(),
                owner_short.as_deref(),
            );

            let mut locations = if let Some(session_locs) =
                session_method_refs.filter(|l| !l.is_empty())
            {
                // Session results are the authoritative call-site source for
                // methods. Push the cursor's own method-name span as the decl so
                // `include_declaration=true` still surfaces it — derived from the
                // identifier under the cursor (not the raw cursor position) so a
                // mid-identifier cursor doesn't shift the span right.
                let mut combined = session_locs;
                if include_declaration {
                    let range =
                        crate::util::word_range_at(&source, position).unwrap_or_else(|| Range {
                            start: position,
                            end: Position {
                                line: position.line,
                                character: position.character + word.len() as u32,
                            },
                        });
                    combined.push(Location {
                        uri: uri.clone(),
                        range,
                    });
                    crate::references::dedup_ref_locations(&mut combined);
                }
                combined
            } else {
                match target_fqn.as_deref() {
                    Some(t) => {
                        find_references_with_target(&word, &all_docs, include_declaration, kind, t)
                    }
                    None => find_references(&word, &all_docs, include_declaration, kind),
                }
            };

            // For Class / Function kinds: AST walker is authoritative; augment
            // with session refs to catch type-resolved sites the walker misses.
            if !matches!(kind, Some(SymbolKind::Method))
                && let Some(sym) = build_mir_symbol(&word, kind, target_fqn.as_deref())
            {
                let extra = self.docs.session_references_to(&sym);
                if !extra.is_empty() {
                    let mut seen: std::collections::HashSet<(String, u32, u32, u32)> = locations
                        .iter()
                        .map(crate::references::ref_location_key)
                        .collect();
                    for loc in extra
                        .into_iter()
                        .filter_map(crate::references::session_tuple_to_location)
                    {
                        if seen.insert(crate::references::ref_location_key(&loc)) {
                            locations.push(loc);
                        }
                    }
                }
            }

            Ok((!locations.is_empty()).then_some(locations))
        })
        .await
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
        Ok(prepare_rename(&source, params.position).map(PrepareRenameResponse::Range))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let word = match word_at_position(&source, position) {
            Some(w) => w,
            None => return Ok(None),
        };
        if word.starts_with('$') {
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            Ok(Some(rename_variable(
                &word,
                &params.new_name,
                uri,
                &doc,
                position,
            )))
        } else if is_after_arrow(&source, position) {
            let all_docs = self.docs.all_docs_for_scan();
            Ok(Some(rename_property(&word, &params.new_name, &all_docs)))
        } else {
            let all_docs = self.docs.all_docs_for_scan();
            let doc_opt = self.get_doc(uri);
            let target_fqn: Option<String> = doc_opt.as_ref().map(|doc| {
                let imports = self.file_imports(uri);
                crate::moniker::resolve_fqn(doc, &word, &imports)
            });
            Ok(Some(rename(
                &word,
                &params.new_name,
                &all_docs,
                target_fqn.as_deref(),
            )))
        }
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let all_indexes = self.docs.all_indexes();
        Ok(signature_help(&source, &doc, position, &all_indexes))
    }

    #[tracing::instrument(skip_all)]
    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        guard_async_result("hover", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let other_docs = self.docs.other_docs(uri, &self.open_urls());
            let analysis = self.docs.cached_analysis(uri);
            let result = hover_info(&source, &doc, analysis.as_deref(), position, &other_docs);
            if result.is_some() {
                return Ok(result);
            }
            // Fallback: look up the word in the workspace index so class names in
            // extends clauses and parameter types resolve even when their defining
            // file is never opened.  Also try the alias-resolved name so that
            // `use Foo as Bar` works even when Foo is only in the index.
            if let Some(word) = crate::util::word_at_position(&source, position) {
                let wi = self.docs.get_workspace_index_salsa();
                // Try the literal word first.
                if let Some(h) = class_hover_from_index(&word, &wi.files) {
                    return Ok(Some(h));
                }
                // Try alias resolution.
                if let Some(resolved) = crate::hover::resolve_use_alias(&doc.program().stmts, &word)
                    && let Some(h) = class_hover_from_index(&resolved, &wi.files)
                {
                    return Ok(Some(h));
                }
            }
            Ok(None)
        })
        .await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(Some(DocumentSymbolResponse::Nested(document_symbols(
            doc.source(),
            &doc,
        ))))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let ranges = folding_ranges(doc.source(), &doc);
        Ok(if ranges.is_empty() {
            None
        } else {
            Some(ranges)
        })
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let analysis = self.docs.cached_analysis(uri);
        let wi = self.docs.get_workspace_index_salsa();
        Ok(Some(inlay_hints(
            doc.source(),
            &doc,
            analysis.as_deref(),
            params.range,
            &wi.files,
        )))
    }

    async fn inlay_hint_resolve(&self, mut item: InlayHint) -> Result<InlayHint> {
        if item.tooltip.is_some() {
            return Ok(item);
        }
        let func_name = item
            .data
            .as_ref()
            .and_then(|d| d.get("php_lsp_fn"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let Some(name) = func_name {
            let all_indexes = self.docs.all_indexes();
            if let Some(md) = docs_for_symbol_from_index(&name, &all_indexes) {
                item.tooltip = Some(InlayHintTooltip::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: md,
                }));
            }
        }
        Ok(item)
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        // Phase J: read through the salsa-memoized aggregate so repeated
        // workspace-symbol queries (every keystroke in the picker) share the
        // same `Arc` until a file changes.
        let wi = self.docs.get_workspace_index_salsa();
        let results = workspace_symbols_from_workspace(&params.query, &wi);
        Ok(Some(results))
    }

    async fn symbol_resolve(&self, params: WorkspaceSymbol) -> Result<WorkspaceSymbol> {
        // For resolve, we need the full range from the ParsedDoc of open files.
        let docs = self.docs.docs_for(&self.open_urls());
        Ok(resolve_workspace_symbol(params, &docs))
    }

    #[tracing::instrument(skip_all)]
    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        guard_async_result("semantic_tokens_full", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => {
                    return Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                        result_id: None,
                        data: vec![],
                    })));
                }
            };
            let tokens = semantic_tokens(doc.source(), &doc);
            let result_id = token_hash(&tokens);
            let tokens_arc = Arc::new(tokens);
            self.docs
                .store_token_cache(uri, result_id.clone(), Arc::clone(&tokens_arc));
            let data = Arc::try_unwrap(tokens_arc).unwrap_or_else(|arc| (*arc).clone());
            Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: Some(result_id),
                data,
            })))
        })
        .await
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokensRangeResult>> {
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => {
                return Ok(Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
                    result_id: None,
                    data: vec![],
                })));
            }
        };
        let tokens = semantic_tokens_range(doc.source(), &doc, params.range);
        Ok(Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }

    async fn semantic_tokens_full_delta(
        &self,
        params: SemanticTokensDeltaParams,
    ) -> Result<Option<SemanticTokensFullDeltaResult>> {
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };

        let new_tokens = Arc::new(semantic_tokens(doc.source(), &doc));
        let new_result_id = token_hash(&new_tokens);
        let prev_id = &params.previous_result_id;

        let result = match self.docs.get_token_cache(uri, prev_id) {
            Some(old_tokens) => {
                let edits = compute_token_delta(&old_tokens, &new_tokens);
                SemanticTokensFullDeltaResult::TokensDelta(SemanticTokensDelta {
                    result_id: Some(new_result_id.clone()),
                    edits,
                })
            }
            // Unknown previous result — fall back to full tokens
            None => SemanticTokensFullDeltaResult::Tokens(SemanticTokens {
                result_id: Some(new_result_id.clone()),
                data: (*new_tokens).clone(),
            }),
        };

        self.docs.store_token_cache(uri, new_result_id, new_tokens);
        Ok(Some(result))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let ranges = selection_ranges(&doc, &params.positions);
        Ok(if ranges.is_empty() {
            None
        } else {
            Some(ranges)
        })
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let word = match word_at_position(&source, position) {
            Some(w) => w,
            None => return Ok(None),
        };
        let all_docs = self.docs.all_docs_for_scan();
        Ok(prepare_call_hierarchy(&word, &all_docs).map(|item| vec![item]))
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let all_docs = self.docs.all_docs_for_scan();
        let calls = incoming_calls(&params.item, &all_docs);
        Ok(if calls.is_empty() { None } else { Some(calls) })
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        let all_docs = self.docs.all_docs_for_scan();
        let calls = outgoing_calls(&params.item, &all_docs);
        Ok(if calls.is_empty() { None } else { Some(calls) })
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let highlights = document_highlights(&source, &doc, position);
        Ok(if highlights.is_empty() {
            None
        } else {
            Some(highlights)
        })
    }

    async fn linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        // Need the word at the cursor to know if this is a variable rename
        // (`$foo`) — the wordPattern we send back must require/forbid `$`
        // accordingly so that linked-mode typing produces valid PHP.
        let word = match crate::util::word_at_position(&source, position) {
            Some(w) => w,
            None => return Ok(None),
        };
        let is_variable = word.starts_with('$');
        let cursor_word_range = match crate::util::word_range_at(&source, position) {
            Some(r) => r,
            None => return Ok(None),
        };

        // Reuse document_highlights: every occurrence of the symbol is a linked range.
        let highlights = document_highlights(&source, &doc, position);
        if highlights.is_empty() {
            return Ok(None);
        }

        // Bail when the cursor's word isn't itself one of the highlight
        // ranges. `document_highlights` resolves the cursor to a word and
        // walks the AST for occurrences of that name; if the cursor sits in
        // a comment or string literal that happens to share a word with a
        // real identifier, the AST occurrences would still come back and
        // entering linked-edit mode would silently mirror unrelated ranges.
        // Comparing against `word_range_at` (rather than a contains check)
        // also accepts the half-open right boundary — a common cursor
        // position right after typing the name.
        if !highlights.iter().any(|h| h.range == cursor_word_range) {
            return Ok(None);
        }

        // Scope class-member rewrites so that two unrelated classes sharing
        // a method/property/const name aren't linked together — but keep
        // legitimate call sites at module scope (`$obj->bar()` outside any
        // class). The rule: drop highlights that fall inside *another*
        // class than the cursor's. Highlights inside the cursor's class
        // and at module scope (outside every class) are preserved.
        // Class declarations themselves (cursor on the class header) stay
        // global so renaming a class spans the whole file.
        let scope_to_class = !is_variable
            && crate::type_map::enclosing_class_at(&source, &doc, position).as_deref()
                != Some(word.as_str());
        let other_class_ranges: Vec<Range> = if scope_to_class {
            let cursor_class = crate::type_map::enclosing_class_range_at(&doc, position);
            crate::type_map::collect_all_class_ranges(&doc)
                .into_iter()
                .filter(|r| Some(*r) != cursor_class)
                .collect()
        } else {
            Vec::new()
        };
        let ranges: Vec<Range> = highlights
            .into_iter()
            .map(|h| h.range)
            .filter(|r| !other_class_ranges.iter().any(|ocr| range_within(*r, *ocr)))
            .collect();
        if ranges.is_empty() {
            return Ok(None);
        }

        // Variables include the leading `$` in their range, so the pattern
        // must require it; for everything else (class/function/method names)
        // a `$` would produce invalid PHP. The Unicode range covers the
        // full BMP so that PHP identifiers using non-Latin alphabets
        // (CJK, Cyrillic, Greek, …) round-trip through linked-mode
        // typing rather than being rejected by the regex.
        let word_pattern = if is_variable {
            r"\$[a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*".to_string()
        } else {
            r"[a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*".to_string()
        };
        Ok(Some(LinkedEditingRanges {
            ranges,
            word_pattern: Some(word_pattern),
        }))
    }

    async fn goto_implementation(
        &self,
        params: tower_lsp::lsp_types::request::GotoImplementationParams,
    ) -> Result<Option<tower_lsp::lsp_types::request::GotoImplementationResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let imports = self.file_imports(uri);
        let word = crate::util::word_at_position(&source, position).unwrap_or_default();
        let fqn = imports.get(&word).map(|s| s.as_str());
        // First pass: open-file ParsedDocs give accurate character positions.
        let open_docs = self.docs.docs_for(&self.open_urls());
        let mut locs = find_implementations(&word, fqn, &open_docs);
        if locs.is_empty() {
            // Second pass: background files via the salsa-memoized workspace
            // aggregate's `subtypes_of` reverse map (line-only positions).
            let wi = self.docs.get_workspace_index_salsa();
            locs = find_implementations_from_workspace(&word, fqn, &wi);
        }
        if locs.is_empty() {
            Ok(None)
        } else {
            Ok(Some(GotoDefinitionResponse::Array(locs)))
        }
    }

    async fn goto_declaration(
        &self,
        params: tower_lsp::lsp_types::request::GotoDeclarationParams,
    ) -> Result<Option<tower_lsp::lsp_types::request::GotoDeclarationResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        // First pass: open-file ParsedDocs give accurate character positions.
        let open_docs = self.docs.docs_for(&self.open_urls());
        if let Some(loc) = goto_declaration(&source, &open_docs, position) {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }
        // Second pass: background files via FileIndex (line-only positions).
        let all_indexes = self.docs.all_indexes();
        Ok(goto_declaration_from_index(&source, &all_indexes, position)
            .map(GotoDefinitionResponse::Scalar))
    }

    async fn goto_type_definition(
        &self,
        params: tower_lsp::lsp_types::request::GotoTypeDefinitionParams,
    ) -> Result<Option<tower_lsp::lsp_types::request::GotoTypeDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let analysis = self.docs.cached_analysis(uri);
        // First pass: open-file ParsedDocs give accurate character positions.
        let open_docs = self.docs.docs_for(&self.open_urls());
        let mut results =
            goto_type_definition(&source, &doc, analysis.as_deref(), &open_docs, position);

        // If no results from first pass, try background files via FileIndex (line-only positions).
        if results.is_empty() {
            let all_indexes = self.docs.all_indexes();
            results = goto_type_definition_from_index(
                &source,
                &doc,
                analysis.as_deref(),
                &all_indexes,
                position,
            );
        }

        // Format response: scalar for single result, array for multiple, none for empty
        let response = match results.len() {
            0 => None,
            1 => Some(GotoDefinitionResponse::Scalar(
                results.into_iter().next().unwrap(),
            )),
            _ => Some(GotoDefinitionResponse::Array(results)),
        };
        Ok(response)
    }

    async fn prepare_type_hierarchy(
        &self,
        params: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        // Phase J: use the salsa-memoized aggregate's `classes_by_name` map.
        let wi = self.docs.get_workspace_index_salsa();
        Ok(prepare_type_hierarchy_from_workspace(&source, &wi, position).map(|item| vec![item]))
    }

    async fn supertypes(
        &self,
        params: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        // Phase J: resolve parents via the aggregate's `classes_by_name` map.
        let wi = self.docs.get_workspace_index_salsa();
        let result = supertypes_of_from_workspace(&params.item, &wi);
        Ok(if result.is_empty() {
            None
        } else {
            Some(result)
        })
    }

    async fn subtypes(
        &self,
        params: TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        // Phase J: O(matches) lookup via the aggregate's `subtypes_of` map.
        let wi = self.docs.get_workspace_index_salsa();
        let result = subtypes_of_from_workspace(&params.item, &wi);
        Ok(if result.is_empty() {
            None
        } else {
            Some(result)
        })
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let all_docs = self.docs.all_docs_for_scan();
        let lenses = code_lenses(uri, &doc, &all_docs);
        Ok(if lenses.is_empty() {
            None
        } else {
            Some(lenses)
        })
    }

    async fn code_lens_resolve(&self, params: CodeLens) -> Result<CodeLens> {
        // Lenses are fully populated by code_lens; nothing to add.
        Ok(params)
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let links = document_links(uri, &doc, doc.source());
        Ok(if links.is_empty() { None } else { Some(links) })
    }

    async fn document_link_resolve(&self, params: DocumentLink) -> Result<DocumentLink> {
        // Links already carry their target URI; nothing to add.
        Ok(params)
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
        Ok(format_document(&source))
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
        Ok(format_range(&source, params.range))
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document_position.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
        let edits = on_type_format(
            &source,
            params.text_document_position.position,
            &params.ch,
            &params.options,
        );
        Ok(if edits.is_empty() { None } else { Some(edits) })
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        match params.command.as_str() {
            "php-lsp.runTest" => {
                // Arguments: [uri_string, "ClassName::methodName"]
                let file_uri = params
                    .arguments
                    .first()
                    .and_then(|v| v.as_str())
                    .and_then(|s| Url::parse(s).ok());
                let filter = params
                    .arguments
                    .get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let root = self.root_paths.load().first().cloned();
                let client = self.client.clone();

                tokio::spawn(async move {
                    run_phpunit(&client, &filter, root.as_deref(), file_uri.as_ref()).await;
                });

                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn will_rename_files(&self, params: RenameFilesParams) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.load();
        let all_docs = self.docs.all_docs_for_scan();
        let mut merged_changes: std::collections::HashMap<
            tower_lsp::lsp_types::Url,
            Vec<tower_lsp::lsp_types::TextEdit>,
        > = std::collections::HashMap::new();

        for file_rename in &params.files {
            let old_path = tower_lsp::lsp_types::Url::parse(&file_rename.old_uri)
                .ok()
                .and_then(|u| u.to_file_path().ok());
            let new_path = tower_lsp::lsp_types::Url::parse(&file_rename.new_uri)
                .ok()
                .and_then(|u| u.to_file_path().ok());

            let (Some(old_path), Some(new_path)) = (old_path, new_path) else {
                continue;
            };

            let old_fqn = psr4.file_to_fqn(&old_path);
            let new_fqn = psr4.file_to_fqn(&new_path);

            let (Some(old_fqn), Some(new_fqn)) = (old_fqn, new_fqn) else {
                continue;
            };

            let edit = use_edits_for_rename(&old_fqn, &new_fqn, &all_docs);
            if let Some(changes) = edit.changes {
                for (uri, edits) in changes {
                    merged_changes.entry(uri).or_default().extend(edits);
                }
            }
        }

        Ok(if merged_changes.is_empty() {
            None
        } else {
            Some(WorkspaceEdit {
                changes: Some(merged_changes),
                ..Default::default()
            })
        })
    }

    async fn did_rename_files(&self, params: RenameFilesParams) {
        for file_rename in &params.files {
            // Drop the old URI from the index
            if let Ok(old_uri) = tower_lsp::lsp_types::Url::parse(&file_rename.old_uri) {
                self.docs.remove(&old_uri);
            }
            // Index the file at its new location
            if let Ok(new_uri) = tower_lsp::lsp_types::Url::parse(&file_rename.new_uri)
                && let Ok(path) = new_uri.to_file_path()
                && let Ok(text) = tokio::fs::read_to_string(&path).await
            {
                self.index_if_not_open(new_uri, &text);
            }
        }
    }

    // ── File-create notifications ────────────────────────────────────────────

    async fn will_create_files(&self, params: CreateFilesParams) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.load();
        let mut changes: std::collections::HashMap<Url, Vec<TextEdit>> =
            std::collections::HashMap::new();

        for file in &params.files {
            let Ok(uri) = Url::parse(&file.uri) else {
                continue;
            };
            // Check the extension from the URI path so this works on Windows
            // where to_file_path() fails for drive-less URIs (e.g. file:///foo.php).
            if !uri.path().ends_with(".php") {
                continue;
            }

            let stub = if let Ok(path) = uri.to_file_path()
                && let Some(fqn) = psr4.file_to_fqn(&path)
            {
                let (ns, class_name) = match fqn.rfind('\\') {
                    Some(pos) => (&fqn[..pos], &fqn[pos + 1..]),
                    None => ("", fqn.as_str()),
                };
                if ns.is_empty() {
                    format!("<?php\n\ndeclare(strict_types=1);\n\nclass {class_name}\n{{\n}}\n")
                } else {
                    format!(
                        "<?php\n\ndeclare(strict_types=1);\n\nnamespace {ns};\n\nclass {class_name}\n{{\n}}\n"
                    )
                }
            } else {
                "<?php\n\n".to_string()
            };

            changes.insert(
                uri,
                vec![TextEdit {
                    range: Range {
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: 0,
                            character: 0,
                        },
                    },
                    new_text: stub,
                }],
            );
        }

        Ok(if changes.is_empty() {
            None
        } else {
            Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            })
        })
    }

    async fn did_create_files(&self, params: CreateFilesParams) {
        for file in &params.files {
            if let Ok(uri) = Url::parse(&file.uri)
                && let Ok(path) = uri.to_file_path()
                && let Ok(text) = tokio::fs::read_to_string(&path).await
            {
                self.index_if_not_open(uri, &text);
            }
        }
        send_refresh_requests(&self.client).await;
    }

    // ── File-delete notifications ────────────────────────────────────────────

    /// Before a file is deleted, return workspace edits that remove every
    /// `use` import referencing its PSR-4 class name.
    async fn will_delete_files(&self, params: DeleteFilesParams) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.load();
        let all_docs = self.docs.all_docs_for_scan();
        let mut merged_changes: std::collections::HashMap<Url, Vec<TextEdit>> =
            std::collections::HashMap::new();

        for file in &params.files {
            let path = Url::parse(&file.uri)
                .ok()
                .and_then(|u| u.to_file_path().ok());
            let Some(path) = path else { continue };
            let Some(fqn) = psr4.file_to_fqn(&path) else {
                continue;
            };

            let edit = use_edits_for_delete(&fqn, &all_docs);
            if let Some(changes) = edit.changes {
                for (uri, edits) in changes {
                    merged_changes.entry(uri).or_default().extend(edits);
                }
            }
        }

        Ok(if merged_changes.is_empty() {
            None
        } else {
            Some(WorkspaceEdit {
                changes: Some(merged_changes),
                ..Default::default()
            })
        })
    }

    async fn did_delete_files(&self, params: DeleteFilesParams) {
        for file in &params.files {
            if let Ok(uri) = Url::parse(&file.uri) {
                self.docs.remove(&uri);
                // Clear diagnostics for the now-deleted file.
                self.client.publish_diagnostics(uri, vec![], None).await;
            }
        }
        send_refresh_requests(&self.client).await;
    }

    // ── Moniker ──────────────────────────────────────────────────────────────

    async fn moniker(&self, params: MonikerParams) -> Result<Option<Vec<Moniker>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let imports = self.file_imports(uri);
        Ok(moniker_at(&source, &doc, position, &imports).map(|m| vec![m]))
    }

    // ── Inline values ────────────────────────────────────────────────────────

    async fn inline_value(&self, params: InlineValueParams) -> Result<Option<Vec<InlineValue>>> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
        let values = inline_values_in_range(&source, params.range);
        Ok(if values.is_empty() {
            None
        } else {
            Some(values)
        })
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();

        let parse_diags = self.get_parse_diagnostics(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => {
                // Even if document not fully indexed, compute result_id for parse diagnostics
                let _version = self
                    .open_files
                    .all_with_diagnostics()
                    .iter()
                    .find(|(u, _, _)| u == uri)
                    .and_then(|(_, _, v)| *v)
                    .unwrap_or(1);
                let result_id = compute_diagnostic_result_id(&parse_diags, uri.as_str());
                return Ok(DocumentDiagnosticReportResult::Report(
                    DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                        related_documents: None,
                        full_document_diagnostic_report: FullDocumentDiagnosticReport {
                            result_id: Some(result_id),
                            items: parse_diags,
                        },
                    }),
                ));
            }
        };
        let (diag_cfg, php_version) = {
            let cfg = self.config.load();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };
        // Note: php_version could be used for version-specific diagnostics in the future
        let _ = php_version;

        // Phase I: salsa Pass-2 is CPU-bound; run off the async executor.
        let docs = Arc::clone(&self.docs);
        let uri_owned = uri.clone();
        let diag_cfg_sem = diag_cfg.clone();
        let sem_diags = tokio::task::spawn_blocking(move || {
            docs.get_semantic_issues_salsa(&uri_owned)
                .map(|issues| {
                    crate::semantic_diagnostics::issues_to_diagnostics(
                        &issues,
                        &uri_owned,
                        &diag_cfg_sem,
                    )
                })
                .unwrap_or_default()
        })
        .await
        .map_err(|e| {
            use std::borrow::Cow;
            tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::InternalError,
                message: Cow::Owned(format!("diagnostic analysis failed: {}", e)),
                data: None,
            }
        })?;

        let items = merge_file_diagnostics(
            parse_diags,
            duplicate_declaration_diagnostics(&source, &doc, &diag_cfg),
            sem_diags,
        );

        // Generate stable result_id for caching
        let _version = self
            .open_files
            .all_with_diagnostics()
            .iter()
            .find(|(u, _, _)| u == uri)
            .and_then(|(_, _, v)| *v)
            .unwrap_or(1);
        let result_id = compute_diagnostic_result_id(&items, uri.as_str());

        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: Some(result_id),
                    items,
                },
            }),
        ))
    }

    async fn workspace_diagnostic(
        &self,
        params: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReportResult> {
        let all_parse_diags = self.all_open_files_with_diagnostics();
        let (diag_cfg, php_version) = {
            let cfg = self.config.load();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };

        // Note: php_version could be used for version-specific diagnostics in the future
        let _ = php_version;

        // Build a URI→result_id lookup from the client's cached state.
        // Per LSP §3.17.7: files present in this map with a matching result_id
        // should return Unchanged; all others return Full.
        // Duplicate URIs: last-wins (HashMap collect). Clients shouldn't send duplicates,
        // but if they do the last entry wins — safe and simple.
        let previous_map: std::collections::HashMap<Url, String> = params
            .previous_result_ids
            .into_iter()
            .map(|p| (p.uri, p.value))
            .collect();

        // Phase I: each file's semantic issues flow through the salsa
        // `semantic_issues` query. The memo is shared with `did_open` /
        // `did_change` / `document_diagnostic` / `code_action`, so repeated
        // workspace-diagnostic pulls reuse prior analysis. The first pull on
        // a cold workspace still walks every file's `StatementsAnalyzer` —
        // run the whole sweep on the blocking pool so the async runtime
        // stays responsive.
        let docs = Arc::clone(&self.docs);
        let diag_cfg_sweep = diag_cfg.clone();
        let items = tokio::task::spawn_blocking(move || {
            all_parse_diags
                .into_iter()
                .filter_map(|(uri, parse_diags, version)| {
                    let doc = docs.get_doc_salsa(&uri)?;

                    let source = doc.source().to_string();
                    let sem_diags = docs
                        .get_semantic_issues_salsa(&uri)
                        .map(|issues| {
                            crate::semantic_diagnostics::issues_to_diagnostics(
                                &issues,
                                &uri,
                                &diag_cfg_sweep,
                            )
                        })
                        .unwrap_or_default();
                    let all_diags = merge_file_diagnostics(
                        parse_diags,
                        duplicate_declaration_diagnostics(&source, &doc, &diag_cfg_sweep),
                        sem_diags,
                    );

                    let result_id = compute_diagnostic_result_id(&all_diags, uri.as_str());

                    // Per LSP §3.17.7: return Unchanged only when the client already has
                    // this exact result_id cached for this URI; otherwise return Full.
                    if previous_map.get(&uri) == Some(&result_id) {
                        Some(WorkspaceDocumentDiagnosticReport::Unchanged(
                            WorkspaceUnchangedDocumentDiagnosticReport {
                                uri,
                                version,
                                unchanged_document_diagnostic_report:
                                    UnchangedDocumentDiagnosticReport { result_id },
                            },
                        ))
                    } else {
                        Some(WorkspaceDocumentDiagnosticReport::Full(
                            WorkspaceFullDocumentDiagnosticReport {
                                uri,
                                version,
                                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                                    result_id: Some(result_id),
                                    items: all_diags,
                                },
                            },
                        ))
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| {
            use std::borrow::Cow;
            tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::InternalError,
                message: Cow::Owned(format!("workspace_diagnostic analysis failed: {}", e)),
                data: None,
            }
        })?;

        Ok(WorkspaceDiagnosticReportResult::Report(
            WorkspaceDiagnosticReport { items },
        ))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let other_docs = self.docs.other_docs(uri, &self.open_urls());

        // Phase I: read semantic issues through the salsa query. The result
        // is memoized across did_open/did_change/document_diagnostic, so
        // code_action usually hits the memo instead of rerunning analysis.
        // On a memo miss (e.g. code-action fires before did_open finishes),
        // the analyzer runs — park that on the blocking pool so the async
        // runtime doesn't stall.
        let diag_cfg = self.config.load().diagnostics.clone();
        let docs_sem = Arc::clone(&self.docs);
        let uri_sem = uri.clone();
        let diag_cfg_sem = diag_cfg.clone();
        let sem_diags = tokio::task::spawn_blocking(move || {
            docs_sem
                .get_semantic_issues_salsa(&uri_sem)
                .map(|issues| {
                    crate::semantic_diagnostics::issues_to_diagnostics(
                        &issues,
                        &uri_sem,
                        &diag_cfg_sem,
                    )
                })
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();

        // Build "Add use import" code actions for undefined class names in range
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();
        for diag in &sem_diags {
            if diag.code != Some(NumberOrString::String("UndefinedClass".to_string())) {
                continue;
            }
            // Only act on diagnostics within the requested range
            if diag.range.start.line < params.range.start.line
                || diag.range.start.line > params.range.end.line
            {
                continue;
            }
            // Message format: "Class {name} does not exist"
            let class_name = diag
                .message
                .strip_prefix("Class ")
                .and_then(|s| s.strip_suffix(" does not exist"))
                .unwrap_or("")
                .trim();
            if class_name.is_empty() {
                continue;
            }

            // Find a class with this short name in other indexed documents
            for (_other_uri, other_doc) in &other_docs {
                if let Some(fqn) = find_fqn_for_class(other_doc, class_name) {
                    let edit = build_use_import_edit(&source, uri, &fqn);
                    let action = CodeAction {
                        title: format!("Add use {fqn}"),
                        kind: Some(CodeActionKind::QUICKFIX),
                        edit: Some(edit),
                        diagnostics: Some(vec![diag.clone()]),
                        ..Default::default()
                    };
                    actions.push(CodeActionOrCommand::CodeAction(action));
                    break; // one action per undefined symbol
                }
            }
        }

        // Defer edit computation to code_action_resolve so the menu renders
        // instantly; the client fetches the full edit only for the selected item.
        for tag in DEFERRED_ACTION_TAGS {
            actions.extend(defer_actions(
                self.generate_deferred_actions(tag, &source, &doc, params.range, uri),
                tag,
                uri,
                params.range,
            ));
        }

        // Extract variable: cheap, keep eager.
        actions.extend(extract_variable_actions(&source, params.range, uri));
        actions.extend(extract_method_actions(&source, &doc, params.range, uri));
        actions.extend(extract_constant_actions(&source, params.range, uri));
        // Inline variable: inverse of extract variable.
        actions.extend(inline_variable_actions(&source, params.range, uri));
        // Organize imports: sort and remove unused use statements.
        if let Some(action) = organize_imports_action(&source, uri) {
            actions.push(action);
        }

        Ok(if actions.is_empty() {
            None
        } else {
            Some(actions)
        })
    }

    async fn code_action_resolve(&self, item: CodeAction) -> Result<CodeAction> {
        let data = match &item.data {
            Some(d) => d.clone(),
            None => return Ok(item),
        };
        let kind_tag = match data.get("php_lsp_resolve").and_then(|v| v.as_str()) {
            Some(k) => k.to_string(),
            None => return Ok(item),
        };
        let uri: Url = match data
            .get("uri")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok())
        {
            Some(u) => u,
            None => return Ok(item),
        };
        let range: Range = match data
            .get("range")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            Some(r) => r,
            None => return Ok(item),
        };

        let source = self.get_open_text(&uri).unwrap_or_default();
        let doc = match self.get_doc(&uri) {
            Some(d) => d,
            None => return Ok(item),
        };

        let candidates = self.generate_deferred_actions(&kind_tag, &source, &doc, range, &uri);

        // Find the action whose title matches and return it fully resolved.
        for candidate in candidates {
            if let CodeActionOrCommand::CodeAction(ca) = candidate
                && ca.title == item.title
            {
                return Ok(ca);
            }
        }

        Ok(item)
    }
}

/// Shorthand for a `FileOperationRegistrationOptions` that matches `*.php` files.
fn php_file_op() -> FileOperationRegistrationOptions {
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
fn defer_actions(
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
fn is_after_arrow(source: &str, position: Position) -> bool {
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
fn symbol_kind_at(source: &str, position: Position, word: &str) -> Option<SymbolKind> {
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
fn range_within(inner: Range, outer: Range) -> bool {
    let start_ok =
        (inner.start.line, inner.start.character) >= (outer.start.line, outer.start.character);
    let end_ok = (inner.end.line, inner.end.character) <= (outer.end.line, outer.end.character);
    start_ok && end_ok
}

fn position_to_byte_offset(source: &str, position: Position) -> Option<u32> {
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
fn cursor_is_on_method_decl(source: &str, stmts: &[Stmt<'_, '_>], position: Position) -> bool {
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
fn cursor_is_on_property_decl(
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
fn cursor_is_on_constant_decl(
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
fn class_name_at_construct_decl(
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
fn promoted_property_at_cursor(
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
const DEFERRED_ACTION_TAGS: &[&str] = &[
    "phpdoc",
    "implement",
    "constructor",
    "getters_setters",
    "return_type",
    "promote",
];

impl Backend {
    /// Tag → generator mapping for deferred code actions.
    fn generate_deferred_actions(
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
    async fn psr4_goto(&self, fqn: &str) -> Option<Location> {
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
async fn run_phpunit(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DiagnosticsConfig, FeaturesConfig, MAX_INDEXED_FILES};
    use crate::use_import::find_use_insert_line;
    use tower_lsp::lsp_types::{Position, Range, Url};

    // DiagnosticsConfig::from_value tests
    #[test]
    fn diagnostics_config_default_is_enabled() {
        let cfg = DiagnosticsConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.undefined_variables);
        assert!(cfg.undefined_functions);
        assert!(cfg.undefined_classes);
        assert!(cfg.arity_errors);
        assert!(cfg.type_errors);
        assert!(cfg.deprecated_calls);
        assert!(cfg.duplicate_declarations);
    }

    #[test]
    fn diagnostics_config_from_empty_object_is_enabled() {
        let cfg = DiagnosticsConfig::from_value(&serde_json::json!({}));
        assert!(cfg.enabled);
        assert!(cfg.undefined_variables);
    }

    #[test]
    fn diagnostics_config_from_non_object_uses_defaults() {
        let cfg = DiagnosticsConfig::from_value(&serde_json::json!(null));
        assert!(cfg.enabled);
    }

    #[test]
    fn diagnostics_config_can_disable_individual_flags() {
        let cfg = DiagnosticsConfig::from_value(&serde_json::json!({
            "enabled": true,
            "undefinedVariables": false,
            "undefinedFunctions": false,
            "undefinedClasses": true,
            "arityErrors": false,
            "typeErrors": true,
            "deprecatedCalls": false,
            "duplicateDeclarations": true,
        }));
        assert!(cfg.enabled);
        assert!(!cfg.undefined_variables);
        assert!(!cfg.undefined_functions);
        assert!(cfg.undefined_classes);
        assert!(!cfg.arity_errors);
        assert!(cfg.type_errors);
        assert!(!cfg.deprecated_calls);
        assert!(cfg.duplicate_declarations);
    }

    #[test]
    fn diagnostics_config_master_switch_disables_all() {
        let cfg = DiagnosticsConfig::from_value(&serde_json::json!({"enabled": false}));
        assert!(!cfg.enabled);
        // Other flags still have their default values
        assert!(cfg.undefined_variables);
    }

    #[test]
    fn diagnostics_config_master_switch_enables_all() {
        let cfg = DiagnosticsConfig::from_value(&serde_json::json!({"enabled": true}));
        assert!(cfg.enabled);
        assert!(cfg.undefined_variables);
    }

    // LspConfig::from_value tests
    #[test]
    fn lsp_config_default_is_empty() {
        let cfg = LspConfig::default();
        assert!(cfg.php_version.is_none());
        assert!(cfg.exclude_paths.is_empty());
        assert!(cfg.diagnostics.enabled);
    }

    #[test]
    fn lsp_config_parses_php_version() {
        let cfg =
            LspConfig::from_value(&serde_json::json!({"phpVersion": crate::autoload::PHP_8_2}));
        assert_eq!(cfg.php_version.as_deref(), Some(crate::autoload::PHP_8_2));
    }

    #[test]
    fn lsp_config_parses_exclude_paths() {
        let cfg = LspConfig::from_value(&serde_json::json!({
            "excludePaths": ["cache/*", "generated/*"]
        }));
        assert_eq!(cfg.exclude_paths, vec!["cache/*", "generated/*"]);
    }

    #[test]
    fn lsp_config_parses_include_paths() {
        let cfg = LspConfig::from_value(&serde_json::json!({
            "includePaths": ["vendor/yiisoft"]
        }));
        assert_eq!(cfg.include_paths, vec!["vendor/yiisoft"]);
    }

    #[test]
    fn lsp_config_parses_both_exclude_and_include_paths() {
        let cfg = LspConfig::from_value(&serde_json::json!({
            "excludePaths": ["cache/*", "logs/*"],
            "includePaths": ["vendor/yiisoft"]
        }));
        assert_eq!(cfg.exclude_paths, vec!["cache/*", "logs/*"]);
        assert_eq!(cfg.include_paths, vec!["vendor/yiisoft"]);
    }

    #[test]
    fn lsp_config_parses_diagnostics_section() {
        let cfg = LspConfig::from_value(&serde_json::json!({
            "diagnostics": {"enabled": false}
        }));
        assert!(!cfg.diagnostics.enabled);
    }

    #[test]
    fn lsp_config_ignores_missing_fields() {
        let cfg = LspConfig::from_value(&serde_json::json!({}));
        assert!(cfg.php_version.is_none());
        assert!(cfg.exclude_paths.is_empty());
    }

    #[test]
    fn lsp_config_parses_max_indexed_files() {
        let cfg = LspConfig::from_value(&serde_json::json!({"maxIndexedFiles": 5000}));
        assert_eq!(cfg.max_indexed_files, 5000);
    }

    #[test]
    fn lsp_config_default_max_indexed_files() {
        let cfg = LspConfig::default();
        assert_eq!(cfg.max_indexed_files, MAX_INDEXED_FILES);
    }

    // FeaturesConfig tests
    #[test]
    fn features_config_default_all_enabled() {
        let cfg = FeaturesConfig::default();
        assert!(cfg.completion);
        assert!(cfg.hover);
        assert!(cfg.definition);
        assert!(cfg.declaration);
        assert!(cfg.references);
        assert!(cfg.document_symbols);
        assert!(cfg.workspace_symbols);
        assert!(cfg.rename);
        assert!(cfg.signature_help);
        assert!(cfg.inlay_hints);
        assert!(cfg.semantic_tokens);
        assert!(cfg.selection_range);
        assert!(cfg.call_hierarchy);
        assert!(cfg.document_highlight);
        assert!(cfg.implementation);
        assert!(cfg.code_action);
        assert!(cfg.type_definition);
        assert!(cfg.code_lens);
        assert!(cfg.formatting);
        assert!(cfg.range_formatting);
        assert!(cfg.on_type_formatting);
        assert!(cfg.document_link);
        assert!(cfg.linked_editing_range);
        assert!(cfg.inline_values);
    }

    #[test]
    fn features_config_from_empty_object_all_enabled() {
        let cfg = FeaturesConfig::from_value(&serde_json::json!({}));
        assert!(cfg.completion);
        assert!(cfg.hover);
        assert!(cfg.call_hierarchy);
        assert!(cfg.inline_values);
    }

    #[test]
    fn features_config_can_disable_individual_flags() {
        let cfg = FeaturesConfig::from_value(&serde_json::json!({
            "callHierarchy": false,
        }));
        assert!(!cfg.call_hierarchy);
        assert!(cfg.completion);
        assert!(cfg.hover);
        assert!(cfg.definition);
        assert!(cfg.inline_values);
    }

    #[test]
    fn lsp_config_parses_features_section() {
        let cfg = LspConfig::from_value(&serde_json::json!({
            "features": {"callHierarchy": false}
        }));
        assert!(!cfg.features.call_hierarchy);
        assert!(cfg.features.completion);
        assert!(cfg.features.hover);
    }

    // find_use_insert_line tests
    #[test]
    fn find_use_insert_line_after_php_open_tag() {
        let src = "<?php\nfunction foo() {}";
        assert_eq!(find_use_insert_line(src), 1);
    }

    #[test]
    fn find_use_insert_line_after_existing_use() {
        let src = "<?php\nuse Foo\\Bar;\nuse Baz\\Qux;\nclass Impl {}";
        assert_eq!(find_use_insert_line(src), 3);
    }

    #[test]
    fn find_use_insert_line_after_namespace() {
        let src = "<?php\nnamespace App\\Services;\nclass Service {}";
        assert_eq!(find_use_insert_line(src), 2);
    }

    #[test]
    fn find_use_insert_line_after_namespace_and_use() {
        let src = "<?php\nnamespace App;\nuse Foo\\Bar;\nclass Impl {}";
        assert_eq!(find_use_insert_line(src), 3);
    }

    #[test]
    fn find_use_insert_line_empty_file() {
        assert_eq!(find_use_insert_line(""), 0);
    }

    // is_after_arrow tests
    #[test]
    fn is_after_arrow_with_method_call() {
        let src = "<?php\n$obj->method();\n";
        // Position after `->m` i.e. on `method` — character 6 (after `$obj->`)
        let pos = Position {
            line: 1,
            character: 6,
        };
        assert!(is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_without_arrow() {
        let src = "<?php\n$obj->method();\n";
        // Position on `$obj` — not after arrow
        let pos = Position {
            line: 1,
            character: 1,
        };
        assert!(!is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_on_standalone_identifier() {
        let src = "<?php\nfunction greet() {}\n";
        let pos = Position {
            line: 1,
            character: 10,
        };
        assert!(!is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_out_of_bounds_line() {
        let src = "<?php\n$x = 1;\n";
        let pos = Position {
            line: 99,
            character: 0,
        };
        assert!(!is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_at_start_of_property() {
        let src = "<?php\n$this->name;\n";
        // `name` starts at character 7 (after `$this->`)
        let pos = Position {
            line: 1,
            character: 7,
        };
        assert!(is_after_arrow(src, pos));
    }

    // php_file_op tests
    #[test]
    fn php_file_op_matches_php_files() {
        let op = php_file_op();
        assert_eq!(op.filters.len(), 1);
        let filter = &op.filters[0];
        assert_eq!(filter.scheme.as_deref(), Some("file"));
        assert_eq!(filter.pattern.glob, "**/*.php");
    }

    // defer_actions tests
    #[test]
    fn defer_actions_strips_edit_and_adds_data() {
        let uri = Url::parse("file:///test.php").unwrap();
        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 5,
            },
        };
        let actions = vec![CodeActionOrCommand::CodeAction(CodeAction {
            title: "My Action".to_string(),
            kind: Some(CodeActionKind::REFACTOR),
            edit: Some(WorkspaceEdit::default()),
            data: None,
            ..Default::default()
        })];
        let deferred = defer_actions(actions, "test_kind", &uri, range);
        assert_eq!(deferred.len(), 1);
        if let CodeActionOrCommand::CodeAction(ca) = &deferred[0] {
            assert!(ca.edit.is_none(), "edit should be stripped");
            assert!(ca.data.is_some(), "data payload should be set");
            let data = ca.data.as_ref().unwrap();
            assert_eq!(data["php_lsp_resolve"], "test_kind");
            assert_eq!(data["uri"], uri.to_string());
        } else {
            panic!("expected CodeAction");
        }
    }

    // build_use_import_edit tests
    #[test]
    fn build_use_import_edit_inserts_after_php_tag() {
        let src = "<?php\nclass Foo {}";
        let uri = Url::parse("file:///test.php").unwrap();
        let edit = build_use_import_edit(src, &uri, "App\\Services\\Bar");
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "use App\\Services\\Bar;\n");
        assert_eq!(edits[0].range.start.line, 1);
    }

    #[test]
    fn build_use_import_edit_inserts_after_existing_use() {
        let src = "<?php\nuse Foo\\Bar;\nclass Impl {}";
        let uri = Url::parse("file:///test.php").unwrap();
        let edit = build_use_import_edit(src, &uri, "Baz\\Qux");
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        assert_eq!(edits[0].range.start.line, 2);
        assert_eq!(edits[0].new_text, "use Baz\\Qux;\n");
    }

    // Extraction logic for "Add use import" code action — matches IssueKind::UndefinedClass message format
    #[test]
    fn undefined_class_name_extracted_from_message() {
        let msg = "Class MyService does not exist";
        let name = msg
            .strip_prefix("Class ")
            .and_then(|s| s.strip_suffix(" does not exist"))
            .unwrap_or("")
            .trim();
        assert_eq!(name, "MyService");
    }

    #[test]
    fn undefined_function_message_not_matched_by_extraction() {
        // UndefinedFunction message format must NOT match the UndefinedClass extraction,
        // ensuring code action is not offered for undefined functions.
        let msg = "Function myHelper() is not defined";
        let name = msg
            .strip_prefix("Class ")
            .and_then(|s| s.strip_suffix(" does not exist"))
            .unwrap_or("")
            .trim();
        assert!(
            name.is_empty(),
            "function diagnostic should not extract a class name"
        );
    }

    // ── position_to_byte_offset ──────────────────────────────────────────────

    #[test]
    fn position_to_byte_offset_first_line() {
        let src = "<?php\nfoo();";
        // Character 0 → byte 0.
        assert_eq!(
            position_to_byte_offset(
                src,
                Position {
                    line: 0,
                    character: 0
                }
            ),
            Some(0)
        );
        // Character 4 → byte 4 (last char 'p' of "<?php").
        assert_eq!(
            position_to_byte_offset(
                src,
                Position {
                    line: 0,
                    character: 4
                }
            ),
            Some(4)
        );
        // Character 5 is past the end of "<?php" (5 chars) — clamps to line_content.len().
        assert_eq!(
            position_to_byte_offset(
                src,
                Position {
                    line: 0,
                    character: 5
                }
            ),
            Some(5)
        );
    }

    #[test]
    fn position_to_byte_offset_second_line() {
        let src = "<?php\nfoo();";
        // Start of line 1 is byte 6 (after "<?php\n").
        assert_eq!(
            position_to_byte_offset(
                src,
                Position {
                    line: 1,
                    character: 0
                }
            ),
            Some(6)
        );
        // "foo" ends at character 3 → byte 9.
        assert_eq!(
            position_to_byte_offset(
                src,
                Position {
                    line: 1,
                    character: 3
                }
            ),
            Some(9)
        );
    }

    #[test]
    fn position_to_byte_offset_line_boundary_returns_none() {
        // A source with exactly one line has only line 0; line 1 must return None.
        let src = "<?php";
        assert_eq!(
            position_to_byte_offset(
                src,
                Position {
                    line: 1,
                    character: 0
                }
            ),
            None
        );
        assert_eq!(
            position_to_byte_offset(
                src,
                Position {
                    line: 5,
                    character: 0
                }
            ),
            None
        );
    }

    // ── cursor_is_on_method_decl ─────────────────────────────────────────────

    #[test]
    fn cursor_on_method_decl_name_returns_true() {
        // "    public function add() {}" — "add" is cols 20-22 on line 2.
        // Use doc.source() so str_offset uses pointer arithmetic (production path).
        let doc = ParsedDoc::parse("<?php\nclass C {\n    public function add() {}\n}".to_string());
        let source = doc.source();
        let stmts = &doc.program().stmts;
        // All three characters of "add" must match.
        for col in 20u32..=22 {
            assert!(
                cursor_is_on_method_decl(
                    source,
                    stmts,
                    Position {
                        line: 2,
                        character: col
                    }
                ),
                "expected true at col {col}"
            );
        }
        // One before and one after must not match.
        assert!(!cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 2,
                character: 19
            }
        ));
        assert!(!cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 2,
                character: 23
            }
        ));
    }

    #[test]
    fn cursor_on_free_function_decl_returns_false() {
        // "add" at col 9 on line 1 is a free function — not a method.
        let doc = ParsedDoc::parse("<?php\nfunction add() {}".to_string());
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(!cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 1,
                character: 9
            }
        ));
    }

    #[test]
    fn cursor_on_method_call_site_returns_false() {
        // "$c->add()" — "add" at col 4 on line 3 is a call site, not a declaration.
        let doc = ParsedDoc::parse(
            "<?php\nclass C { public function add() {} }\n$c = new C();\n$c->add();".to_string(),
        );
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(!cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 3,
                character: 4
            }
        ));
    }

    #[test]
    fn cursor_on_interface_method_decl_returns_true() {
        // "    public function add(): void;" — "add" starts at col 20 on line 2.
        let doc = ParsedDoc::parse(
            "<?php\ninterface I {\n    public function add(): void;\n}".to_string(),
        );
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 2,
                character: 20
            }
        ));
    }

    #[test]
    fn cursor_on_trait_method_decl_returns_true() {
        // "    public function add() {}" — "add" starts at col 20 on line 2.
        let doc = ParsedDoc::parse("<?php\ntrait T {\n    public function add() {}\n}".to_string());
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 2,
                character: 20
            }
        ));
    }

    #[test]
    fn cursor_on_enum_method_decl_returns_true() {
        // "    public function label(): string {}" — "label" starts at col 20 on line 2.
        let doc = ParsedDoc::parse(
            "<?php\nenum Status {\n    public function label(): string { return 'x'; }\n}"
                .to_string(),
        );
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 2,
                character: 20
            }
        ));
    }

    #[test]
    fn cursor_on_method_decl_in_unbraced_namespace_returns_true() {
        // Unbraced (Simple) namespace: the class is a top-level sibling of the
        // namespace statement, not nested inside it.
        //
        // Line 0: <?php
        // Line 1: namespace App;
        // Line 2: class C {
        // Line 3:     public function add() {}   ← "add" starts at col 20
        // Line 4: }
        let doc = ParsedDoc::parse(
            "<?php\nnamespace App;\nclass C {\n    public function add() {}\n}".to_string(),
        );
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(
            cursor_is_on_method_decl(
                source,
                stmts,
                Position {
                    line: 3,
                    character: 20
                }
            ),
            "method in unbraced namespace must be detected"
        );
    }

    #[test]
    fn cursor_on_method_decl_in_braced_namespace_returns_true() {
        // Braced namespace: the class is nested inside NamespaceBody::Braced.
        //
        // Line 0: <?php
        // Line 1: namespace App {
        // Line 2:     class C {
        // Line 3:         public function add() {}   ← "add" starts at col 24
        // Line 4:     }
        // Line 5: }
        let doc = ParsedDoc::parse(
            "<?php\nnamespace App {\n    class C {\n        public function add() {}\n    }\n}"
                .to_string(),
        );
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(
            cursor_is_on_method_decl(
                source,
                stmts,
                Position {
                    line: 3,
                    character: 24
                }
            ),
            "method in braced namespace must be detected"
        );
    }

    // --- LspConfig::merge_project_configs ---

    #[test]
    fn merge_file_only_uses_file_values() {
        let file = serde_json::json!({
            "phpVersion": "8.1",
            "excludePaths": ["vendor/*"],
            "maxIndexedFiles": 500,
        });
        let merged = LspConfig::merge_project_configs(Some(&file), None);
        let cfg = LspConfig::from_value(&merged);
        assert_eq!(cfg.php_version, Some("8.1".to_string()));
        assert_eq!(cfg.exclude_paths, vec!["vendor/*"]);
        assert_eq!(cfg.max_indexed_files, 500);
    }

    #[test]
    fn merge_editor_wins_per_key_over_file() {
        let file = serde_json::json!({"phpVersion": "8.1", "maxIndexedFiles": 100});
        let editor = serde_json::json!({"phpVersion": "8.3", "maxIndexedFiles": 200});
        let merged = LspConfig::merge_project_configs(Some(&file), Some(&editor));
        let cfg = LspConfig::from_value(&merged);
        assert_eq!(cfg.php_version, Some("8.3".to_string()));
        assert_eq!(cfg.max_indexed_files, 200);
    }

    #[test]
    fn merge_exclude_paths_concat_not_replace() {
        let file = serde_json::json!({"excludePaths": ["cache/*"]});
        let editor = serde_json::json!({"excludePaths": ["logs/*"]});
        let merged = LspConfig::merge_project_configs(Some(&file), Some(&editor));
        let cfg = LspConfig::from_value(&merged);
        // File entries come first, editor entries appended.
        assert_eq!(cfg.exclude_paths, vec!["cache/*", "logs/*"]);
    }

    #[test]
    fn merge_include_paths_concat_not_replace() {
        let file = serde_json::json!({"includePaths": ["vendor/yiisoft"]});
        let editor = serde_json::json!({"includePaths": ["vendor/symfony"]});
        let merged = LspConfig::merge_project_configs(Some(&file), Some(&editor));
        let cfg = LspConfig::from_value(&merged);
        // File entries come first, editor entries appended.
        assert_eq!(cfg.include_paths, vec!["vendor/yiisoft", "vendor/symfony"]);
    }

    #[test]
    fn merge_no_file_uses_editor_only() {
        let editor = serde_json::json!({"phpVersion": "8.2", "excludePaths": ["tmp/*"]});
        let merged = LspConfig::merge_project_configs(None, Some(&editor));
        let cfg = LspConfig::from_value(&merged);
        assert_eq!(cfg.php_version, Some("8.2".to_string()));
        assert_eq!(cfg.exclude_paths, vec!["tmp/*"]);
    }

    #[test]
    fn merge_both_none_returns_defaults() {
        let merged = LspConfig::merge_project_configs(None, None);
        let cfg = LspConfig::from_value(&merged);
        assert!(cfg.php_version.is_none());
        assert!(cfg.exclude_paths.is_empty());
        assert_eq!(cfg.max_indexed_files, MAX_INDEXED_FILES);
    }

    #[test]
    fn merge_file_editor_both_have_exclude_paths_all_present() {
        let file = serde_json::json!({"excludePaths": ["a/*", "b/*"]});
        let editor = serde_json::json!({"excludePaths": ["c/*"]});
        let merged = LspConfig::merge_project_configs(Some(&file), Some(&editor));
        let cfg = LspConfig::from_value(&merged);
        assert_eq!(cfg.exclude_paths, vec!["a/*", "b/*", "c/*"]);
    }
}
