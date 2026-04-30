use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use dashmap::DashMap;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::notification::Progress as ProgressNotification;

/// Sent to the client once Phase 3 (reference index build) finishes.
/// Allows tests and tooling to wait for the codebase fast path to be active.
enum IndexReadyNotification {}
impl tower_lsp::lsp_types::notification::Notification for IndexReadyNotification {
    type Params = ();
    const METHOD: &'static str = "$/php-lsp/indexReady";
}
use tower_lsp::lsp_types::request::{
    CodeLensRefresh, InlayHintRefreshRequest, InlineValueRefreshRequest, SemanticTokensRefresh,
    WorkDoneProgressCreate, WorkspaceDiagnosticRefresh,
};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, async_trait};

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};

use crate::ast::{ParsedDoc, str_offset};
use crate::autoload::Psr4Map;
use crate::call_hierarchy::{incoming_calls, outgoing_calls, prepare_call_hierarchy};
use crate::code_lens::code_lenses;
use crate::completion::{CompletionCtx, filtered_completions_at};
use crate::declaration::{goto_declaration, goto_declaration_from_index};
use crate::definition::{
    find_declaration_range, find_in_indexes, find_method_in_class_hierarchy, goto_definition,
};
use crate::diagnostics::parse_document;
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
use crate::organize_imports::organize_imports_action;
use crate::phpdoc_action::phpdoc_actions;
use crate::phpstorm_meta::PhpStormMeta;
use crate::promote_action::promote_constructor_actions;
use crate::references::{
    SymbolKind, find_constructor_references, find_references, find_references_codebase_with_target,
    find_references_with_target,
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
use crate::util::word_at;

/// Per-category diagnostic toggle flags.
/// The master `enabled` switch defaults to `true`. Individual category flags
/// also default to `true`, so all diagnostics are on out of the box; set
/// `initializationOptions.diagnostics.enabled = false` to silence everything,
/// or turn off specific categories individually.
#[derive(Debug, Clone)]
pub struct DiagnosticsConfig {
    /// Master switch: when `false`, no diagnostics are emitted. Defaults to `true`.
    pub enabled: bool,
    /// Undefined variable references.
    pub undefined_variables: bool,
    /// Calls to undefined functions.
    pub undefined_functions: bool,
    /// References to undefined classes / interfaces / traits.
    pub undefined_classes: bool,
    /// Wrong number of arguments passed to a function.
    pub arity_errors: bool,
    /// Return-type mismatches.
    pub type_errors: bool,
    /// Calls to `@deprecated` members.
    pub deprecated_calls: bool,
    /// Duplicate class / function declarations.
    pub duplicate_declarations: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        DiagnosticsConfig {
            enabled: true,
            undefined_variables: true,
            undefined_functions: true,
            undefined_classes: true,
            arity_errors: true,
            type_errors: true,
            deprecated_calls: true,
            duplicate_declarations: true,
        }
    }
}

impl DiagnosticsConfig {
    /// All categories on. Used in tests and by clients that explicitly enable
    /// diagnostics without overriding individual flags.
    #[cfg(test)]
    pub fn all_enabled() -> Self {
        DiagnosticsConfig {
            enabled: true,
            ..DiagnosticsConfig::default()
        }
    }

    fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = DiagnosticsConfig::default();
        let Some(obj) = v.as_object() else { return cfg };
        let flag = |key: &str| obj.get(key).and_then(|x| x.as_bool()).unwrap_or(true);
        cfg.enabled = obj.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
        cfg.undefined_variables = flag("undefinedVariables");
        cfg.undefined_functions = flag("undefinedFunctions");
        cfg.undefined_classes = flag("undefinedClasses");
        cfg.arity_errors = flag("arityErrors");
        cfg.type_errors = flag("typeErrors");
        cfg.deprecated_calls = flag("deprecatedCalls");
        cfg.duplicate_declarations = flag("duplicateDeclarations");
        cfg
    }
}

/// Per-feature capability toggles. All default to `true` (enabled).
/// Set `initializationOptions.features.<name> = false` to suppress a capability.
#[derive(Debug, Clone)]
pub struct FeaturesConfig {
    pub completion: bool,
    pub hover: bool,
    pub definition: bool,
    pub declaration: bool,
    pub references: bool,
    pub document_symbols: bool,
    pub workspace_symbols: bool,
    pub rename: bool,
    pub signature_help: bool,
    pub inlay_hints: bool,
    pub semantic_tokens: bool,
    pub selection_range: bool,
    pub call_hierarchy: bool,
    pub document_highlight: bool,
    pub implementation: bool,
    pub code_action: bool,
    pub type_definition: bool,
    pub code_lens: bool,
    pub formatting: bool,
    pub range_formatting: bool,
    pub on_type_formatting: bool,
    pub document_link: bool,
    pub linked_editing_range: bool,
    pub inline_values: bool,
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        FeaturesConfig {
            completion: true,
            hover: true,
            definition: true,
            declaration: true,
            references: true,
            document_symbols: true,
            workspace_symbols: true,
            rename: true,
            signature_help: true,
            inlay_hints: true,
            semantic_tokens: true,
            selection_range: true,
            call_hierarchy: true,
            document_highlight: true,
            implementation: true,
            code_action: true,
            type_definition: true,
            code_lens: true,
            formatting: true,
            range_formatting: true,
            on_type_formatting: true,
            document_link: true,
            linked_editing_range: true,
            inline_values: true,
        }
    }
}

impl FeaturesConfig {
    fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = FeaturesConfig::default();
        let Some(obj) = v.as_object() else { return cfg };
        let flag = |key: &str| obj.get(key).and_then(|x| x.as_bool()).unwrap_or(true);
        cfg.completion = flag("completion");
        cfg.hover = flag("hover");
        cfg.definition = flag("definition");
        cfg.declaration = flag("declaration");
        cfg.references = flag("references");
        cfg.document_symbols = flag("documentSymbols");
        cfg.workspace_symbols = flag("workspaceSymbols");
        cfg.rename = flag("rename");
        cfg.signature_help = flag("signatureHelp");
        cfg.inlay_hints = flag("inlayHints");
        cfg.semantic_tokens = flag("semanticTokens");
        cfg.selection_range = flag("selectionRange");
        cfg.call_hierarchy = flag("callHierarchy");
        cfg.document_highlight = flag("documentHighlight");
        cfg.implementation = flag("implementation");
        cfg.code_action = flag("codeAction");
        cfg.type_definition = flag("typeDefinition");
        cfg.code_lens = flag("codeLens");
        cfg.formatting = flag("formatting");
        cfg.range_formatting = flag("rangeFormatting");
        cfg.on_type_formatting = flag("onTypeFormatting");
        cfg.document_link = flag("documentLink");
        cfg.linked_editing_range = flag("linkedEditingRange");
        cfg.inline_values = flag("inlineValues");
        cfg
    }
}

