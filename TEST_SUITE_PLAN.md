# Test Suite Migration Plan

Goal: remove all 40 `tests/e2e_*.rs` files and have a single implementation of server-wiring
tests: the `tests/feature_*.rs` suite backed by `tests/common/TestServer`.

Both suites already go through the full JSON-RPC wire protocol via the same `TestServer`
harness. Migration is a straight lift — no harness changes needed.

---

## Principle

One suite. Every test is a `feature_*.rs` file that:

1. Builds a `TestServer` (real in-process LSP server over an in-memory channel).
2. Sends LSP notifications / requests via the `check_*` helpers or raw `TestServer` methods.
3. Asserts with `expect![[...]]` snapshots or the caret-annotation DSL (`// ^^^ error: …`).

---

## Current state (after Group B completion)

```
tests/e2e_*.rs     — 18 files  ← still to migrate (Groups C remain)
tests/feature_*.rs — 22 files  ← extended
```

---

## Group A — ✅ Done (9 files deleted)

| Deleted | Covered by |
|---|---|
| `e2e_call_hierarchy.rs` | `feature_hierarchy.rs` |
| `e2e_code_action_resolve.rs` | `feature_code_actions.rs` |
| `e2e_completion.rs` | `feature_completion.rs` |
| `e2e_diagnostics.rs` | `feature_diagnostics.rs` |
| `e2e_references.rs` | `feature_references.rs` |
| `e2e_document_symbols.rs` | `feature_symbols.rs` |
| `e2e_declaration.rs` | `feature_definition.rs` |
| `e2e_type_definition.rs` | `feature_definition.rs` |
| `e2e_implementation.rs` | `feature_definition.rs` |

---

## Group B — Port into existing feature files (7 files, ~25 tests)

Add the listed tests to the named file, then delete the e2e source.

### `feature_folding.rs` — ✅ Done (`e2e_selection_range.rs` deleted)
- [x] `selection_range_expands_from_position` (strengthened: monotonic expansion check)

### `feature_diagnostics.rs` — ✅ Done (`e2e_workspace_diagnostics.rs` deleted)
- [x] `pull_diagnostics_returns_report` (strengthened: pinned kind='full', items=0)
- [x] `workspace_diagnostic_returns_report` (strengthened: exact item count)

### `feature_definition.rs` — ✅ Done (`e2e_cross_file.rs` deleted, tests re-routed)
- [x] `goto_definition_resolves_use_import_across_files` (strengthened: line assertion)
- [x] `goto_definition_method_call_across_files` (strengthened: line assertion)

### `feature_references.rs` — ✅ Done (from `e2e_cross_file.rs`)
- [x] `references_include_use_imports_across_files` (strengthened: count >= 2)

### `feature_symbols.rs` — ✅ Done (`e2e_symbol_resolve.rs` deleted; `e2e_cross_file.rs` deleted)
- [x] `workspace_symbol_finds_class_by_short_name` (from `e2e_cross_file.rs`)
- [x] `symbol_resolve_fills_range_for_open_file` (strengthened: line + char assertions)
- [x] `symbol_resolve_unchanged_for_closed_file`
- [x] `symbol_resolve_passthrough_for_already_resolved_location` (strengthened: end.line added)

### `feature_rename.rs` — ✅ Done (`e2e_file_rename.rs` deleted; `e2e_cross_file.rs` deleted)
- [x] `rename_class_edits_all_dependents` (from `e2e_cross_file.rs`, snapshot-pinned)
- [x] `will_rename_file_rewrites_use_imports_in_dependents` (snapshot-pinned)
- [x] `will_rename_file_same_psr4_fqn_produces_no_edits`
- [x] `will_delete_file_strips_use_imports_from_dependents` (snapshot-pinned)

### `feature_hover.rs` — ✅ Done (`e2e_traits.rs` deleted)
- [x] `hover_trait_inherited_method`
- [x] `hover_multi_trait_alpha` / `hover_multi_trait_beta`
- [x] `hover_on_empty_file_returns_null_not_error`
- [x] `hover_past_eof_does_not_crash`

### `feature_definition.rs` — ✅ Done (from `e2e_traits.rs` + `e2e_robustness.rs`)
- [x] `definition_trait_use_resolves_to_trait_decl`
- [x] `definition_trait_method_via_this`
- [x] `definition_on_unknown_symbol_returns_null`

### `feature_completion.rs` — ✅ Done (`e2e_traits.rs` deleted)
- [x] `completion_this_arrow_includes_trait_methods` (kept `#[ignore]`)

### `feature_references.rs` — ✅ Done (from `e2e_robustness.rs`)
- [x] `references_on_unopened_uri_returns_empty`

### `feature_rename.rs` — ✅ Done (from `e2e_robustness.rs`)
- [x] `rename_on_nonexistent_symbol_does_not_error`

