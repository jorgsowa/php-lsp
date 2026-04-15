/// Semantic diagnostics bridge.
///
/// Delegates all analysis to the `mir-analyzer` crate and converts its `Issue`
/// type into the `tower-lsp` `Diagnostic` type expected by the LSP backend.
use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};

use crate::ast::{ParsedDoc, offset_to_position};
use crate::backend::DiagnosticsConfig;
use crate::docblock::{docblock_before, parse_docblock};

/// Run semantic checks on `doc` using the backend's persistent codebase.
/// The codebase is updated incrementally: the current file's definitions are
/// evicted and re-collected, then `finalize()` rebuilds inheritance tables.
///
/// `php_version` is a version string like `"8.1"` sourced from `LspConfig`.
/// It is threaded through here so callers are already wired correctly once
/// `mir_analyzer` gains version-gating support.
///
/// TODO: pass `php_version` to `mir_analyzer` once it exposes a version API.
pub fn semantic_diagnostics(
    uri: &Url,
    doc: &ParsedDoc,
    codebase: &mir_codebase::Codebase,
    cfg: &DiagnosticsConfig,
    _php_version: Option<&str>,
) -> Vec<Diagnostic> {
    if !cfg.enabled {
        return vec![];
    }

    let file: Arc<str> = Arc::from(uri.as_str());

    // Incremental update: evict stale definitions for this file, re-collect,
    // and rebuild inheritance tables.
    codebase.remove_file_definitions(&file);
    let source_map = php_rs_parser::source_map::SourceMap::new(doc.source());
    let collector = mir_analyzer::collector::DefinitionCollector::new(
        codebase,
        file.clone(),
        doc.source(),
        &source_map,
    );
    let collector_issues = collector.collect(doc.program());
    codebase.finalize();

    // Pass 2: analyse function/method bodies in the current document.
    let mut issue_buffer = mir_issues::IssueBuffer::new();
    let mut symbols = Vec::new();
    let mut analyzer = mir_analyzer::stmt::StatementsAnalyzer::new(
        codebase,
        file.clone(),
        doc.source(),
        &source_map,
        &mut issue_buffer,
        &mut symbols,
    );
    let mut ctx = mir_analyzer::context::Context::new();
    analyzer.analyze_stmts(&doc.program().stmts, &mut ctx);

    collector_issues
        .into_iter()
        .chain(issue_buffer.into_issues())
        .filter(|i| !i.suppressed)
        .filter(|i| issue_passes_filter(i, cfg))
        .map(|i| to_lsp_diagnostic(i, uri))
        .collect()
}

/// Run semantic body analysis on `doc` assuming the codebase is already
/// finalized (all definitions collected, `finalize()` already called).
///
/// Unlike [`semantic_diagnostics`], this function does **not** mutate the
/// codebase — it skips the `remove_file_definitions` / re-collect / `finalize`
/// cycle. Intended for workspace diagnostic batch passes where the codebase is
/// built once upfront and `finalize()` is called a single time before the loop.
pub fn semantic_diagnostics_no_rebuild(
    uri: &Url,
    doc: &ParsedDoc,
    codebase: &mir_codebase::Codebase,
    cfg: &DiagnosticsConfig,
    _php_version: Option<&str>,
) -> Vec<Diagnostic> {
    if !cfg.enabled {
        return vec![];
    }

    let file: Arc<str> = Arc::from(uri.as_str());
    let source_map = php_rs_parser::source_map::SourceMap::new(doc.source());

    // Pass 2 only: analyse function/method bodies.
    // The codebase is already finalized — skip remove/re-collect/finalize so
    // that inheritance tables are not torn down and rebuilt for every file.
    let mut issue_buffer = mir_issues::IssueBuffer::new();
    let mut symbols = Vec::new();
    let mut analyzer = mir_analyzer::stmt::StatementsAnalyzer::new(
        codebase,
        file,
        doc.source(),
        &source_map,
        &mut issue_buffer,
        &mut symbols,
    );
    let mut ctx = mir_analyzer::context::Context::new();
    analyzer.analyze_stmts(&doc.program().stmts, &mut ctx);

    issue_buffer
        .into_issues()
        .into_iter()
        .filter(|i| !i.suppressed)
        .filter(|i| issue_passes_filter(i, cfg))
        .map(|i| to_lsp_diagnostic(i, uri))
        .collect()
}

