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
                character: start.character
                    + word.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
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
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == word
                            && spans_equal(&member.span, span)
                        {
                            return true;
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
                    if let NamespaceBody::Braced(inner) = &ns.body
                        && check(inner, word, span)
                    {
                        return true;
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
        // Without declaration: only the call site (line 2)
        assert_eq!(
            without_decl.len(),
            1,
            "expected 1 call-site ref without declaration"
        );
        assert_eq!(
            without_decl[0].range.start.line, 2,
            "call site should be on line 2"
        );
        // With declaration: 2 refs total (decl on line 1, call on line 2)
        assert_eq!(
            with_decl.len(),
            2,
            "expected 2 refs with declaration included"
        );
    }

    #[test]
    fn finds_new_expression_reference() {
        let src = "<?php\nclass Foo {}\n$x = new Foo();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Foo", &docs, false);
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 reference to Foo in new expr"
        );
        assert_eq!(
            refs[0].range.start.line, 2,
            "new Foo() reference should be on line 2"
        );
    }

    #[test]
    fn finds_reference_in_nested_function_call() {
        let src = "<?php\nfunction greet() {}\necho(greet());";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, false);
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 nested function call reference"
        );
        assert_eq!(
            refs[0].range.start.line, 2,
            "nested greet() call should be on line 2"
        );
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
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 method call reference to 'add'"
        );
        assert_eq!(
            refs[0].range.start.line, 3,
            "add() call should be on line 3"
        );
    }

    #[test]
    fn finds_reference_inside_if_body() {
        let src = "<?php\nfunction check() {}\nif (true) { check(); }";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("check", &docs, false);
        assert_eq!(refs.len(), 1, "expected exactly 1 reference inside if body");
        assert_eq!(
            refs[0].range.start.line, 2,
            "check() inside if should be on line 2"
        );
    }

    #[test]
    fn finds_use_statement_reference() {
        // Renaming MyClass — the `use MyClass;` statement should be in the results
        // when using find_references_with_use.
        let src = "<?php\nuse MyClass;\n$x = new MyClass();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references_with_use("MyClass", &docs, false);
        // Exactly 2 references: the `use MyClass;` on line 1 and `new MyClass()` on line 2.
        assert_eq!(
            refs.len(),
            2,
            "expected exactly 2 references, got: {:?}",
            refs
        );
        let mut lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            vec![1, 2],
            "references should be on lines 1 (use) and 2 (new)"
        );
    }

    #[test]
    fn find_references_returns_correct_lines() {
        // `helper` is called on lines 1 and 2 (0-based); check exact line numbers.
        let src = "<?php\nhelper();\nhelper();\nfunction helper() {}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("helper", &docs, false);
        assert_eq!(refs.len(), 2, "expected exactly 2 call-site references");
        let mut lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        lines.sort_unstable();
        assert_eq!(lines, vec![1, 2], "references should be on lines 1 and 2");
    }

    #[test]
    fn declaration_excluded_when_flag_false() {
        // When include_declaration=false the declaration line must not appear.
        let src = "<?php\nfunction doWork() {}\ndoWork();\ndoWork();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("doWork", &docs, false);
        // Declaration is on line 1; call sites are on lines 2 and 3.
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            !lines.contains(&1),
            "declaration line (1) must not appear when include_declaration=false, got: {:?}",
            lines
        );
        assert_eq!(refs.len(), 2, "expected 2 call-site references only");
    }

    #[test]
    fn partial_match_not_included() {
        // Searching for references to `greet` should NOT include occurrences of `greeting`.
        let src = "<?php\nfunction greet() {}\nfunction greeting() {}\ngreet();\ngreeting();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, false);
        // Only `greet()` call site should be included, not `greeting()`.
        for r in &refs {
            // Each reference range should span exactly the length of "greet" (5 chars),
            // not longer (which would indicate "greeting" was matched).
            let span_len = r.range.end.character - r.range.start.character;
            assert_eq!(
                span_len, 5,
                "reference span length should equal len('greet')=5, got {} at {:?}",
                span_len, r
            );
        }
        // There should be exactly 1 call-site reference (the greet() call, not greeting()).
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 reference to 'greet' (not 'greeting'), got: {:?}",
            refs
        );
    }

    #[test]
    fn finds_reference_in_class_property_default() {
        // A class constant used as a property default value should be found by find_references.
        let src = "<?php\nclass Foo {\n    public string $status = Status::ACTIVE;\n}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Status", &docs, false);
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 reference to Status in property default, got: {:?}",
            refs
        );
        assert_eq!(refs[0].range.start.line, 2, "reference should be on line 2");
    }

    #[test]
    fn finds_reference_inside_enum_method_body() {
        // A function call inside an enum method body should be found by find_references.
        let src = "<?php\nfunction helper() {}\nenum Status {\n    public function label(): string { return helper(); }\n}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("helper", &docs, false);
        assert_eq!(
            refs.len(),
            1,
            "expected exactly 1 reference to helper() inside enum method, got: {:?}",
            refs
        );
        assert_eq!(refs[0].range.start.line, 3, "reference should be on line 3");
    }

    #[test]
    fn finds_reference_in_for_init_and_update() {
        // Function calls in `for` init and update expressions should be found.
        let src = "<?php\nfunction tick() {}\nfor (tick(); $i < 10; tick()) {}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("tick", &docs, false);
        assert_eq!(
            refs.len(),
            2,
            "expected exactly 2 references to tick() (init + update), got: {:?}",
            refs
        );
        // Both are on line 2.
        assert!(refs.iter().all(|r| r.range.start.line == 2));
    }
}
