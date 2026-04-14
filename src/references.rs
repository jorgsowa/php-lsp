use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Span, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::ast::str_offset;
use crate::ast::{ParsedDoc, offset_to_position};
use crate::walk::{
    class_refs_in_stmts, function_refs_in_stmts, method_refs_in_stmts, refs_in_stmts,
    refs_in_stmts_with_use,
};

/// What kind of symbol the cursor is on.  Used to dispatch to the
/// appropriate semantic walker so that, e.g., searching for `get` as a
/// *method* doesn't return free-function calls named `get`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// A free (top-level) function.
    Function,
    /// An instance or static method (`->name`, `?->name`, `::name`).
    Method,
    /// A class, interface, trait, or enum name used as a type.
    Class,
}

/// Find all locations where `word` is referenced across the given documents.
/// If `include_declaration` is true, also includes the declaration site.
/// Pass `kind` to restrict results to a particular symbol category; `None`
/// falls back to the original word-based walker (better some results than none).
pub fn find_references(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
    kind: Option<SymbolKind>,
) -> Vec<Location> {
    find_references_inner(word, all_docs, include_declaration, false, kind)
}

/// Like `find_references` but also includes `use` statement spans.
/// Used by rename so that `use Foo;` statements are also updated.
/// Always uses the general walker (rename must update all occurrence kinds).
pub fn find_references_with_use(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
) -> Vec<Location> {
    find_references_inner(word, all_docs, include_declaration, true, None)
}

fn find_references_inner(
    word: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    include_declaration: bool,
    include_use: bool,
    kind: Option<SymbolKind>,
) -> Vec<Location> {
    let mut locations = Vec::new();

    for (uri, doc) in all_docs {
        let source = doc.source();
        let stmts = &doc.program().stmts;
        let mut spans = Vec::new();

        if include_use {
            // Rename path: general walker covers call sites, `use` imports, and declarations.
            refs_in_stmts_with_use(source, stmts, word, &mut spans);
            if !include_declaration {
                spans.retain(|span| !is_declaration_span(source, stmts, word, span));
            }
        } else {
            match kind {
                Some(SymbolKind::Function) => function_refs_in_stmts(stmts, word, &mut spans),
                Some(SymbolKind::Method) => method_refs_in_stmts(stmts, word, &mut spans),
                Some(SymbolKind::Class) => class_refs_in_stmts(stmts, word, &mut spans),
                // General walker already includes declarations; filter them out if unwanted.
                None => {
                    refs_in_stmts(source, stmts, word, &mut spans);
                    if !include_declaration {
                        spans.retain(|span| !is_declaration_span(source, stmts, word, span));
                    }
                }
            }
            // Typed walkers never emit declaration spans, so add them separately when wanted.
            // Pass `kind` so only declarations of the matching category are appended —
            // a Method search must not return a free-function declaration with the same name.
            if include_declaration && kind.is_some() {
                collect_declaration_spans(source, stmts, word, kind, &mut spans);
            }
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

/// Build a span covering exactly the declared name (not the keyword before it).
fn declaration_name_span(source: &str, name: &str) -> Span {
    let start = str_offset(source, name);
    Span {
        start,
        end: start + name.len() as u32,
    }
}

/// Collect every span where `word` is *declared* within `stmts`.
///
/// When `kind` is `Some`, only declarations of the matching category are collected:
/// - `Function` → free (`StmtKind::Function`) declarations only
/// - `Method`   → method declarations inside classes / traits / enums only
/// - `Class`    → class / interface / trait / enum type declarations only
///
/// `None` collects every declaration kind (used by `is_declaration_span`).
fn collect_declaration_spans(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    word: &str,
    kind: Option<SymbolKind>,
    out: &mut Vec<Span>,
) {
    let want_free = matches!(kind, None | Some(SymbolKind::Function));
    let want_method = matches!(kind, None | Some(SymbolKind::Method));
    let want_type = matches!(kind, None | Some(SymbolKind::Class));

    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                if want_free && f.name == word {
                    out.push(declaration_name_span(source, f.name));
                }
            }
            StmtKind::Class(c) => {
                if want_type
                    && let Some(name) = c.name
                    && name == word
                {
                    out.push(declaration_name_span(source, name));
                }
                if want_method {
                    for member in c.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == word
                        {
                            out.push(declaration_name_span(source, m.name));
                        }
                    }
                }
            }
            StmtKind::Interface(i) => {
                if want_type && i.name == word {
                    out.push(declaration_name_span(source, i.name));
                }
            }
            StmtKind::Trait(t) => {
                if want_type && t.name == word {
                    out.push(declaration_name_span(source, t.name));
                }
                if want_method {
                    for member in t.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == word
                        {
                            out.push(declaration_name_span(source, m.name));
                        }
                    }
                }
            }
            StmtKind::Enum(e) => {
                if want_type && e.name == word {
                    out.push(declaration_name_span(source, e.name));
                }
                for member in e.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Method(m) if want_method && m.name == word => {
                            out.push(declaration_name_span(source, m.name));
                        }
                        EnumMemberKind::Case(c) if want_type && c.name == word => {
                            out.push(declaration_name_span(source, c.name));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_declaration_spans(source, inner, word, kind, out);
                }
            }
            _ => {}
        }
    }
}