/// Configuration received from the client via `initializationOptions`.
#[derive(Debug, Clone)]
pub struct LspConfig {
    /// PHP version string, e.g. `"8.1"`.  Set explicitly via `initializationOptions`
    /// or auto-detected from `composer.json` / the `php` binary at startup.
    pub php_version: Option<String>,
    /// Glob patterns for paths to exclude from workspace indexing.
    pub exclude_paths: Vec<String>,
    /// Per-category diagnostic toggles.
    pub diagnostics: DiagnosticsConfig,
    /// Per-feature capability toggles.
    pub features: FeaturesConfig,
    /// Hard cap on the number of PHP files indexed during a workspace scan.
    /// Defaults to [`MAX_INDEXED_FILES`]. Set lower via `initializationOptions`
    /// to reduce memory on projects with very large vendor trees.
    pub max_indexed_files: usize,
}

impl Default for LspConfig {
    fn default() -> Self {
        LspConfig {
            php_version: None,
            exclude_paths: Vec::new(),
            diagnostics: DiagnosticsConfig::default(),
            features: FeaturesConfig::default(),
            max_indexed_files: MAX_INDEXED_FILES,
        }
    }
}

impl LspConfig {
    /// Merge a `.php-lsp.json` value with editor `initializationOptions` /
    /// `workspace/configuration`. Editor settings win per-key; `excludePaths`
    /// arrays are **concatenated** (file entries first, editor entries appended)
    /// rather than replaced, since exclusion patterns are additive.
    ///
    /// Hot-reload of `.php-lsp.json` on file change is not supported; the file
    /// is only read during `initialize` and `did_change_configuration`.
    pub fn merge_project_configs(
        file: Option<&serde_json::Value>,
        editor: Option<&serde_json::Value>,
    ) -> serde_json::Value {
        let mut merged = file
            .cloned()
            .unwrap_or(serde_json::Value::Object(Default::default()));
        let Some(editor_obj) = editor.and_then(|e| e.as_object()) else {
            return merged;
        };
        let merged_obj = merged
            .as_object_mut()
            .expect("merged base is always an object");
        for (key, val) in editor_obj {
            if key == "excludePaths" {
                let file_arr = merged_obj
                    .get("excludePaths")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let editor_arr = val.as_array().cloned().unwrap_or_default();
                merged_obj.insert(
                    key.clone(),
                    serde_json::Value::Array([file_arr, editor_arr].concat()),
                );
            } else {
                merged_obj.insert(key.clone(), val.clone());
            }
        }
        merged
    }

    fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = LspConfig::default();
        if let Some(ver) = v.get("phpVersion").and_then(|x| x.as_str())
            && crate::autoload::is_valid_php_version(ver)
        {
            cfg.php_version = Some(ver.to_string());
        }
        if let Some(arr) = v.get("excludePaths").and_then(|x| x.as_array()) {
            cfg.exclude_paths = arr
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect();
        }
        if let Some(diag_val) = v.get("diagnostics") {
            cfg.diagnostics = DiagnosticsConfig::from_value(diag_val);
        }
        if let Some(feat_val) = v.get("features") {
            cfg.features = FeaturesConfig::from_value(feat_val);
        }
        if let Some(n) = v.get("maxIndexedFiles").and_then(|x| x.as_u64()) {
            cfg.max_indexed_files = n as usize;
        }
        cfg
    }
}

/// Per-open-file state owned by `Backend` (Phase E4).
///
/// Previously this lived inside `DocumentStore`'s `map: DashMap<Url, Document>`,
/// but none of these fields are salsa-shaped: `text` is the live editor buffer,
/// `version` is an async-parse gate, and `parse_diagnostics` is a publish cache.
/// Keeping them on `Backend` leaves `DocumentStore` as a pure salsa-input wrapper.
#[derive(Default, Clone)]
struct OpenFile {
    /// Live editor text.
    text: String,
    /// Monotonic counter bumped on every `set_open_text` / `close_open_file`;
    /// used to discard stale async parse results.
    version: u64,
    /// Parse-level diagnostics most recently cached for publication.
    parse_diagnostics: Vec<Diagnostic>,
}

/// Shared handle to open-file state. Cheaply cloneable — wraps an `Arc<DashMap>`
/// so it can be captured by async closures alongside `Arc<DocumentStore>`.
#[derive(Clone, Default)]
pub struct OpenFiles(Arc<DashMap<Url, OpenFile>>);

impl OpenFiles {
    fn new() -> Self {
        Self::default()
    }

    fn set_open_text(&self, docs: &DocumentStore, uri: Url, text: String) -> u64 {
        docs.mirror_text(&uri, &text);
        let mut entry = self.0.entry(uri).or_default();
        entry.version += 1;
        entry.text = text;
        entry.version
    }

    fn close(&self, docs: &DocumentStore, uri: &Url) {
        self.0.remove(uri);
        docs.evict_token_cache(uri);
    }

    fn current_version(&self, uri: &Url) -> Option<u64> {
        self.0.get(uri).map(|e| e.version)
    }

    fn text(&self, uri: &Url) -> Option<String> {
        self.0.get(uri).map(|e| e.text.clone())
    }

    fn set_parse_diagnostics(&self, uri: &Url, diagnostics: Vec<Diagnostic>) {
        if let Some(mut entry) = self.0.get_mut(uri) {
            entry.parse_diagnostics = diagnostics;
        }
    }

    fn parse_diagnostics(&self, uri: &Url) -> Option<Vec<Diagnostic>> {
        self.0.get(uri).map(|e| e.parse_diagnostics.clone())
    }

    fn all_with_diagnostics(&self) -> Vec<(Url, Vec<Diagnostic>, Option<i64>)> {
        self.0
            .iter()
            .map(|e| {
                (
                    e.key().clone(),
                    e.value().parse_diagnostics.clone(),
                    Some(e.value().version as i64),
                )
            })
            .collect()
    }

    fn urls(&self) -> Vec<Url> {
        self.0.iter().map(|e| e.key().clone()).collect()
    }

