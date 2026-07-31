use super::helpers::*;
use super::*;
use crate::document::ast::ParsedDoc;
use crate::editing::use_import::{build_use_import_edit, find_use_insert_line};
use crate::lang::config::{DiagnosticsConfig, FeaturesConfig, MAX_INDEXED_FILES};
use tower_lsp_server::ls_types::{Position, Range, Uri};

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
        LspConfig::from_value(&serde_json::json!({"phpVersion": crate::lang::autoload::PHP_8_2}));
    assert_eq!(
        cfg.php_version.as_deref(),
        Some(crate::lang::autoload::PHP_8_2)
    );
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

#[test]
fn lsp_config_default_debounce_ms() {
    let cfg = LspConfig::default();
    assert_eq!(cfg.debounce_ms, 100);
}

#[test]
fn lsp_config_parses_debounce_ms() {
    let cfg = LspConfig::from_value(&serde_json::json!({"debounceMs": 150}));
    assert_eq!(cfg.debounce_ms, 150);
}

#[test]
fn lsp_config_debounce_ms_minimum_one() {
    let cfg = LspConfig::from_value(&serde_json::json!({"debounceMs": 0}));
    assert_eq!(cfg.debounce_ms, 1);
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

// php_file_op tests
#[test]
fn php_file_op_matches_php_files() {
    let op = php_file_op();
    assert_eq!(op.filters.len(), 1);
    let filter = &op.filters[0];
    assert_eq!(filter.scheme.as_deref(), Some("file"));
    assert_eq!(filter.pattern.glob, "**/*.php");
}

// Uri regression tests (url::Url -> ls_types::Uri migration; the new Uri
// wraps fluent_uri, strict RFC 3986, instead of WHATWG-style url::Url)
#[test]
fn uri_from_file_path_wire_string_roundtrips_for_space_and_unicode_paths() {
    // A Uri's own wire-format string (what the server actually sends the
    // client, e.g. in a `definition` response) must parse back to an equal,
    // equally-hashable Uri — this is exactly the round trip a real editor
    // performs, and DocumentStore's Uri-keyed maps must recognize it as the
    // same file. Note: this parses the *percent-encoded* wire form, not a
    // raw path with an unencoded space — a real LSP client never sends the
    // latter (RFC 3986 requires encoding).
    let base = std::env::temp_dir();
    for path in [
        base.join("php-lsp test dir").join("Foo.php"),
        base.join("php-lsp-é-测试").join("Bar.php"),
        base.join("has spaces")
            .join("and-é-unicode")
            .join("Baz.php"),
    ] {
        let from_path = Uri::from_file_path(&path)
            .unwrap_or_else(|| panic!("from_file_path failed for {path:?}"));
        let wire_string = from_path.as_str().to_string();
        let parsed: Uri = wire_string
            .parse()
            .unwrap_or_else(|e| panic!("re-parsing {wire_string:?} (from {path:?}) failed: {e:?}"));
        assert_eq!(
            from_path, parsed,
            "Uri::from_file_path and re-parsing its own wire string disagree for {path:?}"
        );

        // Hash equality matters as much as `==`: DocumentStore's DashMap
        // relies on it to look up the same file regardless of which
        // construction path produced the key.
        let mut hasher_a = std::collections::hash_map::DefaultHasher::new();
        let mut hasher_b = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&from_path, &mut hasher_a);
        std::hash::Hash::hash(&parsed, &mut hasher_b);
        assert_eq!(
            std::hash::Hasher::finish(&hasher_a),
            std::hash::Hasher::finish(&hasher_b),
            "hash mismatch between equal Uris for {path:?}"
        );
    }
}

#[test]
fn uri_to_file_path_roundtrips_space_and_unicode_paths() {
    // The two most common ways a percent-encoding bug would show up: a
    // project directory with a space (e.g. under "My Documents"), or with
    // accented/non-ASCII characters.
    let base = std::env::temp_dir();
    for path in [
        base.join("php-lsp test dir").join("Foo.php"),
        base.join("php-lsp-é-测试").join("Bar.php"),
    ] {
        let uri = Uri::from_file_path(&path)
            .unwrap_or_else(|| panic!("from_file_path failed for {path:?}"));
        let back = uri
            .to_file_path()
            .unwrap_or_else(|| panic!("to_file_path failed for {path:?}"));
        assert_eq!(
            back.as_ref(),
            path.as_path(),
            "round-trip mismatch for {path:?}"
        );
    }
}

// defer_actions tests
#[test]
fn defer_actions_strips_edit_and_adds_data() {
    let uri = ("file:///test.php").parse::<Uri>().unwrap();
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
    let uri = ("file:///test.php").parse::<Uri>().unwrap();
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
    let uri = ("file:///test.php").parse::<Uri>().unwrap();
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

// mir reports the namespace-resolved attempt (e.g. `App\Widget` for a bare
// `Widget` reference inside `namespace App;`), not the token the developer
// wrote — the handler must take the last `\`-segment before doing an index
// lookup by short name, or the quick-fix never fires in namespaced files.
#[test]
fn undefined_class_name_strips_namespace_resolved_prefix() {
    let msg = "Class App\\Service\\Widget does not exist";
    let resolved = msg
        .strip_prefix("Class ")
        .and_then(|s| s.strip_suffix(" does not exist"))
        .unwrap_or("")
        .trim();
    let short = resolved.rsplit('\\').next().unwrap_or(resolved);
    assert_eq!(short, "Widget");
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

#[test]
fn position_to_byte_offset_first_line() {
    let src = "<?php\nfoo();";
    // Character 0 → byte 0.
    assert_eq!(
        position_to_byte_offset_strict(
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
        position_to_byte_offset_strict(
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
        position_to_byte_offset_strict(
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
        position_to_byte_offset_strict(
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
        position_to_byte_offset_strict(
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
        position_to_byte_offset_strict(
            src,
            Position {
                line: 1,
                character: 0
            }
        ),
        None
    );
    assert_eq!(
        position_to_byte_offset_strict(
            src,
            Position {
                line: 5,
                character: 0
            }
        ),
        None
    );
}

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
    let doc =
        ParsedDoc::parse("<?php\ninterface I {\n    public function add(): void;\n}".to_string());
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
        "<?php\nenum Status {\n    public function label(): string { return 'x'; }\n}".to_string(),
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