### `feature_diagnostics.rs` — ✅ Done (from `e2e_robustness.rs`)
- [x] `requests_on_parse_error_file_do_not_error` (already present)

---

## Group C — Create new feature files (10 files, ~79 tests)

Each new file is a straight migration of the e2e source. Use `check_*` helpers and
`expect![[...]]` snapshots throughout; replace any manual JSON path extraction.

### `feature_formatting.rs` (4 tests, from `e2e_formatting.rs`)
- [ ] `formatting_returns_null_or_valid_edits`
- [ ] `range_formatting_returns_null_or_valid_edits`
- [ ] `on_type_formatting_unknown_trigger_returns_null`
- [ ] `on_type_formatting_close_brace_deindents`

### `feature_semantic_tokens.rs` (6 tests, from `e2e_semantic_tokens.rs`)
- [ ] `semantic_tokens_full_returned`
- [ ] `semantic_tokens_range_returns_data`
- [ ] `semantic_tokens_full_delta_returns_result`
- [ ] `semantic_tokens_delta_with_stale_previous_result_id_degrades_to_full`
- [ ] `semantic_tokens_delta_without_baseline_degrades_to_full`
- [ ] `semantic_tokens_delta_after_didchange_reflects_new_content`

### `feature_document_link.rs` (6 tests, from `e2e_document_link.rs`)
- [ ] `document_link_multiple_requires_produce_multiple_links`
- [ ] `document_link_docblock_at_link_produces_http_link`
- [ ] `document_link_at_see_class_ref_produces_no_link`
- [ ] `document_link_plain_file_returns_null`
- [ ] `document_link_require_target_is_file_uri`
- [ ] `document_link_range_is_inside_quotes`

### `feature_file_ops.rs` (7 tests, from `e2e_file_ops.rs` + `e2e_file_create_stub.rs`)
- [ ] `will_rename_files_outside_psr4_returns_null` (from `e2e_file_ops.rs`)
- [ ] `will_create_files_returns_workspace_edit_with_stub` (from `e2e_file_ops.rs`)
- [ ] `will_delete_files_outside_psr4_returns_null` (from `e2e_file_ops.rs`)
- [ ] `will_create_files_psr4_mapped_generates_namespace_stub` (from `e2e_file_create_stub.rs`)
- [ ] `will_create_files_outside_psr4_root_generates_minimal_stub` (from `e2e_file_create_stub.rs`)
- [ ] `will_create_files_root_namespace_generates_stub_without_namespace` (from `e2e_file_create_stub.rs`)
- [ ] `will_create_files_multiple_files_get_independent_stubs` (from `e2e_file_create_stub.rs`)

### `feature_doc_lifecycle.rs` (9 tests, from `e2e_doc_lifecycle.rs` + `e2e_file_notifications.rs`)
- [ ] `did_close_clears_diagnostics` (from `e2e_doc_lifecycle.rs`)
- [ ] `did_close_unopened_does_not_crash` (from `e2e_doc_lifecycle.rs`)
- [ ] `did_save_republishes_empty_diagnostics_for_clean_file` (from `e2e_doc_lifecycle.rs`)
- [ ] `did_save_republishes_diagnostics_for_duplicate_functions` (from `e2e_doc_lifecycle.rs`)
- [ ] `will_save_wait_until_returns_null_or_empty_for_formatted_file` (from `e2e_doc_lifecycle.rs`)
- [ ] `will_save_wait_until_returns_null_or_edits_for_unformatted_file` (from `e2e_doc_lifecycle.rs`)
- [ ] `did_change_updates_document` (from `e2e_lifecycle.rs`)
- [ ] `document_link_returns_array` (from `e2e_protocol.rs` — verifies endpoint is wired)
- [ ] `inline_value_returns_array` (from `e2e_protocol.rs`)

### `feature_incremental.rs` (13 tests, from `e2e_incremental.rs`)
- [ ] `hover_reflects_didchange_new_symbol`
- [ ] `definition_cache_is_invalidated_after_didchange`
- [ ] `references_reflect_didchange_additions_and_removals`
- [ ] `diagnostics_replaced_not_appended_on_didchange`
- [ ] `cross_file_diagnostics_refresh_on_next_didchange`
- [ ] `cross_file_diagnostics_republish_on_dependency_change`
- [ ] `true_burst_didchange_converges_to_final_text`
- [ ] `reopen_does_not_duplicate_symbols`
- [ ] `cross_file_diagnostic_clears_when_dependency_opened`
- [ ] `cross_file_republish_fans_out_to_multiple_dependents`
- [ ] `cross_file_republish_skips_closed_files`
- [ ] `cross_file_republish_uses_empty_array_for_clean_dependent`
- [ ] `cross_file_republish_preserves_dependent_parse_errors`