/// Returns true if this span is the declaration site (function/class/method name).
/// Compares against the name's own span (not the whole statement span).
fn is_declaration_span(source: &str, stmts: &[Stmt<'_, '_>], word: &str, span: &Span) -> bool {
    let mut decl_spans = Vec::new();
    collect_declaration_spans(source, stmts, word, None, &mut decl_spans);
    decl_spans.iter().any(|s| spans_equal(s, span))
}

fn spans_equal(a: &Span, b: &Span) -> bool {
    a.start == b.start && a.end == b.end
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
        let refs = find_references("greet", &docs, false, None);
        assert_eq!(refs.len(), 2, "expected 2 call-site refs, got {:?}", refs);
    }

    #[test]
    fn include_declaration_adds_def_site() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let with_decl = find_references("greet", &docs, true, None);
        let without_decl = find_references("greet", &docs, false, None);
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
        let refs = find_references("Foo", &docs, false, None);
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
        let refs = find_references("greet", &docs, false, None);
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
        let refs = find_references("helper", &[a, b], false, None);
        assert_eq!(refs.len(), 2, "expected 2 cross-file references");
        assert!(refs.iter().all(|r| r.uri.path().ends_with("/b.php")));
    }

    #[test]
    fn finds_method_call_reference() {
        let src = "<?php\nclass Calc { public function add() {} }\n$c = new Calc();\n$c->add();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("add", &docs, false, None);
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
        let refs = find_references("check", &docs, false, None);
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
        let refs = find_references("helper", &docs, false, None);
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
        let refs = find_references("doWork", &docs, false, None);
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
        let refs = find_references("greet", &docs, false, None);
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
        let refs = find_references("Status", &docs, false, None);
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
        let refs = find_references("helper", &docs, false, None);
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
        let refs = find_references("tick", &docs, false, None);
        assert_eq!(
            refs.len(),
            2,
            "expected exactly 2 references to tick() (init + update), got: {:?}",
            refs
        );
        // Both are on line 2.
        assert!(refs.iter().all(|r| r.range.start.line == 2));
    }

    // ── Semantic (kind-aware) tests ───────────────────────────────────────────

    #[test]
    fn function_kind_skips_method_call_with_same_name() {
        // When looking for the free function `get`, method calls `$obj->get()` must be excluded.
        let src = "<?php\nfunction get() {}\nget();\n$obj->get();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("get", &docs, false, Some(SymbolKind::Function));
        // Only the free call `get()` on line 2 should appear; not the method call on line 3.
        assert_eq!(
            refs.len(),
            1,
            "expected 1 free-function ref, got: {:?}",
            refs
        );
        assert_eq!(refs[0].range.start.line, 2);
    }

    #[test]
    fn method_kind_skips_free_function_call_with_same_name() {
        // When looking for the method `add`, the free function call `add()` must be excluded.
        let src = "<?php\nfunction add() {}\nadd();\n$calc->add();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("add", &docs, false, Some(SymbolKind::Method));
        // Only the method call on line 3 should appear.
        assert_eq!(refs.len(), 1, "expected 1 method ref, got: {:?}", refs);
        assert_eq!(refs[0].range.start.line, 3);
    }

    #[test]
    fn class_kind_finds_new_expression() {
        // SymbolKind::Class should find `new Foo()` but not a free function call `Foo()`.
        let src = "<?php\nclass Foo {}\n$x = new Foo();\nFoo();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Foo", &docs, false, Some(SymbolKind::Class));
        // `new Foo()` on line 2 yes; `Foo()` on line 3 should NOT appear as a class ref.
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&2),
            "expected new Foo() on line 2, got: {:?}",
            refs
        );
        assert!(
            !lines.contains(&3),
            "free call Foo() should not appear as class ref, got: {:?}",
            refs
        );
    }

    #[test]
    fn class_kind_finds_extends_and_implements() {
        let src = "<?php\nclass Base {}\ninterface Iface {}\nclass Child extends Base implements Iface {}";
        let docs = vec![doc("/a.php", src)];

        let base_refs = find_references("Base", &docs, false, Some(SymbolKind::Class));
        let lines_base: Vec<u32> = base_refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines_base.contains(&3),
            "expected extends Base on line 3, got: {:?}",
            base_refs
        );

        let iface_refs = find_references("Iface", &docs, false, Some(SymbolKind::Class));
        let lines_iface: Vec<u32> = iface_refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines_iface.contains(&3),
            "expected implements Iface on line 3, got: {:?}",
            iface_refs
        );
    }

    #[test]
    fn class_kind_finds_type_hint() {
        // SymbolKind::Class should find `Foo` as a parameter type hint.
        let src = "<?php\nclass Foo {}\nfunction take(Foo $x): void {}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Foo", &docs, false, Some(SymbolKind::Class));
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&2),
            "expected type hint Foo on line 2, got: {:?}",
            refs
        );
    }

    // ── Declaration span precision tests ────────────────────────────────────────

    #[test]
    fn function_declaration_span_points_to_name_not_keyword() {
        // `include_declaration: true` — the declaration ref must start at `greet`,
        // not at the `function` keyword.
        let src = "<?php\nfunction greet() {}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, true, None);
        assert_eq!(refs.len(), 1, "expected exactly 1 ref (the declaration)");
        // "function " is 9 bytes; "greet" starts at byte 15 (after "<?php\n").
        // As a position, line 1, character 9.
        assert_eq!(
            refs[0].range.start.line, 1,
            "declaration should be on line 1"
        );
        assert_eq!(
            refs[0].range.start.character, 9,
            "declaration should start at the function name, not the 'function' keyword"
        );
        assert_eq!(
            refs[0].range.end.character,
            refs[0].range.start.character
                + "greet".chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
            "range should span exactly the function name"
        );
    }

    #[test]
    fn class_declaration_span_points_to_name_not_keyword() {
        let src = "<?php\nclass MyClass {}";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("MyClass", &docs, true, None);
        assert_eq!(refs.len(), 1);
        // "class " is 6 bytes; "MyClass" starts at character 6.
        assert_eq!(refs[0].range.start.line, 1);
        assert_eq!(
            refs[0].range.start.character, 6,
            "declaration should start at 'MyClass', not 'class'"
        );
    }

    #[test]
    fn method_declaration_span_points_to_name_not_keyword() {
        let src = "<?php\nclass C {\n    public function doThing() {}\n}\n(new C())->doThing();";
        let docs = vec![doc("/a.php", src)];
        // include_declaration=true so we get the method declaration too.
        let refs = find_references("doThing", &docs, true, None);
        // Declaration on line 2, call on line 4.
        let decl_ref = refs
            .iter()
            .find(|r| r.range.start.line == 2)
            .expect("no declaration ref on line 2");
        // "    public function " is 20 chars; "doThing" starts at character 20.
        assert_eq!(
            decl_ref.range.start.character, 20,
            "method declaration should start at the method name, not 'public function'"
        );
    }

    #[test]
    fn method_kind_with_include_declaration_does_not_return_free_function() {
        // Regression: kind precision must be preserved even when include_declaration=true.
        // A free function `get` and a method `get` coexist; searching with
        // SymbolKind::Method must NOT return either the free function call or its declaration.
        //
        // Line 0: <?php
        // Line 1: function get() {}          ← free function declaration
        // Line 2: get();                     ← free function call
        // Line 3: class C { public function get() {} }  ← method declaration
        // Line 4: $c->get();                 ← method call
        let src =
            "<?php\nfunction get() {}\nget();\nclass C { public function get() {} }\n$c->get();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("get", &docs, true, Some(SymbolKind::Method));
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&3),
            "method declaration (line 3) must be present, got: {:?}",
            lines
        );
        assert!(
            lines.contains(&4),
            "method call (line 4) must be present, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&1),
            "free function declaration (line 1) must not appear when kind=Method, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&2),
            "free function call (line 2) must not appear when kind=Method, got: {:?}",
            lines
        );
    }

    #[test]
    fn function_kind_with_include_declaration_does_not_return_method_call() {
        // Symmetric: SymbolKind::Function + include_declaration=true must not return method
        // calls or method declarations with the same name.
        //
        // Line 0: <?php
        // Line 1: function add() {}          ← free function declaration
        // Line 2: add();                     ← free function call
        // Line 3: class C { public function add() {} }  ← method declaration
        // Line 4: $c->add();                 ← method call
        let src =
            "<?php\nfunction add() {}\nadd();\nclass C { public function add() {} }\n$c->add();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("add", &docs, true, Some(SymbolKind::Function));
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            lines.contains(&1),
            "function declaration (line 1) must be present, got: {:?}",
            lines
        );
        assert!(
            lines.contains(&2),
            "function call (line 2) must be present, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&3),
            "method declaration (line 3) must not appear when kind=Function, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&4),
            "method call (line 4) must not appear when kind=Function, got: {:?}",
            lines
        );
    }

    #[test]
    fn declaration_filter_finds_method_inside_same_named_class() {
        // Edge case: a class named `get` contains a method also named `get`.
        // collect_declaration_spans(kind=None) must find BOTH the class declaration
        // and the method declaration so is_declaration_span correctly filters both
        // when include_declaration=false.
        //
        // Line 0: <?php
        // Line 1: class get { public function get() {} }
        // Line 2: $obj->get();
        let src = "<?php\nclass get { public function get() {} }\n$obj->get();";
        let docs = vec![doc("/a.php", src)];

        // With include_declaration=false, neither the class name nor the method
        // declaration should appear — only the call site on line 2.
        let refs = find_references("get", &docs, false, None);
        let lines: Vec<u32> = refs.iter().map(|r| r.range.start.line).collect();
        assert!(
            !lines.contains(&1),
            "declaration line (1) must not appear when include_declaration=false, got: {:?}",
            lines
        );
        assert!(
            lines.contains(&2),
            "call site (line 2) must be present, got: {:?}",
            lines
        );

        // With include_declaration=true, the class declaration AND method declaration
        // are both on line 1; the call site is on line 2.
        let refs_with = find_references("get", &docs, true, None);
        assert_eq!(
            refs_with.len(),
            3,
            "expected 3 refs (class decl + method decl + call), got: {:?}",
            refs_with
        );
    }
}