    fn contains(&self, uri: &Url) -> bool {
        self.0.contains_key(uri)
    }

    /// Open-gated parsed doc: returns `Some` only when `uri` is currently open.
    fn get_doc(&self, docs: &DocumentStore, uri: &Url) -> Option<Arc<ParsedDoc>> {
        if !self.contains(uri) {
            return None;
        }
        docs.get_doc_salsa(uri)
    }
}

/// Build the full diagnostic bundle for an already-open file.
///
/// Reuses cached parse diagnostics from `OpenFiles` (set by the file's own
/// debounced parse) and recomputes the rest:
/// - `duplicate_declaration_diagnostics` is intra-file (AST walk over the
///   doc's own statements), so a dependency change does NOT change its
///   result — but it's cheap and keeps this helper a single source of
///   truth for "the diagnostic bundle for `uri`".
/// - `semantic_issues` is salsa-cached; for files unaffected by the
///   triggering change it's a cache hit.
///
/// Used both for the originating file (during `did_open`/`did_change`) and
/// when proactively republishing diagnostics to other open files after a
/// dependency edit. Salsa-blocking — call from a `spawn_blocking` if invoked
/// off the originating file's debounce path.
fn compute_open_file_diagnostics(
    docs: &DocumentStore,
    open_files: &OpenFiles,
    uri: &Url,
    diag_cfg: &DiagnosticsConfig,
) -> Vec<Diagnostic> {
    let mut out = open_files.parse_diagnostics(uri).unwrap_or_default();
    let source = open_files.text(uri).unwrap_or_default();
    if let Some(d) = open_files.get_doc(docs, uri) {
        out.extend(duplicate_declaration_diagnostics(&source, &d, diag_cfg));
    }
    if let Some(issues) = docs.get_semantic_issues_salsa(uri) {
        out.extend(crate::semantic_diagnostics::issues_to_diagnostics(
            &issues, uri, diag_cfg,
        ));
    }
    out
}

