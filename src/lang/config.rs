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
    /// Unused-symbol warnings (unused variables / parameters / methods /
    /// properties / functions). New in mir 0.22; defaults to `false` so the
    /// LSP doesn't add noisy warnings to existing workspaces without an
    /// opt-in. Toggle via `diagnostics.unusedSymbols` in initializationOptions.
    pub unused_symbols: bool,
    /// Missing type annotations on interface methods and class properties
    /// (MissingReturnType, MissingParamType, MissingPropertyType). Off by
    /// default; opt in via `diagnostics.missingTypes`.
    pub missing_types: bool,
    /// Mixed-type usage lints: passing `mixed` to a typed parameter, assigning
    /// mixed to a typed property, array/property access on mixed, etc. Off by
    /// default; opt in via `diagnostics.mixedUsage`.
    pub mixed_usage: bool,
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
            unused_symbols: false,
            missing_types: false,
            mixed_usage: false,
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

    pub(crate) fn from_value(v: &serde_json::Value) -> Self {
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
        cfg.unused_symbols = obj
            .get("unusedSymbols")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        cfg.missing_types = obj
            .get("missingTypes")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        cfg.mixed_usage = obj
            .get("mixedUsage")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
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
    pub(crate) fn from_value(v: &serde_json::Value) -> Self {
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

/// PHPStan integration. Off by default: running a project's own PHPStan
/// installation takes far longer than the built-in analyzer (seconds, not
/// milliseconds) and requires a project-specific config, so it's an explicit
/// opt-in rather than something that fires for every workspace.
#[derive(Debug, Clone)]
pub struct PhpstanConfig {
    pub enabled: bool,
    /// Executable name or path. Defaults to `phpstan` resolved via `$PATH`;
    /// set to e.g. `vendor/bin/phpstan` for a project-local install.
    pub bin_path: String,
    /// Passed as `-c <path>` when set. Otherwise PHPStan uses its own
    /// discovery (`phpstan.neon` / `phpstan.neon.dist` in the workspace root).
    pub config_path: Option<String>,
}

impl Default for PhpstanConfig {
    fn default() -> Self {
        PhpstanConfig {
            enabled: false,
            bin_path: "phpstan".to_string(),
            config_path: None,
        }
    }
}

impl PhpstanConfig {
    pub(crate) fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = PhpstanConfig::default();
        let Some(obj) = v.as_object() else { return cfg };
        cfg.enabled = obj
            .get("enabled")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        if let Some(s) = obj.get("binPath").and_then(|x| x.as_str()) {
            cfg.bin_path = s.to_string();
        }
        if let Some(s) = obj.get("configPath").and_then(|x| x.as_str()) {
            cfg.config_path = Some(s.to_string());
        }
        cfg
    }
}

/// PHPCS integration. Off by default, same rationale as [`PhpstanConfig`].
#[derive(Debug, Clone)]
pub struct PhpcsConfig {
    pub enabled: bool,
    /// Executable name or path. Defaults to `phpcs` resolved via `$PATH`;
    /// set to e.g. `vendor/bin/phpcs` for a project-local install.
    pub bin_path: String,
    /// Passed as `--standard=<value>` when set (e.g. `"PSR12"`). Otherwise
    /// PHPCS uses its own default/ruleset discovery.
    pub standard: Option<String>,
}

impl Default for PhpcsConfig {
    fn default() -> Self {
        PhpcsConfig {
            enabled: false,
            bin_path: "phpcs".to_string(),
            standard: None,
        }
    }
}

impl PhpcsConfig {
    pub(crate) fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = PhpcsConfig::default();
        let Some(obj) = v.as_object() else { return cfg };
        cfg.enabled = obj
            .get("enabled")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        if let Some(s) = obj.get("binPath").and_then(|x| x.as_str()) {
            cfg.bin_path = s.to_string();
        }
        if let Some(s) = obj.get("standard").and_then(|x| x.as_str()) {
            cfg.standard = Some(s.to_string());
        }
        cfg
    }
}

/// External static-analysis tools run as child processes and merged into
/// published diagnostics, in addition to (not instead of) the built-in
/// analyzer. See [`PhpstanConfig`] / [`PhpcsConfig`] for why they default off.
#[derive(Debug, Clone, Default)]
pub struct ExternalToolsConfig {
    pub phpstan: PhpstanConfig,
    pub phpcs: PhpcsConfig,
}