/// Returns `true` if the mir-analyzer issue is allowed through by the config.
fn issue_passes_filter(issue: &mir_issues::Issue, cfg: &DiagnosticsConfig) -> bool {
    use mir_issues::IssueKind;
    match &issue.kind {
        IssueKind::UndefinedVariable { .. } | IssueKind::PossiblyUndefinedVariable { .. } => {
            cfg.undefined_variables
        }
        IssueKind::UndefinedFunction { .. } | IssueKind::UndefinedMethod { .. } => {
            cfg.undefined_functions
        }
        IssueKind::UndefinedClass { .. } => cfg.undefined_classes,
        IssueKind::InvalidReturnType { .. }
        | IssueKind::InvalidArgument { .. }
        | IssueKind::NullMethodCall { .. }
        | IssueKind::NullPropertyFetch { .. }
        | IssueKind::NullableReturnStatement { .. }
        | IssueKind::InvalidPropertyAssignment { .. }
        | IssueKind::InvalidOperand { .. } => cfg.type_errors,
        IssueKind::DeprecatedMethod { .. } | IssueKind::DeprecatedClass { .. } => {
            cfg.deprecated_calls
        }
        _ => true,
    }
}

/// Check for deprecated function/method calls and emit Warning diagnostics.
pub fn deprecated_call_diagnostics(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    cfg: &DiagnosticsConfig,
) -> Vec<Diagnostic> {
    if !cfg.enabled || !cfg.deprecated_calls {
        return vec![];
    }
    let mut diags = Vec::new();
    let all_sources: Vec<(&str, &ParsedDoc)> = std::iter::once((source, doc))
        .chain(other_docs.iter().map(|d| (d.source(), d.as_ref())))
        .collect();
    collect_deprecated_calls(source, &doc.program().stmts, &all_sources, &mut diags);
    diags
}

fn collect_deprecated_calls(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    all_sources: &[(&str, &ParsedDoc)],
    diags: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Expression(e) => {
                check_expr_for_deprecated(source, e, all_sources, diags);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_deprecated_calls(source, inner, all_sources, diags);
                }
            }
            StmtKind::Function(f) => {
                collect_deprecated_calls(source, &f.body, all_sources, diags);
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && let Some(body) = &m.body
                    {
                        collect_deprecated_calls(source, body, all_sources, diags);
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && let Some(body) = &m.body
                    {
                        collect_deprecated_calls(source, body, all_sources, diags);
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && let Some(body) = &m.body
                    {
                        collect_deprecated_calls(source, body, all_sources, diags);
                    }
                }
            }
            _ => {}
        }
    }
}

