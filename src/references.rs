use std::sync::Arc;

use php_ast::{ClassMemberKind, NamespaceBody, Span, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::ast::{ParsedDoc, offset_to_position};
use crate::walk::{refs_in_stmts, refs_in_stmts_with_use};

/// Find all locations where `word` is referenced across the given documents.
/// If `include_declaration` is true, also includes the declaration site.
pub fn find_references(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
) -> Vec<Location> {
    find_references_inner(word, all_docs, include_declaration, false)
}

/// Like `find_references` but also includes `use` statement spans.
/// Used by rename so that `use Foo;` statements are also updated.
pub fn find_references_with_use(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
) -> Vec<Location> {
    find_references_inner(word, all_docs, include_declaration, true)
}

fn find_references_inner(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
    include_use: bool,
) -> Vec<Location> {
    let mut locations = Vec::new();

    for (uri, doc) in all_docs {
        let source = doc.source();
        let stmts = &doc.program().stmts;
        let mut spans = Vec::new();
        if include_use {
            refs_in_stmts_with_use(stmts, word, &mut spans);
        } else {
            refs_in_stmts(stmts, word, &mut spans);
        }

        if !include_declaration {
            spans.retain(|span| !is_declaration_span(stmts, word, span));
        }

        for span in spans {
            let start = offset_to_position(source, span.start);
            let end = Position {
                line: start.line,
                character: start.character + word.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
            };
            locations.push(Location {
                uri: uri.clone(),
                range: Range { start, end },
            });
        }
    }

    locations
}

/// Returns true if this span is the declaration site (function/class/method name).
fn is_declaration_span(stmts: &[Stmt<'_, '_>], word: &str, span: &Span) -> bool {
    fn check(stmts: &[Stmt<'_, '_>], word: &str, span: &Span) -> bool {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Function(f) if f.name == word => {
                    if spans_equal(&stmt.span, span) {
                        return true;
                    }
                }
                StmtKind::Class(c) if c.name == Some(word) => {
                    if spans_equal(&stmt.span, span) {
                        return true;
                    }
                }
                StmtKind::Class(c) => {
                    for member in c.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            if m.name == word && spans_equal(&member.span, span) {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Interface(i) if i.name == word => {
                    if spans_equal(&stmt.span, span) {
                        return true;
                    }
                }
                StmtKind::Trait(t) if t.name == word => {
                    if spans_equal(&stmt.span, span) {
                        return true;
                    }
                }
                StmtKind::Namespace(ns) => {
                    if let NamespaceBody::Braced(inner) = &ns.body {
                        if check(inner, word, span) {
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    check(stmts, word, span)
}

fn spans_equal(a: &Span, b: &Span) -> bool {
    a.start == b.start
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(path: &str, source: &str) -> (Url, Arc<ParsedDoc>) {
        (uri(path), Arc::new(ParsedDoc::parse(source.to_string())))
    }

    #[test]
    fn finds_function_call_reference() {
        let src = "<?php\nfunction greet() {}\ngreet();\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, false);
        assert_eq!(refs.len(), 2, "expected 2 call-site refs, got {:?}", refs);
    }

    #[test]
    fn include_declaration_adds_def_site() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let with_decl = find_references("greet", &docs, true);
        let without_decl = find_references("greet", &docs, false);
        assert!(
            with_decl.len() > without_decl.len(),
            "declaration should be included"
        );
    }

    #[test]
    fn finds_new_expression_reference() {
        let src = "<?php\nclass Foo {}\n$x = new Foo();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Foo", &docs, false);
        assert!(!refs.is_empty(), "expected reference to Foo in new expr");
    }

    #[test]
    fn finds_reference_in_nested_function_call() {
        let src = "<?php\nfunction greet() {}\necho(greet());";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, false);
        assert!(!refs.is_empty(), "expected nested function call reference");
    }

    #[test]
    fn finds_references_across_multiple_docs() {
        let a = doc("/a.php", "<?php\nfunction helper() {}");
        let b = doc("/b.php", "<?php\nhelper();\nhelper();");
        let refs = find_references("helper", &[a, b], false);
        assert_eq!(refs.len(), 2, "expected 2 cross-file references");
        assert!(refs.iter().all(|r| r.uri.path().ends_with("/b.php")));
    }

    #[test]
    fn finds_method_call_reference() {
        let src = "<?php\nclass Calc { public function add() {} }\n$c = new Calc();\n$c->add();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("add", &docs, false);
        assert!(!refs.is_empty(), "expected method call reference to 'add'");
    }

    #[test]
    fn finds_reference_inside_if_body() {
        let src = "<?php\nfunction check() {}\nif (true) { check(); }";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("check", &docs, false);
        assert!(!refs.is_empty(), "expected reference inside if body");
    }
}
