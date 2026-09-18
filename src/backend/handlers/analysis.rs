use std::sync::Arc;

use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;

use crate::document::open_files::compute_open_file_diagnostics;

use super::super::Backend;
use super::super::compute_diagnostic_result_id;

impl Backend {
    pub(crate) async fn handle_diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = &params.text_document.uri;
        let previous_result_id = params.previous_result_id.clone();
        let (diag_cfg, php_version) = {
            let cfg = self.config.load();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };
        let _ = php_version;

        let items = if self.open_files.contains(uri) {
            let docs = Arc::clone(&self.docs);
            let open_files = self.open_files.clone();
            let uri_owned = uri.clone();
            let is_laravel = self.laravel.load_full().is_laravel;
            tokio::task::spawn_blocking(move || {
                compute_open_file_diagnostics(&docs, &open_files, &uri_owned, &diag_cfg, is_laravel)
            })
            .await
            .map_err(|e| {
                use std::borrow::Cow;
                tower_lsp_server::jsonrpc::Error {
                    code: tower_lsp_server::jsonrpc::ErrorCode::InternalError,
                    message: Cow::Owned(format!("diagnostic analysis failed: {}", e)),
                    data: None,
                }
            })?
        } else {
            Vec::new()
        };
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
        let open_files_with_versions = self.all_open_files_with_diagnostic_versions();
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
            for (uri, version) in open_files_with_versions {
                // A write during the sweep means the results are immediately
                // stale. Return empty so the editor re-requests after the edit.
                if docs.write_rev() != cancel_rev {
                    return vec![];
                }
                let all_diags = compute_open_file_diagnostics(
                    &docs,
                    &open_files,
                    &uri,
                    &diag_cfg_sweep,
                    is_laravel,
                );
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