pub struct Backend {
    client: Client,
    docs: Arc<DocumentStore>,
    /// Open-file state: text, version token, parse diagnostics.
    /// Files that are only background-indexed (never opened in the editor)
    /// do not appear here; they live only in `DocumentStore`'s salsa layer.
    open_files: OpenFiles,
    root_paths: Arc<RwLock<Vec<PathBuf>>>,
    psr4: Arc<RwLock<Psr4Map>>,
    meta: Arc<RwLock<PhpStormMeta>>,
    config: Arc<RwLock<LspConfig>>,
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
            root_paths: Arc::new(RwLock::new(Vec::new())),
            psr4,
            meta: Arc::new(RwLock::new(PhpStormMeta::default())),
            config: Arc::new(RwLock::new(LspConfig::default())),
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
    fn index_from_doc_if_not_open(&self, uri: Url, doc: &ParsedDoc, diags: Vec<Diagnostic>) {
        if !self.open_files.contains(&uri) {
            self.docs.index_from_doc(uri, doc, diags);
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

    /// Current finalized codebase — stubs + all known files, memoized by salsa.
    /// Cheap Arc clone on the happy path; on edits the query re-runs under the
    /// DocumentStore host lock. Hold the returned Arc for the duration of a
    /// request to get a consistent snapshot.
    fn codebase(&self) -> Arc<mir_codebase::Codebase> {
        self.docs.get_codebase_salsa()
    }

    /// Look up the import map for a file from the persistent codebase.
    fn file_imports(&self, uri: &Url) -> std::collections::HashMap<String, String> {
        self.codebase()
            .file_imports
            .get(uri.as_str())
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    /// Resolve the PHP version to use. See `autoload::resolve_php_version_from_roots`
    /// for the full priority order.
    fn resolve_php_version(&self, explicit: Option<&str>) -> (String, &'static str) {
        let roots = self.root_paths.read().unwrap().clone();
        crate::autoload::resolve_php_version_from_roots(&roots, explicit)
    }
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
            *self.root_paths.write().unwrap() = roots;
        }

        // Pre-load PSR-4 map synchronously during initialize so it is available
        // before the first didOpen arrives. The initialized handler reloads it too
        // (after workspace folders may be updated), but doing it here eliminates
        // the race where didOpen runs before the initialized handler finishes its
        // register_capability round-trip.
        {
            let roots = self.root_paths.read().unwrap().clone();
            if !roots.is_empty() {
                let mut merged = Psr4Map::empty();
                for root in &roots {
                    merged.extend(Psr4Map::load(root));
                }
                *self.psr4.write().unwrap() = merged;
            }
        }

        // Parse initializationOptions merged with .php-lsp.json (editor wins per-key).
        {
            let opts = params.initialization_options.as_ref();
            let roots = self.root_paths.read().unwrap().clone();

            // Load .php-lsp.json from the workspace root (first root wins).
            let file_cfg = crate::autoload::load_project_config_json(&roots);

            // Warn if the file exists but is not valid JSON (Null sentinel).
            if matches!(file_cfg, Some(serde_json::Value::Null)) {
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        "php-lsp: .php-lsp.json contains invalid JSON — ignoring",
                    )
                    .await;
            }

            // Warn if .php-lsp.json contains an unrecognised phpVersion.
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

            // Warn if the client supplied an unrecognised phpVersion.
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
            // our supported range (e.g. a legacy project with ">=5.6" in composer.json).
            // TODO: instead of storing and using the unsupported version, consider clamping
            // it to the nearest supported version so analysis stays meaningful.
            if source != "set by editor" && !crate::autoload::is_valid_php_version(&ver) {
                self.client
                    .show_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: detected PHP {ver} is outside the supported range ({}); \
                             analysis may be inaccurate",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }
            cfg.php_version = Some(ver.clone());
            if let Ok(pv) = ver.parse::<mir_analyzer::PhpVersion>() {
                self.docs.set_php_version(pv);
            }
            *self.config.write().unwrap() = cfg;
        }

        let feat = self.config.read().unwrap().features.clone();
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
            // Watch for configuration changes so we can pull the latest settings.
            Registration {
                id: "php-lsp-config-change".to_string(),
                method: "workspace/didChangeConfiguration".to_string(),
                register_options: Some(serde_json::json!({"section": "php-lsp"})),
            },
        ];
        self.client.register_capability(registrations).await.ok();

        // Load PSR-4 autoload map and kick off background workspace scan.
        // Extract roots first so RwLockReadGuard is dropped before any .await.
        let roots = self.root_paths.read().unwrap().clone();
        if !roots.is_empty() {
            // Build PSR-4 map from all roots (entries from all roots are merged).
            {
                let mut merged = Psr4Map::empty();
                for root in &roots {
                    merged.extend(Psr4Map::load(root));
                }
                *self.psr4.write().unwrap() = merged;
            }
            // Load PHPStorm metadata from the first root, if present.
            *self.meta.write().unwrap() = PhpStormMeta::load(&roots[0]);

            // Create a client-side progress indicator for the workspace scan.
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
            let (exclude_paths, max_indexed_files) = {
                let cfg = self.config.read().unwrap();
                (cfg.exclude_paths.clone(), cfg.max_indexed_files)
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
                    warm_docs.warm_reference_index();
                });
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
            let roots = self.root_paths.read().unwrap().clone();

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
            // TODO: instead of storing and using the unsupported version, consider clamping
            // it to the nearest supported version so analysis stays meaningful.
            if source != "set by editor" && !crate::autoload::is_valid_php_version(&ver) {
                self.client
                    .show_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: detected PHP {ver} is outside the supported range ({}); \
                             analysis may be inaccurate",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }
            cfg.php_version = Some(ver.clone());
            if let Ok(pv) = ver.parse::<mir_analyzer::PhpVersion>() {
                self.docs.set_php_version(pv);
            }
            *self.config.write().unwrap() = cfg;
            send_refresh_requests(&self.client).await;
        }
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        // Remove folders from our tracked roots.
        {
            let mut roots = self.root_paths.write().unwrap();
            for removed in &params.event.removed {
                if let Ok(path) = removed.uri.to_file_path() {
                    roots.retain(|r| r != &path);
                }
            }
        }

        // Add new folders and kick off background scans for each.
        let (exclude_paths, max_indexed_files) = {
            let cfg = self.config.read().unwrap();
            (cfg.exclude_paths.clone(), cfg.max_indexed_files)
        };
        for added in &params.event.added {
            if let Ok(path) = added.uri.to_file_path() {
                let is_new = {
                    let mut roots = self.root_paths.write().unwrap();
                    if !roots.contains(&path) {
                        roots.push(path.clone());
                        true
                    } else {
                        false
                    }
                };
                if is_new {
                    let docs = Arc::clone(&self.docs);
                    let open_files = self.open_files.clone();
                    let ex = exclude_paths.clone();
                    let path_clone = path.clone();
                    let client = self.client.clone();
                    tokio::spawn(async move {
                        let cache = crate::cache::WorkspaceCache::new(&path_clone);
                        scan_workspace(path_clone, docs, open_files, cache, &ex, max_indexed_files)
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

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;

        // Store text immediately so other features work while parsing.
        // This also mirrors the new text into salsa, so the codebase query
        // sees it when semantic_diagnostics runs below.
        self.set_open_text(uri.clone(), text.clone());

        let docs_for_spawn = Arc::clone(&self.docs);
        let diag_cfg = self.config.read().unwrap().diagnostics.clone();

        // Phase I: parse + semantic analysis both run on the blocking pool.
        // The semantic pass is memoized by salsa, but the *first* call per
        // file walks `StatementsAnalyzer` over the AST (hundreds of ms on
        // cold files) — we must not block the async executor on it.
        let uri_sem = uri.clone();
        let (parse_diags, sem_issues) = tokio::task::spawn_blocking(move || {
            let (_doc, parse_diags) = parse_document(&text);
            let sem_issues = docs_for_spawn.get_semantic_issues_salsa(&uri_sem);
            (parse_diags, sem_issues)
        })
        .await
        .unwrap_or_else(|_| (vec![], None));

        self.set_parse_diagnostics(&uri, parse_diags.clone());
        let stored_source = self.get_open_text(&uri).unwrap_or_default();
        let doc2 = self.get_doc(&uri);
        let mut all_diags = parse_diags;
        if let Some(ref d) = doc2 {
            all_diags.extend(duplicate_declaration_diagnostics(
                &stored_source,
                d,
                &diag_cfg,
            ));
        }
        if let Some(issues) = sem_issues {
            all_diags.extend(crate::semantic_diagnostics::issues_to_diagnostics(
                &issues, &uri, &diag_cfg,
            ));
        }
        // Publish for the opened file FIRST — see did_change for why ordering matters.
        self.client
            .publish_diagnostics(uri.clone(), all_diags, None)
            .await;

        // Cross-file republish: opening a file that defines new symbols can
        // clear `UndefinedClass`/`UndefinedFunction` errors in already-open
        // dependents. Symmetric to the loop in did_change.
        let docs_dep = Arc::clone(&self.docs);
        let open_files_dep = self.open_files.clone();
        let diag_cfg_dep = diag_cfg.clone();
        let opened_uri = uri.clone();
        let dependents = tokio::task::spawn_blocking(move || {
            let mut out: Vec<(Url, Vec<Diagnostic>)> = Vec::new();
            for other in open_files_dep.urls() {
                if other == opened_uri {
                    continue;
                }
                let diags = compute_open_file_diagnostics(
                    &docs_dep,
                    &open_files_dep,
                    &other,
                    &diag_cfg_dep,
                );
                out.push((other, diags));
            }
            out
        })
        .await
        .unwrap_or_default();
        for (dep_uri, dep_diags) in dependents {
            self.client
                .publish_diagnostics(dep_uri, dep_diags, None)
                .await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
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
        let diag_cfg = self.config.read().unwrap().diagnostics.clone();
        tokio::spawn(async move {
            // 100 ms debounce: if another edit arrives before we parse,
            // the version gate in Backend below will discard this result.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let (_doc, diagnostics) = tokio::task::spawn_blocking(move || parse_document(&text))
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
                let extra = tokio::task::spawn_blocking(move || {
                    let Some(d) = open_files_sem.get_doc(&docs_sem, &uri_sem) else {
                        return Vec::<Diagnostic>::new();
                    };
                    let source = open_files_sem.text(&uri_sem).unwrap_or_default();
                    let mut out = Vec::new();
                    if let Some(issues) = docs_sem.get_semantic_issues_salsa(&uri_sem) {
                        out.extend(crate::semantic_diagnostics::issues_to_diagnostics(
                            &issues,
                            &uri_sem,
                            &diag_cfg_sem,
                        ));
                    }
                    out.extend(duplicate_declaration_diagnostics(
                        &source,
                        &d,
                        &diag_cfg_sem,
                    ));
                    out
                })
                .await
                .unwrap_or_default();

                let mut all_diags = diagnostics;
                all_diags.extend(extra);
                // Publish for the changed file FIRST. Test harnesses (and
                // some clients) consume publishDiagnostics for unrelated
                // URIs while waiting for one specific URI; reversing this
                // order would silently swallow the changed file's publish.
                client
                    .publish_diagnostics(uri.clone(), all_diags, None)
                    .await;

                // Cross-file republish: a dependency change may invalidate
                // diagnostics in other open files. We re-query each open
                // file's diagnostics (salsa-cached for unaffected files,
                // recomputed for affected ones) and publish the result.
                //
                // Race window: if `other` is being edited concurrently, its
                // own debounced did_change will still fire a republish, so
                // any briefly-stale publish here self-corrects within ~100ms.
                let docs_dep = Arc::clone(&docs);
                let open_files_dep = open_files.clone();
                let diag_cfg_dep = diag_cfg.clone();
                let changed_uri = uri.clone();
                let dependents = tokio::task::spawn_blocking(move || {
                    let mut out: Vec<(Url, Vec<Diagnostic>)> = Vec::new();
                    for other in open_files_dep.urls() {
                        if other == changed_uri {
                            continue;
                        }
                        let diags = compute_open_file_diagnostics(
                            &docs_dep,
                            &open_files_dep,
                            &other,
                            &diag_cfg_dep,
                        );
                        out.push((other, diags));
                    }
                    out
                })
                .await
                .unwrap_or_default();
                for (dep_uri, dep_diags) in dependents {
                    client.publish_diagnostics(dep_uri, dep_diags, None).await;
                }
            }
        });
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
        let diag_cfg = self.config.read().unwrap().diagnostics.clone();
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
                        let (doc, diags) = parse_document(&text);
                        self.index_from_doc_if_not_open(change.uri.clone(), &doc, diags);
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

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        // B4c: first production caller migrated to salsa-backed read.
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(Some(CompletionResponse::Array(vec![]))),
        };
        let other_with_returns = self.docs.other_docs_with_returns(uri, &self.open_urls());
        let other_docs: Vec<Arc<ParsedDoc>> = other_with_returns
            .iter()
            .map(|(_, d, _)| d.clone())
            .collect();
        let other_returns: Vec<Arc<crate::ast::MethodReturnsMap>> = other_with_returns
            .iter()
            .map(|(_, _, r)| r.clone())
            .collect();
        let doc_returns = self.docs.get_method_returns_salsa(uri);
        let trigger = params
            .context
            .as_ref()
            .and_then(|c| c.trigger_character.as_deref());
        let meta_guard = self.meta.read().unwrap();
        let meta_opt = if meta_guard.is_empty() {
            None
        } else {
            Some(&*meta_guard)
        };
        let imports = self.file_imports(uri);
        let ctx = CompletionCtx {
            source: Some(&source),
            position: Some(position),
            meta: meta_opt,
            doc_uri: Some(uri),
            file_imports: Some(&imports),
            doc_returns: doc_returns.as_deref(),
            other_returns: Some(&other_returns),
        };
        Ok(Some(CompletionResponse::Array(filtered_completions_at(
            &doc,
            &other_docs,
            trigger,
            &ctx,
        ))))
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
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        // Search current file's ParsedDoc first (fast), then fall back to index search.
        let empty_other_docs: Vec<(Url, Arc<ParsedDoc>)> = vec![];
        if let Some(loc) = goto_definition(uri, &source, &doc, &empty_other_docs, position) {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }
        // Receiver-aware method dispatch: `$var->method()` must jump to the
        // method defined in `$var`'s class hierarchy, not the first `method`
        // found in any indexed file (which would return a wrong class).
        if let Some(line_text) = source.lines().nth(position.line as usize)
            && let Some(word) = crate::util::word_at(&source, position)
            && let Some(receiver) = crate::hover::extract_receiver_var_before_cursor(
                line_text,
                position.character as usize,
            )
        {
            let class_name = if receiver == "$this" {
                crate::type_map::enclosing_class_at(&source, &doc, position)
            } else {
                let doc_returns = self
                    .docs
                    .get_method_returns_salsa(uri)
                    .unwrap_or_else(|| std::sync::Arc::new(Default::default()));
                let tm = crate::type_map::TypeMap::from_docs_at_position(
                    &doc,
                    &doc_returns,
                    std::iter::empty(),
                    None,
                    position,
                );
                tm.get(&receiver).map(|s| s.to_string())
            };
            if let Some(cls) = class_name {
                let first_cls = cls.split('|').next().unwrap_or(&cls).to_owned();
                let all_indexes = self.docs.all_indexes();
                if let Some(loc) = find_method_in_class_hierarchy(&first_cls, &word, &all_indexes) {
                    return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                }
            }
        }

        // Cross-file: use FileIndex (no disk I/O for background files).
        let other_indexes = self.docs.other_indexes(uri);
        if let Some(word) = crate::util::word_at(&source, position)
            && let Some(loc) = find_in_indexes(&word, &other_indexes)
        {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }

        // PSR-4 fallback: only useful for fully-qualified names (contain `\`)
        if let Some(word) = word_at(&source, position)
            && word.contains('\\')
            && let Some(loc) = self.psr4_goto(&word).await
        {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }

        Ok(None)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let word = match word_at(&source, position) {
            Some(w) => w,
            None => return Ok(None),
        };
        // Special case: cursor on a class's `__construct` method declaration.
        // The constructor's call sites are `new OwningClass(...)`, not
        // `->__construct()`, so name-only matching would return every class's
        // constructor declaration (what issue reports describe as "references
        // to __construct shows every class"). Redirect to Class-kind refs on
        // the owning class and tack on the ctor's own decl span.
        if word == "__construct"
            && let Some(doc) = self.get_doc(uri)
            && let Some(class_name) =
                class_name_at_construct_decl(doc.source(), &doc.program().stmts, position)
        {
            let all_docs = self.docs.all_docs_for_scan();
            let include_declaration = params.context.include_declaration;
            // `class_name` is the FQN when the constructor is inside a namespace
            // (e.g. `"Shop\\Order"`). The AST walker must search for the *short*
            // name (`"Order"`) since that's what appears in source at call sites,
            // while the FQN is used only to scope the search and prevent collisions
            // between two classes with the same short name in different namespaces.
            let short_name = class_name
                .rsplit('\\')
                .next()
                .unwrap_or(class_name.as_str())
                .to_owned();
            let class_fqn = if class_name.contains('\\') {
                Some(class_name.as_str())
            } else {
                None
            };
            // Use `new_refs_in_stmts` directly — bypasses the codebase/salsa
            // index whose `ClassReference` key is too broad (covers type hints,
            // `instanceof`, `extends`, `implements` in addition to `new` calls).
            let mut locations = find_constructor_references(&short_name, &all_docs, class_fqn);
            if include_declaration {
                // The cursor is already on the `__construct` name (verified by
                // `class_name_at_construct_decl`), so use the cursor position directly as
                // the span rather than re-searching via str_offset (which finds the first
                // occurrence in the file and would point at the wrong constructor in files
                // with more than one class).
                let end = Position {
                    line: position.line,
                    character: position.character + "__construct".len() as u32,
                };
                locations.push(Location {
                    uri: uri.clone(),
                    range: Range {
                        start: position,
                        end,
                    },
                });
            }
            return Ok(if locations.is_empty() {
                None
            } else {
                Some(locations)
            });
        }

        let doc_opt = self.get_doc(uri);
        // Check for promoted constructor property params before the character-based
        // heuristic: `$name` in `public function __construct(public string $name)`
        // should find `->name` property accesses, not `$name` variable occurrences.
        let (word, kind) = if let Some(doc) = &doc_opt
            && let Some(prop_name) =
                promoted_property_at_cursor(doc.source(), &doc.program().stmts, position)
        {
            (prop_name, Some(SymbolKind::Property))
        } else if let Some(doc) = &doc_opt {
            let stmts = &doc.program().stmts;
            if cursor_is_on_method_decl(doc.source(), stmts, position) {
                (word, Some(SymbolKind::Method))
            } else if let Some(prop_name) =
                cursor_is_on_property_decl(doc.source(), stmts, position)
            {
                (prop_name, Some(SymbolKind::Property))
            } else {
                let k = symbol_kind_at(&source, position, &word);
                (word, k)
            }
        } else {
            let k = symbol_kind_at(&source, position, &word);
            (word, k)
        };
        let all_docs = self.docs.all_docs_for_scan();
        let include_declaration = params.context.include_declaration;

        // Resolve the FQN at the cursor so `find_references_codebase_with_target`
        // can match by exact FQN instead of short name. This fixes the
        // cross-namespace overmatch for Function/Class and the unrelated-class
        // overmatch for Method (via the owning FQCN).
        let target_fqn: Option<String> = doc_opt.as_ref().and_then(|doc| {
            let imports = self.file_imports(uri);
            match kind {
                Some(SymbolKind::Function) | Some(SymbolKind::Class) => {
                    let resolved = crate::moniker::resolve_fqn(doc, &word, &imports);
                    if resolved.contains('\\') {
                        Some(resolved)
                    } else {
                        None
                    }
                }
                Some(SymbolKind::Method) => {
                    // Owning FQCN: the class/interface/trait/enum that contains the cursor.
                    let short_owner =
                        crate::type_map::enclosing_class_at(doc.source(), doc, position)?;
                    // `resolve_fqn` walks the doc and applies namespace prefix if any.
                    Some(crate::moniker::resolve_fqn(doc, &short_owner, &imports))
                }
                _ => None,
            }
        });

        // Fast path: look up references via the salsa `symbol_refs` query.
        // First call per key runs `file_refs` across the workspace; subsequent
        // calls hit salsa's memo. Falls back to the full AST scan for Method /
        // None kinds, and whenever the symbol is not found in the codebase.
        let locations = {
            let cb = self.codebase();
            let docs = Arc::clone(&self.docs);
            let lookup = move |key: &str| docs.get_symbol_refs_salsa(key);
            find_references_codebase_with_target(
                &word,
                &all_docs,
                include_declaration,
                kind,
                target_fqn.as_deref(),
                &cb,
                &lookup,
            )
            .unwrap_or_else(|| match target_fqn.as_deref() {
                Some(t) => {
                    find_references_with_target(&word, &all_docs, include_declaration, kind, t)
                }
                None => find_references(&word, &all_docs, include_declaration, kind),
            })
        };

        Ok(if locations.is_empty() {
            None
        } else {
            Some(locations)
        })
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
        let word = match word_at(&source, position) {
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
            Ok(Some(rename(&word, &params.new_name, &all_docs)))
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
        Ok(signature_help(&source, &doc, position))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let doc_returns = self
            .docs
            .get_method_returns_salsa(uri)
            .unwrap_or_else(|| std::sync::Arc::new(Default::default()));
        let other_docs = self.docs.other_docs_with_returns(uri, &self.open_urls());
        let result = hover_info(&source, &doc, &doc_returns, position, &other_docs);
        if result.is_some() {
            return Ok(result);
        }
        // Fallback: look up the word in the workspace index so class names in
        // extends clauses and parameter types resolve even when their defining
        // file is never opened.  Also try the alias-resolved name so that
        // `use Foo as Bar` works even when Foo is only in the index.
        let all_indexes = self.docs.all_indexes();
        if let Some(word) = crate::util::word_at(&source, position) {
            // Try the literal word first.
            if let Some(h) = class_hover_from_index(&word, &all_indexes) {
                return Ok(Some(h));
            }
            // Try alias resolution.
            if let Some(resolved) = crate::hover::resolve_use_alias(&doc.program().stmts, &word)
                && let Some(h) = class_hover_from_index(&resolved, &all_indexes)
            {
                return Ok(Some(h));
            }
        }
        Ok(None)
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
        let doc_returns = self.docs.get_method_returns_salsa(uri);
        let wi = self.docs.get_workspace_index_salsa();
        Ok(Some(inlay_hints(
            doc.source(),
            &doc,
            doc_returns.as_deref(),
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
        Ok(if results.is_empty() {
            None
        } else {
            Some(results)
        })
    }

    async fn symbol_resolve(&self, params: WorkspaceSymbol) -> Result<WorkspaceSymbol> {
        // For resolve, we need the full range from the ParsedDoc of open files.
        let docs = self.docs.docs_for(&self.open_urls());
        Ok(resolve_workspace_symbol(params, &docs))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
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
        let ranges = selection_ranges(doc.source(), &doc, &params.positions);
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
        let word = match word_at(&source, position) {
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
        // Reuse document_highlights: every occurrence of the symbol is a linked range.
        let highlights = document_highlights(&source, &doc, position);
        if highlights.is_empty() {
            return Ok(None);
        }
        let ranges: Vec<Range> = highlights.into_iter().map(|h| h.range).collect();
        Ok(Some(LinkedEditingRanges {
            ranges,
            // PHP identifiers: letters, digits, underscore; variables also allow leading $
            word_pattern: Some(r"[$a-zA-Z_\x80-\xff][a-zA-Z0-9_\x80-\xff]*".to_string()),
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
        let word = crate::util::word_at(&source, position).unwrap_or_default();
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
        let doc_returns = self.docs.get_method_returns_salsa(uri);
        // First pass: open-file ParsedDocs give accurate character positions.
        let open_docs = self.docs.docs_for(&self.open_urls());
        if let Some(loc) =
            goto_type_definition(&source, &doc, doc_returns.as_deref(), &open_docs, position)
        {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }
        // Second pass: background files via FileIndex (line-only positions).
        let all_indexes = self.docs.all_indexes();
        Ok(goto_type_definition_from_index(
            &source,
            &doc,
            doc_returns.as_deref(),
            &all_indexes,
            position,
        )
        .map(GotoDefinitionResponse::Scalar))
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

                let root = self.root_paths.read().unwrap().first().cloned();
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
        let psr4 = self.psr4.read().unwrap();
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
        let psr4 = self.psr4.read().unwrap();
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
        let psr4 = self.psr4.read().unwrap();
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
                return Ok(DocumentDiagnosticReportResult::Report(
                    DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                        related_documents: None,
                        full_document_diagnostic_report: FullDocumentDiagnosticReport {
                            result_id: None,
                            items: parse_diags,
                        },
                    }),
                ));
            }
        };
        let (diag_cfg, php_version) = {
            let cfg = self.config.read().unwrap();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };
        let _ = php_version.as_deref();
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
        .unwrap_or_default();
        let mut items = parse_diags;
        items.extend(sem_diags);
        items.extend(duplicate_declaration_diagnostics(&source, &doc, &diag_cfg));

        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items,
                },
            }),
        ))
    }

    async fn workspace_diagnostic(
        &self,
        _params: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReportResult> {
        let all_parse_diags = self.all_open_files_with_diagnostics();
        let (diag_cfg, php_version) = {
            let cfg = self.config.read().unwrap();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };

        // Phase I: each file's semantic issues flow through the salsa
        // `semantic_issues` query. The memo is shared with `did_open` /
        // `did_change` / `document_diagnostic` / `code_action`, so repeated
        // workspace-diagnostic pulls reuse prior analysis. The first pull on
        // a cold workspace still walks every file's `StatementsAnalyzer` —
        // run the whole sweep on the blocking pool so the async runtime
        // stays responsive.
        let _ = php_version.as_deref();
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
                    let mut all_diags = parse_diags;
                    all_diags.extend(sem_diags);
                    all_diags.extend(duplicate_declaration_diagnostics(
                        &source,
                        &doc,
                        &diag_cfg_sweep,
                    ));

                    Some(WorkspaceDocumentDiagnosticReport::Full(
                        WorkspaceFullDocumentDiagnosticReport {
                            uri,
                            version,
                            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                                result_id: None,
                                items: all_diags,
                            },
                        },
                    ))
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();

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
        let diag_cfg = self.config.read().unwrap().diagnostics.clone();
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
fn position_to_offset(source: &str, position: Position) -> Option<u32> {
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
    let Some(cursor) = position_to_offset(source, position) else {
        return false;
    };

    fn check(source: &str, stmts: &[Stmt<'_, '_>], cursor: u32) -> bool {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Class(c) => {
                    for member in c.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let start = str_offset(source, m.name);
                            let end = start + m.name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Interface(i) => {
                    for member in i.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let start = str_offset(source, m.name);
                            let end = start + m.name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Trait(t) => {
                    for member in t.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let start = str_offset(source, m.name);
                            let end = start + m.name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Enum(e) => {
                    for member in e.members.iter() {
                        if let EnumMemberKind::Method(m) = &member.kind {
                            let start = str_offset(source, m.name);
                            let end = start + m.name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Namespace(ns) => {
                    if let NamespaceBody::Braced(inner) = &ns.body
                        && check(source, inner, cursor)
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
    let cursor = position_to_offset(source, position)?;

    fn check(source: &str, stmts: &[Stmt<'_, '_>], cursor: u32) -> Option<String> {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Class(c) => {
                    for member in c.members.iter() {
                        if let ClassMemberKind::Property(p) = &member.kind {
                            let start = str_offset(source, p.name);
                            let end = start + p.name.len() as u32;
                            if cursor >= start && cursor < end {
                                return Some(p.name.to_owned());
                            }
                        }
                    }
                }
                StmtKind::Trait(t) => {
                    for member in t.members.iter() {
                        if let ClassMemberKind::Property(p) = &member.kind {
                            let start = str_offset(source, p.name);
                            let end = start + p.name.len() as u32;
                            if cursor >= start && cursor < end {
                                return Some(p.name.to_owned());
                            }
                        }
                    }
                }
                StmtKind::Namespace(ns) => {
                    if let NamespaceBody::Braced(inner) = &ns.body
                        && let Some(name) = check(source, inner, cursor)
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
    let cursor = position_to_offset(source, position)?;

    fn check(source: &str, stmts: &[Stmt<'_, '_>], cursor: u32, ns_prefix: &str) -> Option<String> {
        let mut current_ns = ns_prefix.to_owned();
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Class(c) => {
                    for member in c.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == "__construct"
                        {
                            let start = str_offset(source, m.name);
                            let end = start + m.name.len() as u32;
                            if cursor >= start && cursor < end {
                                let short = c.name?;
                                return Some(if current_ns.is_empty() {
                                    short.to_owned()
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
                            if let Some(name) = check(source, inner, cursor, &ns_name) {
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
    let cursor = position_to_offset(source, position)?;

    fn check(source: &str, stmts: &[Stmt<'_, '_>], cursor: u32) -> Option<String> {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Class(c) => {
                    for member in c.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == "__construct"
                        {
                            for param in m.params.iter() {
                                if param.visibility.is_none() {
                                    continue;
                                }
                                let name_start = str_offset(source, param.name);
                                let name_end = name_start + param.name.len() as u32;
                                if cursor >= name_start && cursor < name_end {
                                    return Some(param.name.trim_start_matches('$').to_owned());
                                }
                            }
                        }
                    }
                }
                StmtKind::Namespace(ns) => {
                    if let NamespaceBody::Braced(inner) = &ns.body
                        && let Some(name) = check(source, inner, cursor)
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
        let path = {
            let psr4 = self.psr4.read().unwrap();
            psr4.resolve(fqn)?
        };

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

/// Ask all connected clients to re-request semantic tokens, code lenses, inlay hints,
/// and diagnostics. Called after bulk index operations so that previously-opened editors
/// immediately pick up the newly indexed symbol information.
async fn send_refresh_requests(client: &Client) {
    client.send_request::<SemanticTokensRefresh>(()).await.ok();
    client.send_request::<CodeLensRefresh>(()).await.ok();
    client
        .send_request::<InlayHintRefreshRequest>(())
        .await
        .ok();
    client
        .send_request::<WorkspaceDiagnosticRefresh>(())
        .await
        .ok();
    client
        .send_request::<InlineValueRefreshRequest>(())
        .await
        .ok();
}

/// Maximum number of PHP files indexed during a workspace scan.
/// Prevents excessive memory use on projects with very large vendor trees.
const MAX_INDEXED_FILES: usize = 50_000;

/// Recursively scan `root` for `*.php` files and add them to the document store.
/// Skips hidden directories (names starting with `.`) and any path whose string
/// representation contains a segment matching one of the `exclude_paths` patterns.
/// Returns the number of files indexed.
///
/// Phase 1 — directory traversal: async, serial (I/O-bound; tokio handles it well).
/// Phase 2 — file reading + parsing: concurrent, bounded by available CPU cores.
///
/// Post-salsa: we only populate the DocumentStore here. The codebase is built
/// on demand by the salsa `codebase` query the first time a feature asks for
/// it — stubs + every indexed file's StubSlice, memoized thereafter.
#[tracing::instrument(
    skip(docs, open_files, cache, exclude_paths),
    fields(root = %root.display())
)]
async fn scan_workspace(
    root: PathBuf,
    docs: Arc<DocumentStore>,
    open_files: OpenFiles,
    cache: Option<crate::cache::WorkspaceCache>,
    exclude_paths: &[String],
    max_files: usize,
) -> usize {
    // Phase 1: collect PHP file paths via async directory walk.
    let mut php_files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root];

    'walk: while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            // Normalize to forward slashes so patterns like "src/Service/*"
            // match on Windows where paths use backslashes.
            let path_str = path.to_string_lossy().replace('\\', "/");
            // Check user-configured exclude patterns (simple substring/prefix match).
            if exclude_paths.iter().any(|pat| {
                let p = pat.trim_end_matches('*').trim_end_matches('/');
                path_str.contains(p)
            }) {
                continue;
            }
            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // Skip hidden directories; vendor is indexed unless excluded above.
                if !name.starts_with('.') {
                    stack.push(path);
                }
            } else if file_type.is_file() && path.extension().is_some_and(|e| e == "php") {
                php_files.push(path);
                if php_files.len() >= max_files {
                    break 'walk;
                }
            }
        }
    }

    // Phase 2: read and parse files concurrently, bounded by available CPU cores.
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let sem = Arc::new(tokio::sync::Semaphore::new(parallelism));
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    for path in php_files {
        let permit = Arc::clone(&sem).acquire_owned().await.unwrap();
        let docs = Arc::clone(&docs);
        let open_files = open_files.clone();
        let cache = cache.clone();
        let count = Arc::clone(&count);
        set.spawn(async move {
            let _permit = permit;
            let Ok(text) = tokio::fs::read_to_string(&path).await else {
                return;
            };
            let Ok(uri) = Url::from_file_path(&path) else {
                return;
            };
            tokio::task::spawn_blocking(move || {
                // Skip files the editor has already opened — their buffer
                // is authoritative; scan must not overwrite their salsa
                // input with disk contents.
                if open_files.contains(&uri) {
                    return;
                }

                // Phase K2b read path: if the on-disk cache has a StubSlice
                // for this (uri, content) key, mirror the text and seed
                // the cached slice — `file_definitions` will return it
                // directly on the first query, skipping parse and
                // `DefinitionCollector` entirely. An edit later clears
                // the seeded slice via `mirror_text` (K2a).
                let cache_key = cache
                    .as_ref()
                    .map(|_| crate::cache::WorkspaceCache::key_for(uri.as_str(), &text));
                if let (Some(cache), Some(key)) = (cache.as_ref(), cache_key.as_ref())
                    && let Some(slice) = cache.read::<mir_codebase::storage::StubSlice>(key)
                {
                    docs.mirror_text(&uri, &text);
                    docs.seed_cached_slice(&uri, Arc::new(slice));
                    count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return;
                }

                // Cache miss: normal parse + mirror.
                let (doc, diags) = parse_document(&text);
                docs.index_from_doc(uri.clone(), &doc, diags);
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                // K2b write path: force `file_definitions` and persist
                // the fresh slice so a subsequent startup hits the cache.
                // The work is unavoidable anyway — `get_codebase_salsa`
                // would call `file_definitions` lazily on first use — so
                // materializing it here trades a small up-front cost for
                // a large warm-start win next time. Best-effort: a write
                // error is logged via `.ok()` and doesn't fail the scan.
                if let (Some(cache), Some(key)) = (cache.as_ref(), cache_key.as_ref())
                    && let Some(slice) = docs.slice_for(&uri)
                {
                    let _ = cache.write(key, &*slice);
                }
            })
            .await
            .ok();
        });
    }

    while set.join_next().await.is_some() {}

    count.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    // ── position_to_offset ───────────────────────────────────────────────────

    #[test]
    fn position_to_offset_first_line() {
        let src = "<?php\nfoo();";
        // Character 0 → byte 0.
        assert_eq!(
            position_to_offset(
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
            position_to_offset(
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
            position_to_offset(
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
    fn position_to_offset_second_line() {
        let src = "<?php\nfoo();";
        // Start of line 1 is byte 6 (after "<?php\n").
        assert_eq!(
            position_to_offset(
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
            position_to_offset(
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
    fn position_to_offset_line_boundary_returns_none() {
        // A source with exactly one line has only line 0; line 1 must return None.
        let src = "<?php";
        assert_eq!(
            position_to_offset(
                src,
                Position {
                    line: 1,
                    character: 0
                }
            ),
            None
        );
        assert_eq!(
            position_to_offset(
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