impl ExternalToolsConfig {
    pub(crate) fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = ExternalToolsConfig::default();
        let Some(obj) = v.as_object() else { return cfg };
        if let Some(v) = obj.get("phpstan") {
            cfg.phpstan = PhpstanConfig::from_value(v);
        }
        if let Some(v) = obj.get("phpcs") {
            cfg.phpcs = PhpcsConfig::from_value(v);
        }
        cfg
    }
}

/// Maximum number of PHP files indexed during a workspace scan.
/// Prevents excessive memory use on projects with very large vendor trees.
pub const MAX_INDEXED_FILES: usize = 50_000;

/// Configuration received from the client via `initializationOptions`.
#[derive(Debug, Clone)]
pub struct LspConfig {
    /// PHP version string, e.g. `"8.1"`.  Set explicitly via `initializationOptions`
    /// or auto-detected from `composer.json` / the `php` binary at startup.
    pub php_version: Option<String>,
    /// Glob patterns for paths to exclude from workspace indexing.
    pub exclude_paths: Vec<String>,
    /// Glob patterns for paths that must be indexed even if they match an
    /// `excludePaths` entry.  Patterns are matched against path components
    /// (same semantics as `excludePaths`).  Example: `["vendor/yiisoft"]`.
    pub include_paths: Vec<String>,
    /// Directories of user-supplied PHP stub files to load in addition to the
    /// bundled built-ins. Each entry is resolved relative to the workspace
    /// root if not already absolute. Every `.php` file found (recursively) is
    /// registered as a read-only, highest-precedence symbol source — useful
    /// for extensions or frameworks the bundled stubs don't cover.
    pub stub_dirs: Vec<String>,
    /// Per-category diagnostic toggles.
    pub diagnostics: DiagnosticsConfig,
    /// Per-feature capability toggles.
    pub features: FeaturesConfig,
    /// Hard cap on the number of PHP files indexed during a workspace scan.
    /// Defaults to [`MAX_INDEXED_FILES`]. Set lower via `initializationOptions`
    /// to reduce memory on projects with very large vendor trees.
    pub max_indexed_files: usize,
    /// Whether to eagerly index `vendor/` during the workspace scan.
    ///
    /// Default `true`: `vendor/` is walked and declaration-extracted
    /// (name/namespace/signature, no type inference) alongside the rest of
    /// the workspace, so bare-name completion, workspace symbols, and
    /// find-implementations/type-hierarchy see vendor classes without extra
    /// configuration (issues #240, #246). This scan is I/O-bound, not
    /// CPU-bound, and cheap relative to the fear it usually gets: on real
    /// fixtures the walk+read+parse+extract for ~5k files runs in ~200ms, and
    /// unchanged files replay from the on-disk content-hash cache
    /// (`index/cache.rs`) on subsequent starts. What eager vendor indexing
    /// does NOT do is pull vendor into the background `warm_analysis_sweep`
    /// (full type analysis for find-references/rename) — that sweep skips
    /// vendor files unconditionally (see `document_store::is_vendor_uri`)
    /// since sweeping the whole vendor tree buys nothing lasting (the
    /// analysis-result cache is capped, see `ANALYSIS_CACHE_CAP`) and would
    /// multiply background CPU cost by vendor's file count. A vendor file
    /// still gets fully analyzed on demand — when a request touches it
    /// directly, or when it's referenced by a currently-open file (which the
    /// sweep prioritizes regardless of this flag).
    ///
    /// Set `false` to skip `vendor/` entirely on scan (old default): vendor
    /// files then load only on demand via PSR-4 resolution (composer
    /// autoload + per-file parse). Useful for very large vendor trees where
    /// even the cheap declaration scan isn't worth paying for.
    pub index_vendor: bool,
    /// Emit extra diagnostic log messages on startup: cache hit/miss ratio,
    /// workspace root paths, and PSR-4 namespace count.
    /// Enable via `initializationOptions.debug = true`.
    pub debug: bool,
    /// Debounce delay in milliseconds between the last `textDocument/didChange`
    /// and the parse + analysis run. Defaults to 100 ms. Set lower for
    /// fast machines / Neovim users; set higher for slow machines or large
    /// files to reduce thrashing.
    pub debounce_ms: u64,
    /// Background-analyze the workspace after indexing (and re-warm after
    /// edits settle) so find-references / rename answer from warm memos
    /// instead of paying a cold per-file analysis at request time. Default
    /// `true`; set `warmAnalysis: false` to trade slower references for a
    /// smaller resident footprint.
    pub warm_analysis: bool,
    /// Extend the background warm-analysis sweep to `vendor/` files (see the
    /// `index_vendor` docs above for why the ambient sweep skips vendor by
    /// default). Opt-in and throttled/idle-priority once implemented — ships
    /// off by default since it's new background CPU cost with no prior
    /// production data on real vendor-tree sizes (ROADMAP 0c step 2,
    /// `~/.claude/plans/crispy-noodling-key.md`). **Not implemented yet**:
    /// setting this to `true` is currently a no-op.
    pub warm_vendor_analysis: bool,
    /// Override the on-disk cache directory. When set, used verbatim (no
    /// schema-version or workspace-hash subdirectories appended). Primarily
    /// useful in tests and CI environments with non-standard cache locations.
    /// When absent, falls back to the platform default (`$XDG_CACHE_HOME` /
    /// `$HOME/.cache` on Unix, `%LOCALAPPDATA%` on Windows).
    pub cache_path: Option<std::path::PathBuf>,
    /// How often the background loop persists staged analysis-cache postings
    /// (reference postings from an in-progress warm sweep or an on-demand
    /// query freshness pass) to disk. Bounds data loss on an unclean exit
    /// (crash, kill — anything that skips `shutdown`) to roughly one
    /// interval. Defaults to 20 s; lower mainly useful in tests.
    pub flush_interval_ms: u64,
    /// External static-analysis tool integration (PHPStan / PHPCS). Both
    /// default off; see [`ExternalToolsConfig`].
    pub external_tools: ExternalToolsConfig,
}

