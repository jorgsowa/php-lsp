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
            include_paths: Vec::new(),
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
            // Both excludePaths and includePaths are concatenated rather than replaced.
            if key == "excludePaths" || key == "includePaths" {
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
        if let Some(arr) = v.get("includePaths").and_then(|x| x.as_array()) {
            cfg.include_paths = arr
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
