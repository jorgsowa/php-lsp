use std::sync::Arc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

use crate::actions::add_throws_action::add_throws_actions;
use crate::actions::arrow_function_action::{
    arrow_function_to_closure_actions, closure_to_arrow_function_actions,
};
use crate::actions::extract_action::extract_variable_actions;
use crate::actions::extract_constant_action::extract_constant_actions;
use crate::actions::extract_interface_action::extract_interface_actions;
use crate::actions::extract_method_action::extract_method_actions;
use crate::actions::inline_action::inline_variable_actions;
use crate::actions::local_to_property_action::local_to_property_actions;
use crate::actions::switch_to_match_action::switch_to_match_actions;
use crate::actions::update_phpdoc_action::update_phpdoc_actions;
use crate::actions::visibility_action::change_visibility_actions;
use crate::editing::organize_imports::organize_imports_action;
use crate::editing::use_import::{
    build_use_function_import_edit, build_use_import_edit, find_fqn_for_class,
    find_fqn_for_function,
};

use super::super::Backend;
use super::super::helpers::{DEFERRED_ACTION_TAGS, defer_actions};

impl Backend {
    pub(crate) async fn handle_code_action(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
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

        let mut actions: Vec<CodeActionOrCommand> = Vec::new();
        self.docs.with_all_indexes(|all_indexes| {
            for diag in &sem_diags {
                if diag.code != Some(NumberOrString::String("UndefinedClass".to_string())) {
                    continue;
                }
                if diag.range.start.line < params.range.start.line
                    || diag.range.start.line > params.range.end.line
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
                if let Some(fqn) = find_fqn_for_class(class_name, all_indexes) {
                    let edit = build_use_import_edit(&source, uri, &fqn);
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
                if diag.range.start.line < params.range.start.line
                    || diag.range.start.line > params.range.end.line
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
                if let Some(fqn) = find_fqn_for_function(fn_name, all_indexes) {
                    let edit = build_use_function_import_edit(&source, uri, &fqn);
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
        });

        for tag in DEFERRED_ACTION_TAGS {
            actions.extend(defer_actions(
                self.generate_deferred_actions(tag, &source, &doc, params.range, uri),
                tag,
                uri,
                params.range,
            ));
        }

        actions.extend(extract_variable_actions(&source, &doc, params.range, uri));
        actions.extend(extract_method_actions(&source, &doc, params.range, uri));
        actions.extend(extract_constant_actions(&source, params.range, uri));
        actions.extend(inline_variable_actions(&source, params.range, uri));
        actions.extend(change_visibility_actions(&source, &doc, params.range, uri));
        actions.extend(closure_to_arrow_function_actions(
            &source,
            &doc,
            params.range,
            uri,
        ));
        actions.extend(arrow_function_to_closure_actions(
            &source,
            &doc,
            params.range,
            uri,
        ));
        actions.extend(switch_to_match_actions(&source, &doc, params.range, uri));
        actions.extend(extract_interface_actions(&source, &doc, params.range, uri));
        actions.extend(local_to_property_actions(&source, &doc, params.range, uri));
        actions.extend(update_phpdoc_actions(uri, &doc, params.range));
        actions.extend(add_throws_actions(uri, &doc, params.range));
        if let Some(action) = organize_imports_action(&source, uri) {
            actions.push(action);
        }

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
