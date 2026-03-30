/// Semantic diagnostics bridge.
///
/// Delegates all analysis to the `mir-php` crate and converts its `Diagnostic`
/// type into the `tower-lsp` `Diagnostic` type expected by the LSP backend.
use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Position, Range, Url,
};

use crate::ast::{ParsedDoc, offset_to_position};
use crate::docblock::{docblock_before, parse_docblock};

/// Run semantic checks on `doc` against `other_docs` and return LSP diagnostics.
pub fn semantic_diagnostics(
    uri: &Url,
    doc: &ParsedDoc,
    other_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<Diagnostic> {
    let source = doc.source();
    let stmts: &[php_ast::Stmt<'_, '_>] = doc.program().stmts.as_ref();

    // Build the workspace context: (source, stmts) for each document.
    let mut all: Vec<(&str, &[php_ast::Stmt<'_, '_>])> = Vec::with_capacity(1 + other_docs.len());
    all.push((source, stmts));
    for (_, d) in other_docs {
        all.push((d.source(), d.program().stmts.as_ref()));
    }

    mir_php::analyze(source, stmts, &all)
        .into_iter()
        .map(|d| to_lsp_diagnostic(d, uri))
        .collect()
}

/// Check for deprecated function/method calls and emit Warning diagnostics.
pub fn deprecated_call_diagnostics(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
) -> Vec<Diagnostic> {
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
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if let Some(body) = &m.body {
                            collect_deprecated_calls(source, body, doc, other_docs, diags);
                        }
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if let Some(body) = &m.body {
                            collect_deprecated_calls(source, body, doc, other_docs, diags);
                        }
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind {
                        if let Some(body) = &m.body {
                            collect_deprecated_calls(source, body, doc, other_docs, diags);
                        }
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
            let func_name = name.as_ref();
            // Search all docs for this function's declaration
            let all_sources: Vec<(&str, &ParsedDoc)> = std::iter::once((source, doc))
                .chain(other_docs.iter().map(|d| (d.source(), d.as_ref())))
                .collect();
            for (src, d) in &all_sources {
                if let Some(span_start) = find_function_span(d, func_name) {
                    if let Some(raw) = docblock_before(src, span_start) {
                        let db = parse_docblock(&raw);
                        if db.is_deprecated() {
                            let start_pos = offset_to_position(source, call.name.span.start);
                            let end_pos = offset_to_position(source, call.name.span.end);
                            let msg = match &db.deprecated {
                                Some(m) if !m.is_empty() => {
                                    format!("Deprecated: {} — {}", func_name, m)
                                }
                                _ => format!("Deprecated: {}", func_name),
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
        }
        // Recurse into arguments so nested calls are also checked.
        for arg in call.args.iter() {
            check_expr_for_deprecated(source, &arg.value, doc, other_docs, diags);
        }
    }
    if let ExprKind::MethodCall(call) = &expr.kind {
        if let ExprKind::Identifier(name) = &call.method.kind {
            let method_name = name.as_ref();
            let all_sources: Vec<(&str, &ParsedDoc)> = std::iter::once((source, doc))
                .chain(other_docs.iter().map(|d| (d.source(), d.as_ref())))
                .collect();
            for (src, d) in &all_sources {
                if let Some(span_start) = find_method_span(d, method_name) {
                    if let Some(raw) = docblock_before(src, span_start) {
                        let db = parse_docblock(&raw);
                        if db.is_deprecated() {
                            let start_pos = offset_to_position(source, call.method.span.start);
                            let end_pos = offset_to_position(source, call.method.span.end);
                            let msg = match &db.deprecated {
                                Some(m) if !m.is_empty() => {
                                    format!("Deprecated: {} — {}", method_name, m)
                                }
                                _ => format!("Deprecated: {}", method_name),
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
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(s) = find_function_span_in_stmts(inner, func_name) {
                        return Some(s);
                    }
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
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == method_name {
                            return Some(member.span.start);
                        }
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == method_name {
                            return Some(member.span.start);
                        }
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind {
                        if m.name == method_name {
                            return Some(member.span.start);
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(s) = find_method_span_in_stmts(inner, method_name) {
                        return Some(s);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Check for duplicate class/function/interface/trait/enum declarations.
pub fn duplicate_declaration_diagnostics(source: &str, doc: &ParsedDoc) -> Vec<Diagnostic> {
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
    for stmt in stmts {
        let name_and_span: Option<(&str, u32)> = match &stmt.kind {
            StmtKind::Class(c) => c.name.map(|n| (n, stmt.span.start)),
            StmtKind::Interface(i) => Some((i.name, stmt.span.start)),
            StmtKind::Trait(t) => Some((t.name, stmt.span.start)),
            StmtKind::Enum(e) => Some((e.name, stmt.span.start)),
            StmtKind::Function(f) => Some((f.name, stmt.span.start)),
            StmtKind::Namespace(ns) => {
                if let php_ast::NamespaceBody::Braced(inner) = &ns.body {
                    let ns_name = ns
                        .name
                        .as_ref()
                        .map(|n| n.to_string_repr().to_string())
                        .unwrap_or_default();
                    let child_ns = if current_ns.is_empty() {
                        ns_name
                    } else {
                        format!("{}\\{}", current_ns, ns_name)
                    };
                    collect_duplicate_decls(source, inner, &child_ns, seen, diags);
                }
                None
            }
            _ => None,
        };
        if let Some((name, span_start)) = name_and_span {
            let key = if current_ns.is_empty() {
                name.to_string()
            } else {
                format!("{}\\{}", current_ns, name)
            };
            if seen.insert(key, ()).is_some() {
                let pos = crate::ast::offset_to_position(source, span_start);
                diags.push(Diagnostic {
                    range: Range {
                        start: pos,
                        end: pos,
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

fn to_lsp_diagnostic(d: mir_php::Diagnostic, uri: &Url) -> Diagnostic {
    let related_information = if d.related.is_empty() {
        None
    } else {
        Some(
            d.related
                .into_iter()
                .map(|(sl, sc, el, ec, msg)| DiagnosticRelatedInformation {
                    location: Location {
                        uri: uri.clone(),
                        range: Range {
                            start: Position {
                                line: sl,
                                character: sc,
                            },
                            end: Position {
                                line: el,
                                character: ec,
                            },
                        },
                    },
                    message: msg,
                })
                .collect(),
        )
    };
    Diagnostic {
        range: Range {
            start: Position {
                line: d.start_line,
                character: d.start_char,
            },
            end: Position {
                line: d.end_line,
                character: d.end_char,
            },
        },
        severity: Some(match d.severity {
            mir_php::Severity::Error => DiagnosticSeverity::ERROR,
            mir_php::Severity::Warning => DiagnosticSeverity::WARNING,
            mir_php::Severity::Information => DiagnosticSeverity::INFORMATION,
            mir_php::Severity::Hint => DiagnosticSeverity::HINT,
        }),
        source: Some("php-lsp".to_string()),
        message: d.message,
        related_information,
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
        let diags = deprecated_call_diagnostics(src, &doc, &[]);
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
        let diags = duplicate_declaration_diagnostics(src, &doc);
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
        let diags = duplicate_declaration_diagnostics(src, &doc);
        assert!(diags.is_empty());
    }

    #[test]
    fn namespace_scoped_duplicate_not_flagged() {
        // Two classes named `Foo` in different namespaces — should produce zero diagnostics.
        let src = "<?php\nnamespace App\\A {\nclass Foo {}\n}\nnamespace App\\B {\nclass Foo {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc);
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
        let diags = duplicate_declaration_diagnostics(src, &doc);
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
        let diags = duplicate_declaration_diagnostics(src, &doc);
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
        let diags = deprecated_call_diagnostics(src, &doc, &[]);
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
        let diags = deprecated_call_diagnostics(src, &doc, &[]);
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
        let diags = duplicate_declaration_diagnostics(src, &doc);
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
        let diags = deprecated_call_diagnostics(src, &doc, &[]);
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
        let diags = deprecated_call_diagnostics(src, &doc, &[]);
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
        let diags = deprecated_call_diagnostics(src, &doc, &[]);
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
}