fn check_expr_for_deprecated(
    source: &str,
    expr: &php_ast::Expr<'_, '_>,
    all_sources: &[(&str, &ParsedDoc)],
    diags: &mut Vec<Diagnostic>,
) {
    if let ExprKind::Assign(a) = &expr.kind {
        check_expr_for_deprecated(source, a.value, all_sources, diags);
        return;
    }
    if let ExprKind::FunctionCall(call) = &expr.kind {
        if let ExprKind::Identifier(name) = &call.name.kind {
            let func_name = name;
            // Search all docs for this function's declaration
            for (src, d) in all_sources {
                if let Some(span_start) = find_function_span(d, func_name)
                    && let Some(raw) = docblock_before(src, span_start)
                {
                    let db = parse_docblock(&raw);
                    if db.is_deprecated() {
                        let start_pos = offset_to_position(source, call.name.span.start);
                        let end_pos = offset_to_position(source, call.name.span.end);
                        let msg = match &db.deprecated {
                            Some(m) if !m.is_empty() => {
                                format!("Deprecated: {} — {}", func_name.as_str(), m)
                            }
                            _ => format!("Deprecated: {}", func_name.as_str()),
                        };
                        diags.push(Diagnostic {
                            range: Range {
                                start: Position {
                                    line: start_pos.line,
                                    character: start_pos.character,
                                },
                                end: Position {
                                    line: end_pos.line,
                                    character: end_pos.character,
                                },
                            },
                            severity: Some(DiagnosticSeverity::WARNING),
                            source: Some("php-lsp".to_string()),
                            message: msg,
                            ..Default::default()
                        });
                        break;
                    }
                }
            }
        }
        // Recurse into arguments so nested calls are also checked.
        for arg in call.args.iter() {
            check_expr_for_deprecated(source, &arg.value, all_sources, diags);
        }
    }
    if let ExprKind::MethodCall(call) = &expr.kind {
        if let ExprKind::Identifier(name) = &call.method.kind {
            let method_name = name;
            for (src, d) in all_sources {
                if let Some(span_start) = find_method_span(d, method_name)
                    && let Some(raw) = docblock_before(src, span_start)
                {
                    let db = parse_docblock(&raw);
                    if db.is_deprecated() {
                        let start_pos = offset_to_position(source, call.method.span.start);
                        let end_pos = offset_to_position(source, call.method.span.end);
                        let msg = match &db.deprecated {
                            Some(m) if !m.is_empty() => {
                                format!("Deprecated: {} — {}", method_name.as_str(), m)
                            }
                            _ => format!("Deprecated: {}", method_name.as_str()),
                        };
                        diags.push(Diagnostic {
                            range: Range {
                                start: Position {
                                    line: start_pos.line,
                                    character: start_pos.character,
                                },
                                end: Position {
                                    line: end_pos.line,
                                    character: end_pos.character,
                                },
                            },
                            severity: Some(DiagnosticSeverity::WARNING),
                            source: Some("php-lsp".to_string()),
                            message: msg,
                            ..Default::default()
                        });
                        break;
                    }
                }
            }
        }
        // Recurse into object and arguments so nested calls are also checked.
        check_expr_for_deprecated(source, call.object, all_sources, diags);
        for arg in call.args.iter() {
            check_expr_for_deprecated(source, &arg.value, all_sources, diags);
        }
    }
}

fn find_function_span(doc: &ParsedDoc, func_name: &str) -> Option<u32> {
    find_function_span_in_stmts(&doc.program().stmts, func_name)
}

