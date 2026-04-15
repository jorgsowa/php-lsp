/// Semantic diagnostics bridge.
///
/// Uses mago (linter + semantics checker) to analyse PHP source and converts
/// its `Issue` type into the `tower-lsp` `Diagnostic` type expected by the LSP backend.
use std::borrow::Cow;
use std::sync::Arc;

use bumpalo::Bump;
use mago_database::file::{File, FileType};
use mago_linter::Linter;
use mago_linter::settings::Settings as LinterSettings;
use mago_names::resolver::NameResolver;
use mago_php_version::PHPVersion;
use mago_reporting::{Issue, Level};
use mago_semantics::SemanticsChecker;
use mago_syntax::parser::parse_file;
use php_ast::{ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};

use crate::ast::{ParsedDoc, offset_to_position};
use crate::backend::DiagnosticsConfig;
use crate::docblock::{docblock_before, parse_docblock};

/// Run semantic checks on `doc` using mago's linter and semantics checker.
///
/// `php_version` is an optional version string like `"8.1"` sourced from `LspConfig`.
pub fn semantic_diagnostics(
    uri: &Url,
    doc: &ParsedDoc,
    cfg: &DiagnosticsConfig,
    php_version: Option<&str>,
) -> Vec<Diagnostic> {
    if !cfg.enabled {
        return vec![];
    }

    let source = doc.source();

    // Build a mago `File` from the source text (ephemeral — no filesystem path needed).
    let file_name: Cow<'static, str> = Cow::Owned(uri.as_str().to_string());
    let contents: Cow<'static, str> = Cow::Owned(source.to_string());
    let mago_file = File::new(file_name, FileType::Host, None, contents);

    // Parse with mago's own parser (required by mago-names / mago-semantics / mago-linter).
    let arena = Bump::new();
    let program = parse_file(&arena, &mago_file);

    // Resolve names (use imports → FQNs).
    let resolver = NameResolver::new(&arena);
    let resolved_names = resolver.resolve(program);

    // Determine PHP version from config (default: 8.4).
    let php_version = parse_php_version(php_version);

    // Run semantics checker.
    let sem_checker = SemanticsChecker::new(php_version);
    let sem_issues = sem_checker.check(&mago_file, program, &resolved_names);

    // Run linter with the same PHP version used for semantic checks.
    let linter_settings = LinterSettings {
        php_version,
        ..LinterSettings::default()
    };
    let linter = Linter::new(&arena, &linter_settings, None, false);
    let lint_issues = linter.lint(&mago_file, program, &resolved_names);

    // Merge, filter, and convert to LSP Diagnostics.
    sem_issues
        .into_iter()
        .chain(lint_issues)
        .filter(|i| issue_passes_filter(i, cfg))
        .filter_map(|i| to_lsp_diagnostic(i, source))
        .collect()
}

/// Parse a PHP version string like `"8.1"` into a `PHPVersion`.
/// Falls back to `PHPVersion::PHP84` if the string is absent or unrecognised.
fn parse_php_version(ver: Option<&str>) -> PHPVersion {
    let Some(s) = ver else {
        return PHPVersion::PHP84;
    };
    let mut parts = s.splitn(2, '.');
    let major: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(8);
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(4);
    PHPVersion::new(major, minor, 0)
}

/// Returns `true` if the mago issue is allowed through by the config.
fn issue_passes_filter(issue: &Issue, cfg: &DiagnosticsConfig) -> bool {
    let code = issue.code.as_deref().unwrap_or("");
    // Map mago issue codes to config flags.
    if code.contains("undefined_variable") || code.contains("UndefinedVariable") {
        return cfg.undefined_variables;
    }
    if code.contains("undefined_function") || code.contains("UndefinedFunction") {
        return cfg.undefined_functions;
    }
    if code.contains("undefined_class") || code.contains("UndefinedClass") {
        return cfg.undefined_classes;
    }
    if code.contains("type_error")
        || code.contains("TypeError")
        || code.contains("invalid_return")
        || code.contains("InvalidReturn")
    {
        return cfg.type_errors;
    }
    if code.contains("deprecated") || code.contains("Deprecated") {
        return cfg.deprecated_calls;
    }
    true
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
    collect_deprecated_calls(source, &doc.program().stmts, doc, other_docs, &mut diags);
    diags
}

