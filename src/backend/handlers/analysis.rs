use std::sync::Arc;

use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;

use crate::analysis::diagnostics::merge_file_diagnostics;

use super::super::Backend;
use super::super::compute_diagnostic_result_id;

impl Backend {
    pub(crate) async fn handle_diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = &params.text_document.uri;
        let previous_result_id = params.previous_result_id.clone();
        let parse_diags = self.get_parse_diagnostics(uri).unwrap_or_default();
        let _doc = match self.get_doc(uri) {
            Some(d) => d,
            None => {
                let mut early_diags = parse_diags;
                early_diags.extend(self.open_files.external_diagnostics(uri));
                let _version = self
                    .open_files
                    .all_with_diagnostics()
                    .iter()
                    .find(|(u, _, _)| u == uri)
                    .and_then(|(_, _, v)| *v)
                    .unwrap_or(1);
                let result_id = compute_diagnostic_result_id(&early_diags, uri.as_str());
                if previous_result_id.as_deref() == Some(result_id.as_str()) {
                    return Ok(DocumentDiagnosticReportResult::Report(
                        DocumentDiagnosticReport::Unchanged(
                            RelatedUnchangedDocumentDiagnosticReport {
                                related_documents: None,
                                unchanged_document_diagnostic_report:
                                    UnchangedDocumentDiagnosticReport { result_id },
                            },
                        ),
                    ));
                }
                return Ok(DocumentDiagnosticReportResult::Report(
                    DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                        related_documents: None,
                        full_document_diagnostic_report: FullDocumentDiagnosticReport {
                            result_id: Some(result_id),
                            items: early_diags,
                        },
                    }),
                ));
            }
        };
        let (diag_cfg, php_version) = {
            let cfg = self.config.load();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };
        let _ = php_version;

        let docs = Arc::clone(&self.docs);
        let uri_owned = uri.clone();
        let diag_cfg_sem = diag_cfg.clone();
        let is_laravel = self.laravel.load_full().is_laravel;
        let sem_diags = tokio::task::spawn_blocking(move || {
            let mut diags = docs
                .get_semantic_issues_salsa(&uri_owned)
                .map(|issues| {
                    crate::semantic_diagnostics::issues_to_diagnostics_gated(
                        &issues,
                        &uri_owned,
                        &diag_cfg_sem,
                        docs.is_index_ready(),
                    )
                })
                .unwrap_or_default();
            diags.extend(crate::laravel::unguarded_model_diagnostics(
                &docs, &uri_owned, is_laravel,
            ));
            diags
        })
        .await
        .map_err(|e| {
            use std::borrow::Cow;
            tower_lsp_server::jsonrpc::Error {
                code: tower_lsp_server::jsonrpc::ErrorCode::InternalError,
                message: Cow::Owned(format!("diagnostic analysis failed: {}", e)),
                data: None,
            }
        })?;

        let mut items = merge_file_diagnostics(parse_diags, sem_diags);
        items.extend(self.open_files.external_diagnostics(uri));

        let _version = self
            .open_files
            .all_with_diagnostics()
            .iter()
            .find(|(u, _, _)| u == uri)
            .and_then(|(_, _, v)| *v)
            .unwrap_or(1);
        let result_id = compute_diagnostic_result_id(&items, uri.as_str());

        if previous_result_id.as_deref() == Some(result_id.as_str()) {
            return Ok(DocumentDiagnosticReportResult::Report(
                DocumentDiagnosticReport::Unchanged(RelatedUnchangedDocumentDiagnosticReport {
                    related_documents: None,
                    unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport {
                        result_id,
                    },
                }),
            ));
        }

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

    pub(crate) async fn handle_workspace_diagnostic(
        &self,
        params: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReportResult> {
        let all_parse_diags = self.all_open_files_with_diagnostics();
        let (diag_cfg, php_version) = {
            let cfg = self.config.load();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };
        let _ = php_version;

        let previous_map: std::collections::HashMap<Uri, String> = params
            .previous_result_ids
            .into_iter()
            .map(|p| (p.uri, p.value))
            .collect();

        let docs = Arc::clone(&self.docs);
        let open_files = self.open_files.clone();
        let diag_cfg_sweep = diag_cfg.clone();
        let is_laravel = self.laravel.load_full().is_laravel;
        let items = tokio::task::spawn_blocking(move || {
            // A user-facing pull: pause the background scan for the sweep and
            // snapshot a settled revision, so only a genuine user edit aborts
            // the sweep below.
            let (_interactive, cancel_rev) = docs.settled_write_rev_guard();
            let mut results = Vec::new();
            for (uri, parse_diags, version) in all_parse_diags {
                // A write during the sweep means the results are immediately
                // stale. Return empty so the editor re-requests after the edit.
                if docs.write_rev() != cancel_rev {
                    return vec![];
                }
                let sem_diags = docs
                    .get_semantic_issues_salsa(&uri)
                    .map(|issues| {
                        crate::semantic_diagnostics::issues_to_diagnostics_gated(
                            &issues,
                            &uri,
                            &diag_cfg_sweep,
                            docs.is_index_ready(),
                        )
                    })
                    .unwrap_or_default();
                let mut all_diags = merge_file_diagnostics(parse_diags, sem_diags);
                all_diags.extend(crate::laravel::unguarded_model_diagnostics(
                    &docs, &uri, is_laravel,
                ));
                all_diags.extend(open_files.external_diagnostics(&uri));
                let result_id = compute_diagnostic_result_id(&all_diags, uri.as_str());
                results.push(if previous_map.get(&uri) == Some(&result_id) {
                    WorkspaceDocumentDiagnosticReport::Unchanged(
                        WorkspaceUnchangedDocumentDiagnosticReport {
                            uri,
                            version,
                            unchanged_document_diagnostic_report:
                                UnchangedDocumentDiagnosticReport { result_id },
                        },
                    )
                } else {
                    WorkspaceDocumentDiagnosticReport::Full(WorkspaceFullDocumentDiagnosticReport {
                        uri,
                        version,
                        full_document_diagnostic_report: FullDocumentDiagnosticReport {
                            result_id: Some(result_id),
                            items: all_diags,
                        },
                    })
                });
            }
            results
        })
        .await
        .map_err(|e| {
            use std::borrow::Cow;
            tower_lsp_server::jsonrpc::Error {
                code: tower_lsp_server::jsonrpc::ErrorCode::InternalError,
                message: Cow::Owned(format!("workspace_diagnostic analysis failed: {}", e)),
                data: None,
            }
        })?;

        Ok(WorkspaceDiagnosticReportResult::Report(
            WorkspaceDiagnosticReport { items },
        ))
    }
}