### `feature_workspace_scan.rs` (6 tests, from `e2e_workspace_scan.rs`)
- [ ] `created_file_becomes_discoverable_via_workspace_symbols`
- [ ] `created_file_in_new_subdirectory_is_indexed`
- [ ] `changed_file_updates_workspace_index`
- [ ] `deleted_file_symbols_removed_from_index`
- [ ] `exclude_paths_honored_by_workspace_scan`
- [ ] `php_lsp_json_exclude_paths_honored`

### `feature_workspace_folders.rs` (12 tests, from `e2e_workspace_folders.rs` + `e2e_watched_files.rs`)
- [ ] `add_workspace_folder_indexes_php_classes` (from `e2e_workspace_folders.rs`)
- [ ] `add_empty_workspace_folder_does_not_crash` (from `e2e_workspace_folders.rs`)
- [ ] `add_workspace_folder_idempotent_on_duplicate` (from `e2e_workspace_folders.rs`)
- [ ] `remove_workspace_folder_does_not_crash_and_keeps_indexed_docs` (from `e2e_workspace_folders.rs`)
- [ ] `workspace_without_composer_json_still_works` (from `e2e_workspace_scan.rs`)
- [ ] `nonexistent_psr4_dir_does_not_crash_server` (from `e2e_workspace_scan.rs`)
- [ ] `malformed_composer_json_does_not_crash_server` (from `e2e_workspace_scan.rs`)
- [ ] `did_rename_files_updates_index_to_new_path` (from `e2e_watched_files.rs`)
- [ ] `did_create_files_adds_new_class_to_index` (from `e2e_watched_files.rs`)
- [ ] `did_delete_files_removes_class_and_clears_diagnostics` (from `e2e_watched_files.rs`)
- [ ] `changed_event_does_not_overwrite_open_editor_file` (from `e2e_watched_files.rs`)
- [ ] `batch_changes_all_applied` (from `e2e_watched_files.rs`)

### `feature_configuration.rs` (5 tests, from `e2e_configuration.rs`)
- [ ] `change_configuration_valid_php_version_is_logged`
- [ ] `change_configuration_invalid_php_version_logs_warning`
- [ ] `change_configuration_triggers_semantic_token_refresh`
- [ ] `change_configuration_can_be_called_twice`
- [ ] `change_configuration_empty_config_uses_detected_version`

### `feature_server.rs` (6 tests, from `e2e_lifecycle.rs` + `e2e_protocol.rs` + `e2e_concurrent.rs`)
- [ ] `initialize_returns_server_capabilities` (from `e2e_lifecycle.rs`)
- [ ] `shutdown_responds_correctly` (from `e2e_lifecycle.rs`)
- [ ] `moniker_returns_no_error` (from `e2e_protocol.rs`)
- [ ] `linked_editing_range_returns_no_error` (from `e2e_protocol.rs`)
- [ ] `many_files_hover_each_returns_own_signature` (from `e2e_concurrent.rs`)
- [ ] `sustained_hover_volley_all_succeed` (from `e2e_concurrent.rs`)
- [ ] `didchange_followed_by_request_sees_new_state_every_iteration` (from `e2e_concurrent.rs`)
- [ ] `request_after_close_and_reopen_returns_fresh_data` (from `e2e_concurrent.rs`)

---

## Group D — ✅ Done (9 symfony files renamed)

`e2e_symfony_*.rs` → `feature_symfony_*.rs`. No content changes — they use `TestServer` and
the `locate()` needle-search pattern which is appropriate for real fixture files without `$0`.

---

## Migration order

1. ~~**Group A** — delete the 9 superseded files~~ ✅
2. ~~**Group D** — rename the symfony files~~ ✅
3. **Group B** — port remaining tests into existing feature files (selection_range, cross_file, symbol_resolve, file_rename still open); delete the source e2e file after each batch passes.
4. **Group C** — create 10 new `feature_*.rs` files. Suggested order by risk:
   - `feature_doc_lifecycle.rs` (simple lifecycle sequencing)
   - `feature_formatting.rs` (no analysis, just format round-trip)
   - `feature_document_link.rs` (no analysis)
   - `feature_file_ops.rs` (will_create/will_delete stubs)
   - `feature_semantic_tokens.rs` (delta logic)
   - `feature_workspace_scan.rs` (requires fixture workspace)
   - `feature_configuration.rs` (requires `change_configuration` helper)
   - `feature_server.rs` (concurrency + protocol stubs)
   - `feature_workspace_folders.rs` (multi-root + watched files)
   - `feature_incremental.rs` (cross-file republish — most complex, do last)

**Rule**: never delete an e2e file before its replacement passes in CI.

---

## Done state

```
tests/feature_*.rs   — 23 files, ~278 tests
tests/e2e_*.rs       — gone
```

`cargo test` runs one suite. `cargo test feature_` filters to the feature tier.
No `e2e_` prefix anywhere.