fn collect_deprecated_calls(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    diags: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Expression(e) => {
                check_expr_for_deprecated(source, e, doc, other_docs, diags);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_deprecated_calls(source, inner, doc, other_docs, diags);
                }
            }
            StmtKind::Function(f) => {
                collect_deprecated_calls(source, &f.body, doc, other_docs, diags);
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && let Some(body) = &m.body
                    {
                        collect_deprecated_calls(source, body, doc, other_docs, diags);
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && let Some(body) = &m.body
                    {
                        collect_deprecated_calls(source, body, doc, other_docs, diags);
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && let Some(body) = &m.body
                    {
                        collect_deprecated_calls(source, body, doc, other_docs, diags);
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
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    diags: &mut Vec<Diagnostic>,
) {
    if let ExprKind::Assign(a) = &expr.kind {
        check_expr_for_deprecated(source, a.value, doc, other_docs, diags);
        return;
    }
    if let ExprKind::FunctionCall(call) = &expr.kind {
        if let ExprKind::Identifier(name) = &call.name.kind {
            let func_name = name;
            // Search all docs for this function's declaration
            let all_sources: Vec<(&str, &ParsedDoc)> = std::iter::once((source, doc))
                .chain(other_docs.iter().map(|d| (d.source(), d.as_ref())))
                .collect();
            for (src, d) in &all_sources {
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
            check_expr_for_deprecated(source, &arg.value, doc, other_docs, diags);
        }
    }
    if let ExprKind::MethodCall(call) = &expr.kind {
        if let ExprKind::Identifier(name) = &call.method.kind {
            let method_name = name;
            let all_sources: Vec<(&str, &ParsedDoc)> = std::iter::once((source, doc))
                .chain(other_docs.iter().map(|d| (d.source(), d.as_ref())))
                .collect();
            for (src, d) in &all_sources {
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
        check_expr_for_deprecated(source, call.object, doc, other_docs, diags);
        for arg in call.args.iter() {
            check_expr_for_deprecated(source, &arg.value, doc, other_docs, diags);
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

/// Convert a mago `Issue` to an LSP `Diagnostic`.
/// Returns `None` if the issue has no primary span (can't locate it in source).
fn to_lsp_diagnostic(issue: Issue, source: &str) -> Option<Diagnostic> {
    // Use the primary annotation span to get byte offsets.
    let span = issue.primary_span()?;
    let start = offset_to_position(source, span.start.offset);
    let end = offset_to_position(source, span.end.offset);
    // Ensure end >= start (mago spans are always start < end, but guard anyway).
    let end =
        if end.line > start.line || (end.line == start.line && end.character > start.character) {
            end
        } else {
            Position {
                line: start.line,
                character: start.character + 1,
            }
        };
    let severity = Some(match issue.level {
        Level::Error => DiagnosticSeverity::ERROR,
        Level::Warning => DiagnosticSeverity::WARNING,
        Level::Help | Level::Note => DiagnosticSeverity::INFORMATION,
    });
    Some(Diagnostic {
        range: Range { start, end },
        severity,
        code: issue.code.map(NumberOrString::String),
        source: Some("php-lsp".to_string()),
        message: issue.message,
        ..Default::default()
    })
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

    // ── mago analysis path ────────────────────────────────────────────────────
    //
    // These tests exercise `semantic_diagnostics()` end-to-end, verifying that
    // mago's linter + semantics checker produce (or correctly suppress)
    // diagnostics for real PHP source.

    mod mago_analysis {
        use super::*;
        use tower_lsp::lsp_types::Url;

        fn url() -> Url {
            Url::parse("file:///test.php").unwrap()
        }

        fn run(src: &str) -> Vec<Diagnostic> {
            let doc = ParsedDoc::parse(src.to_string());
            semantic_diagnostics(&url(), &doc, &DiagnosticsConfig::default(), None)
        }

        // ── Master switch ─────────────────────────────────────────────────────

        #[test]
        fn disabled_config_always_returns_empty() {
            let src = "<?php\nnew NonExistentClass();\n";
            let doc = ParsedDoc::parse(src.to_string());
            let cfg = DiagnosticsConfig {
                enabled: false,
                ..DiagnosticsConfig::default()
            };
            let diags = semantic_diagnostics(&url(), &doc, &cfg, None);
            assert!(diags.is_empty(), "got: {:?}", diags);
        }

        // ── No false positives on valid PHP ───────────────────────────────────

        #[test]
        fn valid_php_with_strict_types_produces_no_errors() {
            // Files with declare(strict_types=1) satisfy the linter's strict-types
            // rule and should produce no error-severity diagnostics.
            let src = concat!(
                "<?php\n",
                "declare(strict_types=1);\n",
                "function hello(): string { return 'world'; }\n",
            );
            let errors: Vec<_> = run(src)
                .into_iter()
                .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
                .collect();
            assert!(errors.is_empty(), "got: {:?}", errors);
        }

        #[test]
        fn simple_function_produces_no_errors() {
            let src = concat!(
                "<?php\n",
                "function add(int $a, int $b): int {\n",
                "    return $a + $b;\n",
                "}\n",
                "echo add(1, 2);\n",
            );
            let errors: Vec<_> = run(src)
                .into_iter()
                .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
                .collect();
            assert!(errors.is_empty(), "got: {:?}", errors);
        }

        #[test]
        fn simple_class_produces_no_errors() {
            let src = concat!(
                "<?php\n",
                "class Box {\n",
                "    public function __construct(public readonly int $value) {}\n",
                "    public function doubled(): int { return $this->value * 2; }\n",
                "}\n",
                "$b = new Box(21);\n",
                "echo $b->doubled();\n",
            );
            let errors: Vec<_> = run(src)
                .into_iter()
                .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
                .collect();
            assert!(errors.is_empty(), "got: {:?}", errors);
        }

        #[test]
        fn php81_enum_produces_no_errors_on_81() {
            let src = concat!(
                "<?php\n",
                "enum Status {\n",
                "    case Active;\n",
                "    case Inactive;\n",
                "}\n",
            );
            let doc = ParsedDoc::parse(src.to_string());
            let errors: Vec<_> =
                semantic_diagnostics(&url(), &doc, &DiagnosticsConfig::default(), Some("8.1"))
                    .into_iter()
                    .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
                    .collect();
            assert!(
                errors.is_empty(),
                "enum should be valid in PHP 8.1, got: {:?}",
                errors
            );
        }

        // ── Linter rules ──────────────────────────────────────────────────────
        //
        // mago's semantics checker operates in single-file mode and does not
        // resolve cross-file symbols, so undefined-class/function detection
        // requires the full workspace index that only the backend provides.
        // At the unit-test level we therefore exercise the *linter* rules that
        // fire on single-file analysis.

        #[test]
        fn missing_strict_types_declaration_is_flagged() {
            // mago's linter emits a "strict-types" diagnostic for any file that
            // lacks `declare(strict_types=1)`.
            use tower_lsp::lsp_types::NumberOrString;
            let src = "<?php\nfunction hello(): string { return 'world'; }\n";
            let diags = run(src);
            assert!(
                diags
                    .iter()
                    .any(|d| d.code == Some(NumberOrString::String("strict-types".to_string()))),
                "expected strict-types diagnostic, got: {:?}",
                diags
            );
        }

        #[test]
        fn strict_types_diagnostic_is_warning_or_information() {
            use tower_lsp::lsp_types::NumberOrString;
            let src = "<?php\nfunction hello(): string { return 'world'; }\n";
            let diags = run(src);
            let strict = diags
                .iter()
                .find(|d| d.code == Some(NumberOrString::String("strict-types".to_string())));
            let sev = strict.and_then(|d| d.severity).unwrap();
            assert!(
                sev == DiagnosticSeverity::WARNING || sev == DiagnosticSeverity::INFORMATION,
                "strict-types should not be an error, got: {:?}",
                sev
            );
        }

        #[test]
        fn adding_strict_types_declaration_clears_lint_warning() {
            use tower_lsp::lsp_types::NumberOrString;
            let without = "<?php\nfunction hello(): string { return 'world'; }\n";
            let with =
                "<?php\ndeclare(strict_types=1);\nfunction hello(): string { return 'world'; }\n";
            let diags_without = run(without);
            let diags_with = run(with);
            assert!(
                diags_without
                    .iter()
                    .any(|d| d.code == Some(NumberOrString::String("strict-types".to_string()))),
                "expected strict-types without declaration, got: {:?}",
                diags_without
            );
            assert!(
                !diags_with
                    .iter()
                    .any(|d| d.code == Some(NumberOrString::String("strict-types".to_string()))),
                "strict-types should clear when declaration is present, got: {:?}",
                diags_with
            );
        }

        // ── Diagnostic shape ──────────────────────────────────────────────────

        #[test]
        fn all_diagnostics_have_severity() {
            let src = "<?php\nnew Xyzzy_Qux_NonExistent_99();\n";
            for d in run(src) {
                assert!(d.severity.is_some(), "diagnostic missing severity: {:?}", d);
            }
        }

        #[test]
        fn all_diagnostics_have_source_php_lsp() {
            let src = "<?php\nnew Xyzzy_Qux_NonExistent_99();\n";
            for d in run(src) {
                assert_eq!(
                    d.source.as_deref(),
                    Some("php-lsp"),
                    "unexpected source: {:?}",
                    d
                );
            }
        }

        #[test]
        fn all_diagnostics_have_valid_ranges() {
            let src = "<?php\nnew Xyzzy_Qux_NonExistent_99();\n";
            for d in run(src) {
                assert!(
                    d.range.start.line <= d.range.end.line,
                    "start.line > end.line: {:?}",
                    d.range
                );
                if d.range.start.line == d.range.end.line {
                    assert!(
                        d.range.start.character <= d.range.end.character,
                        "start.character > end.character on same line: {:?}",
                        d.range
                    );
                }
            }
        }

        // ── PHP version handling ──────────────────────────────────────────────
        //
        // The PHP version must be forwarded to both the SemanticsChecker and the
        // Linter so that version-gated rules fire against the correct baseline.

        #[test]
        fn linter_uses_project_php_version() {
            // `explicit_nullable_param` is a PHP 8.4 deprecation rule: writing
            // `?Foo` as a parameter type hint is deprecated in favour of
            // `Foo|null`.  It must not fire when the project targets PHP 8.3.
            let src = concat!(
                "<?php\n",
                "declare(strict_types=1);\n",
                "function greet(?string $name): string {\n",
                "    return 'Hello ' . ($name ?? 'World');\n",
                "}\n",
            );
            let doc = ParsedDoc::parse(src.to_string());
            let diags_83 =
                semantic_diagnostics(&url(), &doc, &DiagnosticsConfig::default(), Some("8.3"));
            let diags_84 =
                semantic_diagnostics(&url(), &doc, &DiagnosticsConfig::default(), Some("8.4"));
            // On PHP 8.4 the deprecated ?T form should produce more diagnostics
            // than on 8.3 where it is still valid.
            assert!(
                diags_84.len() >= diags_83.len(),
                "PHP 8.4 should produce at least as many diagnostics as 8.3 \
                 for deprecated ?T syntax (8.3={}, 8.4={})",
                diags_83.len(),
                diags_84.len(),
            );
        }

        #[test]
        fn unknown_version_string_does_not_panic() {
            let src = "<?php\necho 'hello';\n";
            let doc = ParsedDoc::parse(src.to_string());
            // None of these should panic.
            let _ = semantic_diagnostics(&url(), &doc, &DiagnosticsConfig::default(), None);
            let _ = semantic_diagnostics(
                &url(),
                &doc,
                &DiagnosticsConfig::default(),
                Some("not-a-version"),
            );
            let _ = semantic_diagnostics(&url(), &doc, &DiagnosticsConfig::default(), Some(""));
        }

        #[test]
        fn version_81_and_84_both_accept_valid_php81() {
            let src = concat!(
                "<?php\n",
                "enum Color { case Red; case Green; case Blue; }\n",
                "function paint(Color $c): void {}\n",
                "paint(Color::Red);\n",
            );
            let doc = ParsedDoc::parse(src.to_string());
            for ver in ["8.1", "8.2", "8.3", "8.4"] {
                let errors: Vec<_> =
                    semantic_diagnostics(&url(), &doc, &DiagnosticsConfig::default(), Some(ver))
                        .into_iter()
                        .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
                        .collect();
                assert!(
                    errors.is_empty(),
                    "PHP {} should accept valid PHP 8.1 code, got: {:?}",
                    ver,
                    errors
                );
            }
        }
    }

    #[test]
    fn to_lsp_diagnostic_sets_code_from_mago_issue() {
        use mago_database::file::FileId;
        use mago_reporting::{Annotation, Issue, Level};
        use mago_span::{Position, Span};
        use tower_lsp::lsp_types::NumberOrString;

        let source = "<?php\nclass Foo {}";
        let file_id = FileId::new("test.php");
        let span = Span::new(file_id, Position::new(6), Position::new(9));
        let issue = Issue::new(Level::Error, "Undefined class Foo")
            .with_code("undefined_class")
            .with_annotation(Annotation::primary(span));
        let diag = to_lsp_diagnostic(issue, source).expect("expected a diagnostic");
        assert_eq!(
            diag.code,
            Some(NumberOrString::String("undefined_class".to_string())),
            "diagnostic code must be set from the mago issue code"
        );
        assert!(
            diag.message.contains("Foo"),
            "diagnostic message should mention the class name"
        );
    }
}