fn find_function_span_in_stmts(stmts: &[Stmt<'_, '_>], func_name: &str) -> Option<u32> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == func_name => {
                return Some(stmt.span.start);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = find_function_span_in_stmts(inner, func_name)
                {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_method_span(doc: &ParsedDoc, method_name: &str) -> Option<u32> {
    find_method_span_in_stmts(&doc.program().stmts, method_name)
}

fn find_method_span_in_stmts(stmts: &[Stmt<'_, '_>], method_name: &str) -> Option<u32> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return Some(member.span.start);
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return Some(member.span.start);
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return Some(member.span.start);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = find_method_span_in_stmts(inner, method_name)
                {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

/// Check for duplicate class/function/interface/trait/enum declarations.
pub fn duplicate_declaration_diagnostics(
    source: &str,
    doc: &ParsedDoc,
    cfg: &DiagnosticsConfig,
) -> Vec<Diagnostic> {
    if !cfg.enabled || !cfg.duplicate_declarations {
        return vec![];
    }
    let mut seen: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
    let mut diags = Vec::new();
    collect_duplicate_decls(source, &doc.program().stmts, "", &mut seen, &mut diags);
    diags
}

fn collect_duplicate_decls(
    source: &str,
    stmts: &[php_ast::Stmt<'_, '_>],
    current_ns: &str,
    seen: &mut std::collections::HashMap<String, ()>,
    diags: &mut Vec<Diagnostic>,
) {
    // Track the active namespace for unbraced `namespace Foo;` declarations.
    let mut active_ns = current_ns.to_string();

    for stmt in stmts {
        let name_and_span: Option<(&str, u32)> = match &stmt.kind {
            StmtKind::Class(c) => c.name.map(|n| (n, stmt.span.start)),
            StmtKind::Interface(i) => Some((i.name, stmt.span.start)),
            StmtKind::Trait(t) => Some((t.name, stmt.span.start)),
            StmtKind::Enum(e) => Some((e.name, stmt.span.start)),
            StmtKind::Function(f) => Some((f.name, stmt.span.start)),
            StmtKind::Namespace(ns) => {
                let ns_name = ns
                    .name
                    .as_ref()
                    .map(|n| n.to_string_repr().to_string())
                    .unwrap_or_default();
                match &ns.body {
                    php_ast::NamespaceBody::Braced(inner) => {
                        let child_ns = if current_ns.is_empty() {
                            ns_name
                        } else {
                            format!("{}\\{}", current_ns, ns_name)
                        };
                        collect_duplicate_decls(source, inner, &child_ns, seen, diags);
                    }
                    php_ast::NamespaceBody::Simple => {
                        // Unbraced namespace: subsequent siblings belong to this namespace.
                        active_ns = if current_ns.is_empty() {
                            ns_name
                        } else {
                            format!("{}\\{}", current_ns, ns_name)
                        };
                    }
                }
                None
            }
            _ => None,
        };
        if let Some((name, span_start)) = name_and_span {
            let key = if active_ns.is_empty() {
                name.to_string()
            } else {
                format!("{}\\{}", active_ns, name)
            };
            if seen.insert(key, ()).is_some() {
                // Find the byte offset of the actual name by searching forward from span_start.
                // The span_start points to keywords like "class", "function", etc.,
                // so we need to find where the identifier name appears.
                let name_byte_offset = find_name_offset(&source[span_start as usize..], name)
                    .map(|off| span_start + off as u32)
                    .unwrap_or(span_start);

                let start_pos = crate::ast::offset_to_position(source, name_byte_offset);
                // Calculate end position by converting UTF-8 character length to UTF-16 code units
                let name_utf16_len = name.chars().map(|c| c.len_utf16() as u32).sum::<u32>();
                let end_pos = Position {
                    line: start_pos.line,
                    character: start_pos.character + name_utf16_len,
                };
                diags.push(Diagnostic {
                    range: Range {
                        start: start_pos,
                        end: end_pos,
                    },
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "Duplicate declaration: `{name}` is already defined in this file"
                    ),
                    source: Some("php-lsp".to_string()),
                    ..Default::default()
                });
            }
        }
    }
}

/// Find the byte offset of an identifier name within a source slice.
/// Searches for word boundary matches (not substring matches).
fn find_name_offset(source: &str, name: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    for i in 0..source.len() {
        if source[i..].starts_with(name) {
            // Check word boundary before
            let before_ok = i == 0 || !is_identifier_char(bytes[i - 1] as char);
            // Check word boundary after
            let after_idx = i + name.len();
            let after_ok =
                after_idx >= source.len() || !is_identifier_char(bytes[after_idx] as char);
            if before_ok && after_ok {
                return Some(i);
            }
        }
    }
    None
}

/// Check if a character is valid in a PHP identifier.
fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn to_lsp_diagnostic(issue: mir_issues::Issue, _uri: &Url) -> Diagnostic {
    // mir-analyzer uses 1-based line numbers; LSP uses 0-based.
    let line = issue.location.line.saturating_sub(1);
    let col_start = issue.location.col_start as u32;
    let col_end = issue.location.col_end as u32;
    Diagnostic {
        range: Range {
            start: Position {
                line,
                character: col_start,
            },
            end: Position {
                line,
                character: col_end.max(col_start + 1),
            },
        },
        severity: Some(match issue.severity {
            mir_issues::Severity::Error => DiagnosticSeverity::ERROR,
            mir_issues::Severity::Warning => DiagnosticSeverity::WARNING,
            mir_issues::Severity::Info => DiagnosticSeverity::INFORMATION,
        }),
        code: Some(NumberOrString::String(issue.kind.name().to_string())),
        source: Some("php-lsp".to_string()),
        message: issue.kind.message(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deprecated_function_call_emits_warning() {
        let src =
            "<?php\n/** @deprecated Use newFunc() instead */\nfunction oldFunc() {}\n\noldFunc();";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = deprecated_call_diagnostics(src, &doc, &[], &DiagnosticsConfig::default());
        assert_eq!(
            diags.len(),
            1,
            "expected exactly 1 deprecated warning diagnostic"
        );
        let d = &diags[0];
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert!(
            d.message.contains("oldFunc"),
            "message should mention the function name"
        );
    }

    #[test]
    fn duplicate_class_emits_warning() {
        let src = "<?php\nclass Foo {}\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert_eq!(
            diags.len(),
            1,
            "expected exactly 1 duplicate warning, got: {:?}",
            diags
        );
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(
            diags[0].message.contains("Foo"),
            "message should mention 'Foo'"
        );
    }

    #[test]
    fn no_duplicate_for_unique_declarations() {
        let src = "<?php\nclass Foo {}\nclass Bar {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert!(diags.is_empty());
    }

    #[test]
    fn namespace_scoped_duplicate_not_flagged() {
        // Two classes named `Foo` in different namespaces — should produce zero diagnostics.
        let src = "<?php\nnamespace App\\A {\nclass Foo {}\n}\nnamespace App\\B {\nclass Foo {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert!(
            diags.is_empty(),
            "classes with same name in different namespaces should not be flagged, got: {:?}",
            diags
        );
    }

    #[test]
    fn duplicate_interface_declaration() {
        // Same interface defined twice in same file — should produce exactly one error.
        let src = "<?php\ninterface Logger {}\ninterface Logger {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert_eq!(
            diags.len(),
            1,
            "expected exactly 1 duplicate-declaration diagnostic, got: {:?}",
            diags
        );
        assert!(
            diags[0].message.contains("Logger"),
            "diagnostic message should mention 'Logger'"
        );
        assert_eq!(
            diags[0].severity,
            Some(DiagnosticSeverity::WARNING),
            "duplicate declaration should be a warning"
        );
    }

    #[test]
    fn duplicate_trait_declaration() {
        // Same trait defined twice in same file — should produce exactly one error.
        let src = "<?php\ntrait Serializable {}\ntrait Serializable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert_eq!(
            diags.len(),
            1,
            "expected exactly 1 duplicate-declaration diagnostic, got: {:?}",
            diags
        );
        assert!(
            diags[0].message.contains("Serializable"),
            "diagnostic message should mention 'Serializable'"
        );
        assert_eq!(
            diags[0].severity,
            Some(DiagnosticSeverity::WARNING),
            "duplicate trait declaration should be a warning"
        );
    }

    #[test]
    fn deprecated_method_call_emits_warning() {
        // Calling a method annotated @deprecated should emit a warning.
        let src = concat!(
            "<?php\n",
            "class Mailer {\n",
            "    /** @deprecated Use sendAsync() instead */\n",
            "    public function send() {}\n",
            "}\n",
            "$m = new Mailer();\n",
            "$m->send();\n",
        );
        let doc = ParsedDoc::parse(src.to_string());
        let diags = deprecated_call_diagnostics(src, &doc, &[], &DiagnosticsConfig::default());
        assert_eq!(
            diags.len(),
            1,
            "expected exactly 1 deprecated warning, got: {:?}",
            diags
        );
        let d = &diags[0];
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert!(
            d.message.contains("send"),
            "message should mention 'send', got: {}",
            d.message
        );
        assert!(
            d.message.to_lowercase().contains("deprecated"),
            "message should contain 'deprecated', got: {}",
            d.message
        );
    }

    #[test]
    fn deprecated_function_warning_has_correct_message() {
        // The deprecation warning message must contain the function name AND the
        // word "Deprecated" (case-sensitive per implementation: "Deprecated: …").
        let src = "<?php\n/** @deprecated old API */\nfunction legacyFn() {}\n\nlegacyFn();";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = deprecated_call_diagnostics(src, &doc, &[], &DiagnosticsConfig::default());
        assert_eq!(diags.len(), 1, "expected exactly 1 diagnostic");
        let msg = &diags[0].message;
        assert!(
            msg.contains("legacyFn"),
            "message should contain function name 'legacyFn', got: {msg}"
        );
        assert!(
            msg.to_lowercase().contains("deprecated"),
            "message should contain 'deprecated', got: {msg}"
        );
    }

    #[test]
    fn duplicate_diagnostic_has_warning_severity() {
        // Duplicate declarations are reported as WARNING by our implementation.
        // (Note: `duplicate_declaration_diagnostics` emits DiagnosticSeverity::WARNING.)
        let src = "<?php\nfunction doWork() {}\nfunction doWork() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert_eq!(diags.len(), 1, "expected exactly 1 duplicate diagnostic");
        assert_eq!(
            diags[0].severity,
            Some(DiagnosticSeverity::WARNING),
            "duplicate declaration diagnostic should have WARNING severity"
        );
    }

    #[test]
    fn deprecated_call_nested_in_argument_is_detected() {
        // A deprecated function call nested inside another call's argument must still warn.
        let src = concat!(
            "<?php\n",
            "/** @deprecated */\n",
            "function oldFn(): string { return ''; }\n",
            "function wrapper(string $s): void {}\n",
            "wrapper(oldFn());\n",
        );
        let doc = ParsedDoc::parse(src.to_string());
        let diags = deprecated_call_diagnostics(src, &doc, &[], &DiagnosticsConfig::default());
        assert_eq!(
            diags.len(),
            1,
            "expected 1 deprecation warning for nested call, got: {:?}",
            diags
        );
        assert!(
            diags[0].message.contains("oldFn"),
            "message should mention 'oldFn'"
        );
    }

    #[test]
    fn deprecated_method_in_trait_is_detected() {
        // A method annotated @deprecated in a trait should trigger a warning when called.
        let src = concat!(
            "<?php\n",
            "trait Logger {\n",
            "    /** @deprecated Use logAsync() instead */\n",
            "    public function log() {}\n",
            "}\n",
            "class App { use Logger; }\n",
            "$a = new App();\n",
            "$a->log();\n",
        );
        let doc = ParsedDoc::parse(src.to_string());
        let diags = deprecated_call_diagnostics(src, &doc, &[], &DiagnosticsConfig::default());
        assert_eq!(
            diags.len(),
            1,
            "expected 1 deprecated warning for trait method, got: {:?}",
            diags
        );
        assert!(
            diags[0].message.contains("log"),
            "message should mention 'log'"
        );
    }

    #[test]
    fn deprecated_method_in_enum_is_detected() {
        let src = concat!(
            "<?php\n",
            "enum Status {\n",
            "    case Active;\n",
            "    /** @deprecated Use activeLabel() instead */\n",
            "    public function label(): string { return 'active'; }\n",
            "}\n",
            "$s = Status::Active;\n",
            "$s->label();\n",
        );
        let doc = ParsedDoc::parse(src.to_string());
        let diags = deprecated_call_diagnostics(src, &doc, &[], &DiagnosticsConfig::default());
        assert_eq!(
            diags.len(),
            1,
            "expected 1 deprecated warning for enum method, got: {:?}",
            diags
        );
        assert!(
            diags[0].message.contains("label"),
            "message should mention 'label'"
        );
    }

    #[test]
    fn unbraced_namespace_classes_with_same_name_not_flagged() {
        // Two classes named `Foo` in different unbraced namespaces — should not be a duplicate.
        let src = "<?php\nnamespace App\\A;\nclass Foo {}\nnamespace App\\B;\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert!(
            diags.is_empty(),
            "classes with same name in different unbraced namespaces should not be flagged, got: {:?}",
            diags
        );
    }

    #[test]
    fn unbraced_namespace_duplicate_in_same_namespace_is_flagged() {
        // Two classes named `Foo` in the same unbraced namespace — should produce one warning.
        let src = "<?php\nnamespace App;\nclass Foo {}\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert_eq!(
            diags.len(),
            1,
            "expected 1 duplicate-declaration diagnostic, got: {:?}",
            diags
        );
        assert!(diags[0].message.contains("Foo"));
    }

    #[test]
    fn duplicate_declaration_range_spans_full_name() {
        // Duplicate declaration diagnostic range should span the entire name, not just first character.
        let src = "<?php\nclass Foo {}\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert_eq!(diags.len(), 1, "expected exactly 1 duplicate diagnostic");

        let d = &diags[0];
        let range_len = d.range.end.character - d.range.start.character;
        let expected_len = "Foo".chars().map(|c| c.len_utf16() as u32).sum::<u32>();
        assert_eq!(
            range_len, expected_len,
            "range length {} should match 'Foo' length {}",
            range_len, expected_len
        );

        // Verify the range actually points to "Foo", not "class"
        // "Foo" appears at character position 6 on line 2: "class Foo {}"
        //                                          012345678...
        assert_eq!(
            d.range.start.character, 6,
            "range should start at 'F' in 'Foo'"
        );
        assert_eq!(
            d.range.end.character, 9,
            "range should end after 'o' in 'Foo'"
        );
    }

    #[test]
    fn duplicate_function_declaration_range_spans_name() {
        // Function duplicate should also span the full function name.
        let src = "<?php\nfunction doWork() {}\nfunction doWork() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert_eq!(diags.len(), 1, "expected exactly 1 duplicate diagnostic");

        let d = &diags[0];
        let range_len = d.range.end.character - d.range.start.character;
        let expected_len = "doWork".chars().map(|c| c.len_utf16() as u32).sum::<u32>();
        assert_eq!(
            range_len, expected_len,
            "range length {} should match 'doWork' length {}",
            range_len, expected_len
        );

        // Verify the range points to "doWork", not "function"
        // "doWork" appears at character position 9 on line 2: "function doWork() {}"
        //                                              0123456789...
        assert_eq!(
            d.range.start.character, 9,
            "range should start at 'd' in 'doWork'"
        );
        assert_eq!(
            d.range.end.character, 15,
            "range should end after 'k' in 'doWork'"
        );
    }

    #[test]
    fn duplicate_interface_range_spans_name() {
        // Interface duplicate should span the full interface name.
        let src = "<?php\ninterface Logger {}\ninterface Logger {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert_eq!(diags.len(), 1, "expected exactly 1 duplicate diagnostic");

        let d = &diags[0];
        let range_len = d.range.end.character - d.range.start.character;
        let expected_len = "Logger".chars().map(|c| c.len_utf16() as u32).sum::<u32>();
        assert_eq!(
            range_len, expected_len,
            "range length {} should match 'Logger' length {}",
            range_len, expected_len
        );

        // Verify the range points to "Logger", not "interface"
        // "Logger" appears at character position 10 on line 2: "interface Logger {}"
        //                                               01234567890...
        assert_eq!(
            d.range.start.character, 10,
            "range should start at 'L' in 'Logger'"
        );
        assert_eq!(
            d.range.end.character, 16,
            "range should end after 'r' in 'Logger'"
        );
    }

    #[test]
    fn duplicate_declaration_range_on_correct_line() {
        // Diagnostic range should be on the correct line.
        let src = "<?php\nclass Foo {}\n\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::default());
        assert_eq!(diags.len(), 1, "expected exactly 1 duplicate diagnostic");

        let d = &diags[0];
        // The second "class Foo" is on line 3 (0-indexed: line 3)
        assert_eq!(
            d.range.start.line, 3,
            "duplicate should be reported on line 3 (0-indexed)"
        );
        assert_eq!(
            d.range.end.line, 3,
            "range end should be on same line as start"
        );
    }

    #[test]
    fn to_lsp_diagnostic_sets_code_to_issue_kind_name() {
        use mir_issues::{Issue, IssueKind, Location};
        use std::sync::Arc;
        use tower_lsp::lsp_types::{NumberOrString, Url};

        let uri = Url::parse("file:///test.php").unwrap();
        let location = Location {
            file: Arc::from("file:///test.php"),
            line: 1,
            col_start: 0,
            col_end: 3,
        };
        let issue = Issue::new(
            IssueKind::UndefinedClass {
                name: "Foo".to_string(),
            },
            location,
        );
        let diag = to_lsp_diagnostic(issue, &uri);
        assert_eq!(
            diag.code,
            Some(NumberOrString::String("UndefinedClass".to_string())),
            "diagnostic code must be the IssueKind name so code actions can match by type"
        );
        assert!(
            diag.message.contains("Foo"),
            "diagnostic message should mention the class name"
        );
    }
}