impl Default for LspConfig {
    fn default() -> Self {
        LspConfig {
            php_version: None,
            exclude_paths: Vec::new(),
            include_paths: Vec::new(),
            stub_dirs: Vec::new(),
            diagnostics: DiagnosticsConfig::default(),
            features: FeaturesConfig::default(),
            max_indexed_files: MAX_INDEXED_FILES,
            index_vendor: true,
            debug: false,
            debounce_ms: 100,
            warm_analysis: true,
            warm_vendor_analysis: false,
            cache_path: None,
            flush_interval_ms: 20_000,
            external_tools: ExternalToolsConfig::default(),
        }
    }
}

impl LspConfig {
    /// Merge a `.php-lsp.json` value with editor `initializationOptions` /
    /// `workspace/configuration`. Editor settings win per-key; `excludePaths`,
    /// `includePaths`, and `stubDirs` arrays are **concatenated** (file entries
    /// first, editor entries appended) rather than replaced, since exclusion
    /// patterns and stub directories are both additive.
    ///
    /// Hot-reload of `.php-lsp.json` on file change is not supported; the file
    /// is only read during `initialize` and `did_change_configuration`.
    pub(crate) fn merge_project_configs(
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
            // excludePaths/includePaths/stubDirs are concatenated rather than replaced.
            if key == "excludePaths" || key == "includePaths" || key == "stubDirs" {
                let file_arr = merged_obj
                    .get(key)
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

    pub(crate) fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = LspConfig::default();
        if let Some(ver) = v.get("phpVersion").and_then(|x| x.as_str()) {
            if crate::lang::autoload::is_valid_php_version(ver) {
                cfg.php_version = Some(ver.to_string());
            } else {
                // Invalid version: skip environment detection, use the latest stubs.
                cfg.php_version = Some(crate::lang::autoload::PHP_8_5.to_string());
            }
        }
        if let Some(arr) = v.get("excludePaths").and_then(|x| x.as_array()) {
            cfg.exclude_paths = arr
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect();
        }
        if let Some(arr) = v.get("includePaths").and_then(|x| x.as_array()) {
            cfg.include_paths = arr
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect();
        }
        if let Some(arr) = v.get("stubDirs").and_then(|x| x.as_array()) {
            cfg.stub_dirs = arr
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
        if let Some(b) = v.get("indexVendor").and_then(|x| x.as_bool()) {
            cfg.index_vendor = b;
        }
        if let Some(b) = v.get("debug").and_then(|x| x.as_bool()) {
            cfg.debug = b;
        }
        if let Some(n) = v.get("debounceMs").and_then(|x| x.as_u64()) {
            cfg.debounce_ms = n.max(1);
        }
        if let Some(b) = v.get("warmAnalysis").and_then(|x| x.as_bool()) {
            cfg.warm_analysis = b;
        }
        if let Some(b) = v.get("warmVendorAnalysis").and_then(|x| x.as_bool()) {
            cfg.warm_vendor_analysis = b;
        }
        if let Some(s) = v.get("cachePath").and_then(|x| x.as_str()) {
            cfg.cache_path = Some(std::path::PathBuf::from(s));
        }
        if let Some(n) = v
            .get("analysisCacheFlushIntervalMs")
            .and_then(|x| x.as_u64())
        {
            cfg.flush_interval_ms = n.max(1);
        }
        if let Some(v) = v.get("externalTools") {
            cfg.external_tools = ExternalToolsConfig::from_value(v);
        }
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapshot of every default value `LspConfig` (and its nested
    /// `DiagnosticsConfig`/`FeaturesConfig`) ships with. A default flip like
    /// `index_vendor: false -> true` (issue #246 fix) shows up here as a diff
    /// instead of silently changing behavior for every user who never sets
    /// the option explicitly — the snapshot forces a conscious
    /// `UPDATE_EXPECT=1` step, which is exactly the point where the author
    /// should also be checking every test/doc that assumed the old default.
    #[test]
    fn default_config_matches_expected_snapshot() {
        expect_test::expect![[r#"
            LspConfig {
                php_version: None,
                exclude_paths: [],
                include_paths: [],
                stub_dirs: [],
                diagnostics: DiagnosticsConfig {
                    enabled: true,
                    undefined_variables: true,
                    undefined_functions: true,
                    undefined_classes: true,
                    arity_errors: true,
                    type_errors: true,
                    deprecated_calls: true,
                    duplicate_declarations: true,
                    unused_symbols: false,
                    missing_types: false,
                    mixed_usage: false,
                },
                features: FeaturesConfig {
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
                },
                max_indexed_files: 50000,
                index_vendor: true,
                debug: false,
                debounce_ms: 100,
                warm_analysis: true,
                warm_vendor_analysis: false,
                cache_path: None,
                flush_interval_ms: 20000,
                external_tools: ExternalToolsConfig {
                    phpstan: PhpstanConfig {
                        enabled: false,
                        bin_path: "phpstan",
                        config_path: None,
                    },
                    phpcs: PhpcsConfig {
                        enabled: false,
                        bin_path: "phpcs",
                        standard: None,
                    },
                },
            }"#]]
        .assert_eq(&format!("{:#?}", LspConfig::default()));
    }

    /// Every key `LspConfig::from_value`/`DiagnosticsConfig::from_value`/
    /// `FeaturesConfig::from_value` actually reads — scraped from this
    /// file's own JSON-object-key lookups, so a newly wired-up option is
    /// caught automatically, no hand-maintained list to fall out of sync —
    /// must appear somewhere in `documentation/src/content/docs/configuration.md`.
    /// This is the gap that let `indexVendor` ship real, tested, and
    /// completely undocumented for months (issue #246): a user had no way
    /// to discover the one setting that fixed their exact problem.
    #[test]
    fn every_config_key_is_documented() {
        let source = include_str!("config.rs");
        let docs = include_str!("../../documentation/src/content/docs/configuration.md");

        let mut keys: Vec<&str> = Vec::new();
        for marker in ["get(\"", "flag(\""] {
            let mut rest = source;
            while let Some(idx) = rest.find(marker) {
                rest = &rest[idx + marker.len()..];
                if let Some(end) = rest.find('"') {
                    keys.push(&rest[..end]);
                }
            }
        }
        keys.sort_unstable();
        keys.dedup();

        let undocumented: Vec<&str> = keys
            .into_iter()
            .filter(|k| !docs.contains(&format!("`{k}`")))
            .collect();
        assert!(
            undocumented.is_empty(),
            "config keys read by LspConfig::from_value but missing from \
             documentation/src/content/docs/configuration.md: {undocumented:?}"
        );
    }

    #[test]
    fn stub_dirs_parses_from_initialization_options() {
        let v = serde_json::json!({"stubDirs": ["stubs", "/abs/other-stubs"]});
        let cfg = LspConfig::from_value(&v);
        assert_eq!(cfg.stub_dirs, vec!["stubs", "/abs/other-stubs"]);
    }

    #[test]
    fn stub_dirs_absent_defaults_to_empty() {
        let cfg = LspConfig::from_value(&serde_json::json!({}));
        assert!(cfg.stub_dirs.is_empty());
    }

    /// `stubDirs` is additive like `excludePaths`/`includePaths`: a project's
    /// `.php-lsp.json` and an editor's `initializationOptions` both listing
    /// directories must combine, not have one replace the other.
    #[test]
    fn stub_dirs_concatenates_file_and_editor_config() {
        let file = serde_json::json!({"stubDirs": ["project-stubs"]});
        let editor = serde_json::json!({"stubDirs": ["personal-stubs"]});
        let merged = LspConfig::merge_project_configs(Some(&file), Some(&editor));
        let cfg = LspConfig::from_value(&merged);
        assert_eq!(cfg.stub_dirs, vec!["project-stubs", "personal-stubs"]);
    }
}
