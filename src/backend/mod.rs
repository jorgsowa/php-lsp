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
use tower_lsp::Client;
use tower_lsp::lsp_types::*;

use crate::ast::ParsedDoc;
use crate::autoload::Psr4Map;
use crate::config::LspConfig;
use crate::document_store::DocumentStore;
use crate::open_files::OpenFiles;
use crate::phpstorm_meta::PhpStormMeta;
use crate::util::fqn_short_name;

use crate::navigation::references::find_constructor_references;

use crate::analysis::diagnostics::merge_file_diagnostics;

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
                let short_owner = crate::type_map::enclosing_class_at(doc.source(), doc, position)?;
                // `resolve_fqn` walks the doc and applies the namespace prefix if any.
                Some(crate::navigation::moniker::resolve_fqn(
                    doc,
                    &short_owner,
                    &imports,
                ))
            }
            Some(SymbolKind::Constant) => {
                if constant_owner.is_some() {
                    // Class constant: the owning class short name as-is.
                    constant_owner
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
        kind: Option<crate::navigation::references::SymbolKind>,
        target_fqn: Option<&str>,
        owner_short: Option<&str>,
    ) -> Option<Vec<Location>> {
        if !matches!(
            kind,
            Some(crate::navigation::references::SymbolKind::Method)
        ) {
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
    kind: Option<crate::navigation::references::SymbolKind>,
    target_fqn: Option<&str>,
) -> Option<mir_analyzer::Name> {
    use crate::navigation::references::SymbolKind;
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

mod helpers;
mod server;
#[cfg(test)]
mod tests;
