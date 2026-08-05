use std::sync::Arc;

use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;

use crate::actions::add_throws_action::add_throws_actions;
use crate::actions::arrow_function_action::{
    arrow_function_to_closure_actions, closure_to_arrow_function_actions,
};
use crate::actions::extract_action::extract_variable_actions;
use crate::actions::extract_constant_action::extract_constant_actions;
use crate::actions::extract_interface_action::extract_interface_actions;
use crate::actions::extract_method_action::extract_method_actions;
use crate::actions::facade_to_di_action::facade_to_di_actions;
use crate::actions::generate_validation_rules_action::generate_validation_rules_actions;
use crate::actions::inline_action::inline_variable_actions;
use crate::actions::local_to_property_action::local_to_property_actions;
use crate::actions::route_scaffold_action::unknown_route_actions;
use crate::actions::switch_to_match_action::switch_to_match_actions;
use crate::actions::update_phpdoc_action::update_phpdoc_actions;
use crate::actions::visibility_action::change_visibility_actions;
use crate::editing::organize_imports::organize_imports_action;
use crate::editing::use_import::{
    build_use_function_import_edit, build_use_import_edit, find_fqn_for_class,
    find_fqn_for_function,
};

use super::super::Backend;
use super::super::helpers::{DEFERRED_ACTION_TAGS, defer_actions, generate_deferred_actions};

/// Whether `actual` satisfies a `context.only` entry of `requested` — either
/// an exact match or a more specific descendant (`refactor` covers
/// `refactor.extract`, per the LSP code-action-kind hierarchy).
fn kind_matches(requested: &CodeActionKind, actual: &CodeActionKind) -> bool {
    let requested = requested.as_str();
    let actual = actual.as_str();
    actual == requested || actual.starts_with(&format!("{requested}."))
}

impl Backend {
    pub(crate) async fn handle_code_action(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri.clone();
        let source = self.get_open_text(&uri).unwrap_or_default();
        let doc = match self.get_doc(&uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let diag_cfg = self.config.load().diagnostics.clone();
        let docs = Arc::clone(&self.docs);
        let laravel = self.laravel.load_full();
        let laravel_root = self.root_paths.load().first().cloned();
        let range = params.range;
        let only = params.context.only;

        // Every step below is CPU-bound tree-walking with no `.await` of its
        // own (semantic-issue lookup, whole-workspace index scans, ~12
        // whole-file AST walkers) — run it all on the blocking pool in one
        // hop instead of on the tokio worker thread, consistent with
        // hover/completion/inlay_hint.
        let actions = tokio::task::spawn_blocking(move || {
            let sem_diags = docs
                .get_semantic_issues_salsa(&uri)
                .map(|issues| {
                    crate::semantic_diagnostics::issues_to_diagnostics(&issues, &uri, &diag_cfg)
                })
                .unwrap_or_default();

            let mut actions: Vec<CodeActionOrCommand> = Vec::new();
            let wi = docs.get_workspace_index_salsa();
            {
                let class_candidates = |short: &str| docs.class_candidates(&wi, short);
                let resolve_class_fqn = |cr| wi.at(cr).map(|(_, cls)| cls.fqn.to_string());
                let get_doc = |uri: &Uri| docs.get_doc_salsa(uri);
                let function_candidates = |name: &str| docs.declaration_candidate_files(&wi, name);
                for diag in &sem_diags {
                    if diag.code != Some(NumberOrString::String("UndefinedClass".to_string())) {
                        continue;
                    }
                    if diag.range.start.line < range.start.line
                        || diag.range.start.line > range.end.line
                    {
                        continue;
                    }
                    let resolved_name = diag
                        .message
                        .strip_prefix("Class ")
                        .and_then(|s| s.strip_suffix(" does not exist"))
                        .unwrap_or("")
                        .trim();
                    if resolved_name.is_empty() {
                        continue;
                    }
                    // `resolved_name` is mir's namespace-resolved attempt (e.g. `App\Widget`
                    // for a bare `Widget` reference inside `namespace App;`), not the token
                    // the developer wrote — take the last segment to recover the short name
                    // the workspace index stores classes under.
                    let class_name = resolved_name.rsplit('\\').next().unwrap_or(resolved_name);
                    if let Some(fqn) =
                        find_fqn_for_class(class_name, &class_candidates, &resolve_class_fqn)
                    {
                        let edit = build_use_import_edit(&source, &uri, &fqn);
                        let action = CodeAction {
                            title: format!("Add use {fqn}"),
                            kind: Some(CodeActionKind::QUICKFIX),
                            edit: Some(edit),
                            diagnostics: Some(vec![diag.clone()]),
                            ..Default::default()
                        };
                        actions.push(CodeActionOrCommand::CodeAction(action));
                    }
                }

                // UndefinedFunction → use function FQN;
                for diag in &sem_diags {
                    if diag.code != Some(NumberOrString::String("UndefinedFunction".to_string())) {
                        continue;
                    }
                    if diag.range.start.line < range.start.line
                        || diag.range.start.line > range.end.line
                    {
                        continue;
                    }
                    let fn_name = diag
                        .message
                        .strip_prefix("Function ")
                        .and_then(|s| s.strip_suffix("() is not defined"))
                        .unwrap_or("")
                        .trim();
                    if fn_name.is_empty() {
                        continue;
                    }
                    if let Some(fqn) =
                        find_fqn_for_function(fn_name, &get_doc, &function_candidates)
                    {
                        let edit = build_use_function_import_edit(&source, &uri, &fqn);
                        let action = CodeAction {
                            title: format!("Add use function {fqn}"),
                            kind: Some(CodeActionKind::QUICKFIX),
                            edit: Some(edit),
                            diagnostics: Some(vec![diag.clone()]),
                            ..Default::default()
                        };
                        actions.push(CodeActionOrCommand::CodeAction(action));
                    }
                }
            }

            for tag in DEFERRED_ACTION_TAGS {
                actions.extend(defer_actions(
                    generate_deferred_actions(&docs, tag, &source, &doc, range, &uri),
                    tag,
                    &uri,
                    range,
                ));
            }

            actions.extend(extract_variable_actions(&source, &doc, range, &uri));
            actions.extend(extract_method_actions(&source, &doc, range, &uri));
            actions.extend(extract_constant_actions(&source, range, &uri));
            actions.extend(inline_variable_actions(&source, range, &uri));
            actions.extend(change_visibility_actions(&source, &doc, range, &uri));
            actions.extend(closure_to_arrow_function_actions(
                &source, &doc, range, &uri,
            ));
            actions.extend(arrow_function_to_closure_actions(
                &source, &doc, range, &uri,
            ));
            actions.extend(switch_to_match_actions(&source, &doc, range, &uri));
            actions.extend(extract_interface_actions(&source, &doc, range, &uri));
            actions.extend(local_to_property_actions(&source, &doc, range, &uri));
            actions.extend(update_phpdoc_actions(&uri, &doc, range));
            actions.extend(add_throws_actions(&uri, &doc, range));
            actions.extend(crate::laravel::missing_key_actions(
                &doc,
                range.start,
                &laravel,
                laravel_root.as_deref(),
            ));
            actions.extend(unknown_route_actions(
                &doc,
                range.start,
                &laravel,
                laravel_root.as_deref(),
                &docs,
            ));
            actions.extend(facade_to_di_actions(
                &source,
                &doc,
                range,
                &uri,
                laravel.is_laravel,
            ));
            actions.extend(generate_validation_rules_actions(
                &source,
                &doc,
                range,
                &uri,
                laravel.is_laravel,
            ));
            if let Some(action) = organize_imports_action(&source, &uri) {
                actions.push(action);
            }
            actions
        })
        .await
        .unwrap_or_default();

        // `context.only` restricts results to the requested kinds (and their
        // descendants, e.g. `refactor` also matches `refactor.extract`) — a
        // client asking for just quickfixes shouldn't have to filter out
        // every refactor/organize-imports action itself.
        let actions = match only {
            Some(kinds) => actions
                .into_iter()
                .filter(|action| match action {
                    CodeActionOrCommand::CodeAction(ca) => ca.kind.as_ref().is_some_and(|kind| {
                        kinds.iter().any(|only_kind| kind_matches(only_kind, kind))
                    }),
                    CodeActionOrCommand::Command(_) => true,
                })
                .collect(),
            None => actions,
        };

        Ok(if actions.is_empty() {
            None
        } else {
            Some(actions)
        })
    }

    pub(crate) async fn handle_code_action_resolve(&self, item: CodeAction) -> Result<CodeAction> {
        let data = match &item.data {
            Some(d) => d.clone(),
            None => return Ok(item),
        };
        let kind_tag = match data.get("php_lsp_resolve").and_then(|v| v.as_str()) {
            Some(k) => k.to_string(),
            None => return Ok(item),
        };
        let uri: Uri = match data
            .get("uri")
            .and_then(|v| v.as_str())
            .and_then(|s| (s).parse::<Uri>().ok())
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

        let docs = Arc::clone(&self.docs);
        let fallback = item.clone();
        // `generate_deferred_actions` can run a full-workspace scan (the
        // "implement" tag's Aho-Corasick search over every cached doc);
        // keep it off the async runtime worker, matching `handle_code_action`.
        let resolved = tokio::task::spawn_blocking(move || {
            let candidates =
                generate_deferred_actions(&docs, &kind_tag, &source, &doc, range, &uri);
            for candidate in candidates {
                if let CodeActionOrCommand::CodeAction(ca) = candidate
                    && ca.title == item.title
                {
                    return ca;
                }
            }
            item
        })
        .await
        .unwrap_or(fallback);

        Ok(resolved)
    }
}
