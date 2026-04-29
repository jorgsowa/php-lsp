use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::ast::{ParsedDoc, SourceView, str_offset};
use crate::util::word_at;
use crate::walk::collect_var_refs_in_scope;

/// Find the definition of the symbol under `position`.
/// Searches the current document first, then `other_docs` for cross-file resolution.
pub fn goto_definition(
    uri: &Url,
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Location> {
    let word = word_at(source, position)?;

    // For $variable, find the first occurrence in scope (= the definition/assignment).
    let sv = doc.view();
    if word.starts_with('$') {
        let bare = word.trim_start_matches('$');
        let byte_off = sv.byte_of_position(position) as usize;
        let mut spans = Vec::new();
        collect_var_refs_in_scope(&doc.program().stmts, bare, byte_off, &mut spans);
        if let Some(span) = spans.into_iter().min_by_key(|s| s.start) {
            return Some(Location {
                uri: uri.clone(),
                range: Range {
                    start: sv.position_of(span.start),
                    end: sv.position_of(span.end),
                },
            });
        }
    }

    if let Some(range) = scan_statements(sv, &doc.program().stmts, &word) {
        return Some(Location {
            uri: uri.clone(),
            range,
        });
    }

    for (other_uri, other_doc) in other_docs {
        let other_sv = other_doc.view();
        if let Some(range) = scan_statements(other_sv, &other_doc.program().stmts, &word) {
            return Some(Location {
                uri: other_uri.clone(),
                range,
            });
        }
    }

    None
}

/// Search an AST for a declaration named `name`, returning its selection range.
/// Used by the PSR-4 fallback in the backend after resolving a class to a file.
pub fn find_declaration_range(_source: &str, doc: &ParsedDoc, name: &str) -> Option<Range> {
    let sv = doc.view();
    scan_statements(sv, &doc.program().stmts, name)
}

fn scan_statements(sv: SourceView<'_>, stmts: &[Stmt<'_, '_>], word: &str) -> Option<Range> {
    // Strip a leading `$` so that `$name` matches property names stored without `$`.
    let bare = word.strip_prefix('$').unwrap_or(word);
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                return Some(sv.name_range(f.name));
            }
            StmtKind::Class(c) if c.name == Some(word) => {
                let name = c.name.expect("match guard ensures Some");
                return Some(sv.name_range(name));
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(m.name));
                        }
                        ClassMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(sv.name_range(cc.name));
                        }
                        ClassMemberKind::Property(p) if p.name == bare => {
                            return Some(sv.name_range(p.name));
                        }
                        // Constructor-promoted parameters act as property declarations.
                        ClassMemberKind::Method(m) if m.name == "__construct" => {
                            for p in m.params.iter() {
                                if p.visibility.is_some() && p.name == bare {
                                    return Some(sv.name_range(p.name));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Interface(i) => {
                if i.name == word {
                    return Some(sv.name_range(i.name));
                }
                for member in i.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(m.name));
                        }
                        ClassMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(sv.name_range(cc.name));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Trait(t) => {
                if t.name == word {
                    return Some(sv.name_range(t.name));
                }
                for member in t.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(m.name));
                        }
                        ClassMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(sv.name_range(cc.name));
                        }
                        ClassMemberKind::Property(p) if p.name == bare => {
                            return Some(sv.name_range(p.name));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Enum(e) if e.name == word => {
                return Some(sv.name_range(e.name));
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(m.name));
                        }
                        EnumMemberKind::Case(c) if c.name == word => {
                            return Some(sv.name_range(c.name));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(range) = scan_statements(sv, inner, word)
                {
                    return Some(range);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find a class/function declaration by name in a slice of `FileIndex` entries.
/// Returns the URI and a line-level `Range`.
pub fn find_in_indexes(
    name: &str,
    indexes: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::file_index::FileIndex>,
    )],
) -> Option<Location> {
    let bare = name.strip_prefix('$').unwrap_or(name);
    for (uri, idx) in indexes {
        // Check top-level functions.
        for f in &idx.functions {
            if f.name == bare || f.name == name {
                let pos = tower_lsp::lsp_types::Position {
                    line: f.start_line,
                    character: 0,
                };
                return Some(Location {
                    uri: uri.clone(),
                    range: Range {
                        start: pos,
                        end: pos,
                    },
                });
            }
        }
        // Check classes / interfaces / traits / enums and their members.
        for cls in &idx.classes {
            if cls.name == bare || cls.name == name {
                let pos = tower_lsp::lsp_types::Position {
                    line: cls.start_line,
                    character: 0,
                };
                return Some(Location {
                    uri: uri.clone(),
                    range: Range {
                        start: pos,
                        end: pos,
                    },
                });
            }
            // Methods.
            for m in &cls.methods {
                if m.name == name {
                    let pos = tower_lsp::lsp_types::Position {
                        line: m.start_line,
                        character: 0,
                    };
                    return Some(Location {
                        uri: uri.clone(),
                        range: Range {
                            start: pos,
                            end: pos,
                        },
                    });
                }
            }
            // Properties (stored without `$`).
            for p in &cls.properties {
                if p.name == bare {
                    let pos = tower_lsp::lsp_types::Position {
                        line: p.start_line,
                        character: 0,
                    };
                    return Some(Location {
                        uri: uri.clone(),
                        range: Range {
                            start: pos,
                            end: pos,
                        },
                    });
                }
            }
            // Class constants.
            for cc in &cls.constants {
                if cc.as_str() == name {
                    let pos = tower_lsp::lsp_types::Position {
                        line: cls.start_line,
                        character: 0,
                    };
                    return Some(Location {
                        uri: uri.clone(),
                        range: Range {
                            start: pos,
                            end: pos,
                        },
                    });
                }
            }
            // Enum cases.
            for case in &cls.cases {
                if case.as_str() == name {
                    let pos = tower_lsp::lsp_types::Position {
                        line: cls.start_line,
                        character: 0,
                    };
                    return Some(Location {
                        uri: uri.clone(),
                        range: Range {
                            start: pos,
                            end: pos,
                        },
                    });
                }
            }
        }
    }
    None
}

/// Walk the class hierarchy (extends + traits) in the workspace index to find
/// `method_name` defined in `class_name` or any of its superclasses/traits.
///
/// Returns the first match in PHP's resolution order: class itself → traits →
/// parent → parent's traits, etc. Uses `indexes` so no disk I/O is needed.
pub fn find_method_in_class_hierarchy(
    class_name: &str,
    method_name: &str,
    indexes: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::file_index::FileIndex>,
    )],
) -> Option<Location> {
    let mut queue: Vec<String> = vec![class_name.to_owned()];
    let mut visited = std::collections::HashSet::new();

    while !queue.is_empty() {
        let current = queue.remove(0);
        if !visited.insert(current.clone()) {
            continue;
        }
        for (uri, idx) in indexes {
            for cls in &idx.classes {
                if cls.name != current && cls.fqn.trim_start_matches('\\') != current.as_str() {
                    continue;
                }
                for m in &cls.methods {
                    if m.name == method_name {
                        let pos = tower_lsp::lsp_types::Position {
                            line: m.start_line,
                            character: 0,
                        };
                        return Some(Location {
                            uri: uri.clone(),
                            range: Range {
                                start: pos,
                                end: pos,
                            },
                        });
                    }
                }
                // Traits first (PHP MRO), then parent.
                for trt in &cls.traits {
                    queue.push(trt.as_ref().to_owned());
                }
                if let Some(parent) = &cls.parent {
                    queue.push(parent.as_ref().to_owned());
                }
            }
        }
    }
    None
}

fn _name_range_from_offset(sv: SourceView<'_>, name: &str) -> Range {
    let start_offset = str_offset(sv.source(), name);
    let start = sv.position_of(start_offset);
    Range {
        start,
        end: Position {
            line: start.line,
            character: start.character + name.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::cursor;

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn jumps_to_function_definition() {
        let (src, p) = cursor("<?php\nfunction g$0reet() {}");
        let doc = ParsedDoc::parse(src.clone());
        let result = goto_definition(&uri(), &src, &doc, &[], p);
        assert!(result.is_some(), "expected a location");
        let loc = result.unwrap();
        assert_eq!(loc.range.start.line, 1);
        assert_eq!(loc.uri, uri());
    }

    #[test]
    fn jumps_to_class_definition() {
        let (src, p) = cursor("<?php\nclass My$0Service {}");
        let doc = ParsedDoc::parse(src.clone());
        let result = goto_definition(&uri(), &src, &doc, &[], p);
        assert!(result.is_some());
        let loc = result.unwrap();
        assert_eq!(loc.range.start.line, 1);
    }

    #[test]
    fn jumps_to_interface_definition() {
        let (src, p) = cursor("<?php\ninterface Co$0untable {}");
        let doc = ParsedDoc::parse(src.clone());
        let result = goto_definition(&uri(), &src, &doc, &[], p);
        assert!(result.is_some());
        assert_eq!(result.unwrap().range.start.line, 1);
    }

    #[test]
    fn jumps_to_trait_definition() {
        let src = "<?php\ntrait Loggable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 8));
        assert!(result.is_some());
        assert_eq!(result.unwrap().range.start.line, 1);
    }

    #[test]
    fn jumps_to_class_method_definition() {
        let src = "<?php\nclass Calc { public function add() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 32));
        assert!(result.is_some(), "expected location for method 'add'");
    }

    #[test]
    fn returns_none_for_unknown_word() {
        let src = "<?php\necho 'hello';";
        let doc = ParsedDoc::parse(src.to_string());
        // `hello` is a string literal, not a symbol — no definition found.
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 6));
        assert!(result.is_none());
    }

    #[test]
    fn variable_goto_definition_jumps_to_first_occurrence() {
        let src = "<?php\nfunction foo() {\n    $x = 1;\n    return $x;\n}";
        let doc = ParsedDoc::parse(src.to_string());
        // Cursor on `$x` in `return $x;` (line 3)
        let result = goto_definition(&uri(), src, &doc, &[], pos(3, 12));
        assert!(result.is_some(), "expected location for $x");
        let loc = result.unwrap();
        // First occurrence is on line 2 (the assignment)
        assert_eq!(
            loc.range.start.line, 2,
            "should jump to first $x occurrence"
        );
    }

    #[test]
    fn jumps_to_enum_definition() {
        let src = "<?php\nenum Suit { case Hearts; }";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 7));
        assert!(result.is_some(), "expected location for enum 'Suit'");
        assert_eq!(result.unwrap().range.start.line, 1);
    }

    #[test]
    fn jumps_to_enum_case_definition() {
        let src = "<?php\nenum Suit { case Hearts; case Spades; }";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 22));
        assert!(result.is_some(), "expected location for enum case 'Hearts'");
    }

    #[test]
    fn jumps_to_enum_method_definition() {
        let src = "<?php\nenum Suit { public function label(): string { return ''; } }";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 30));
        assert!(
            result.is_some(),
            "expected location for enum method 'label'"
        );
    }

    #[test]
    fn jumps_to_symbol_inside_namespace() {
        let src = "<?php\nnamespace App {\nfunction boot() {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(2, 10));
        assert!(result.is_some());
        assert_eq!(result.unwrap().range.start.line, 2);
    }

    #[test]
    fn finds_class_definition_in_other_document() {
        let current_src = "<?php\n$s = new MyService();";
        let current_doc = ParsedDoc::parse(current_src.to_string());
        let other_src = "<?php\nclass MyService {}";
        let other_uri = Url::parse("file:///other.php").unwrap();
        let other_doc = Arc::new(ParsedDoc::parse(other_src.to_string()));

        let result = goto_definition(
            &uri(),
            current_src,
            &current_doc,
            &[(other_uri.clone(), other_doc)],
            pos(1, 13),
        );
        assert!(result.is_some(), "expected cross-file location");
        assert_eq!(result.unwrap().uri, other_uri);
    }

    #[test]
    fn finds_function_definition_in_other_document() {
        let current_src = "<?php\nhelperFn();";
        let current_doc = ParsedDoc::parse(current_src.to_string());
        let other_src = "<?php\nfunction helperFn() {}";
        let other_uri = Url::parse("file:///helpers.php").unwrap();
        let other_doc = Arc::new(ParsedDoc::parse(other_src.to_string()));

        let result = goto_definition(
            &uri(),
            current_src,
            &current_doc,
            &[(other_uri.clone(), other_doc)],
            pos(1, 3),
        );
        assert!(
            result.is_some(),
            "expected cross-file location for helperFn"
        );
        assert_eq!(result.unwrap().uri, other_uri);
    }

    #[test]
    fn current_file_takes_priority_over_other_docs() {
        let src = "<?php\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let other_src = "<?php\nclass Foo {}";
        let other_uri = Url::parse("file:///other.php").unwrap();
        let other_doc = Arc::new(ParsedDoc::parse(other_src.to_string()));

        let result = goto_definition(&uri(), src, &doc, &[(other_uri, other_doc)], pos(1, 8));
        assert_eq!(result.unwrap().uri, uri(), "should prefer current file");
    }

    #[test]
    fn goto_definition_class_constant() {
        // Cursor on `STATUS_OK` in the class constant declaration should jump to `const STATUS_OK`.
        // Source: line 0 = <?php, line 1 = class MyClass { const STATUS_OK = 1; }
        // The cursor is placed on `STATUS_OK` inside the const declaration.
        let src = "<?php\nclass MyClass { const STATUS_OK = 1; }";
        let doc = ParsedDoc::parse(src.to_string());
        // `STATUS_OK` starts at column 22 on line 1 (after "class MyClass { const ")
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 22));
        assert!(
            result.is_some(),
            "expected a location for class constant STATUS_OK"
        );
        let loc = result.unwrap();
        assert_eq!(
            loc.range.start.line, 1,
            "should jump to line 1 where the constant is declared"
        );
        assert_eq!(loc.uri, uri(), "should be in the same file");
    }

    #[test]
    fn goto_definition_property() {
        // Cursor on the property `$name` in its declaration should jump to that declaration.
        // Source: line 0 = <?php, line 1 = class Person { public string $name; }
        // Column breakdown of line 1: "class Person { public string $name; }"
        //   col 0-4: "class", 5: " ", 6-11: "Person", 12: " ", 13: "{", 14: " ",
        //   15-20: "public", 21: " ", 22-27: "string", 28: " ", 29: "$", 30-33: "name"
        let src = "<?php\nclass Person { public string $name; }";
        let doc = ParsedDoc::parse(src.to_string());
        // Cursor on column 30 — on the `n` in `$name`.
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 30));
        assert!(
            result.is_some(),
            "expected a location for property '$name', cursor at column 30"
        );
        let loc = result.unwrap();
        assert_eq!(
            loc.range.start.line, 1,
            "should jump to line 1 where the property is declared"
        );
        assert_eq!(loc.uri, uri(), "should be in the same file");
    }

    #[test]
    fn jumps_to_trait_method_definition() {
        let src = "<?php\ntrait Greeting {\n    public function sayHello(string $name): string { return ''; }\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(2, 22));
        assert!(
            result.is_some(),
            "expected location for trait method 'sayHello'"
        );
        assert_eq!(result.unwrap().range.start.line, 2);
    }

    // ── find_method_in_class_hierarchy ───────────────────────────────────────

    fn make_index(path: &str, src: &str) -> (Url, std::sync::Arc<crate::file_index::FileIndex>) {
        use crate::file_index::FileIndex;
        let u = Url::parse(&format!("file://{path}")).unwrap();
        let d = ParsedDoc::parse(src.to_string());
        (u, std::sync::Arc::new(FileIndex::extract(&d)))
    }

    #[test]
    fn hierarchy_finds_method_in_class_itself() {
        let (uri, idx) = make_index(
            "/a.php",
            "<?php\nclass Foo { public function bar(): void {} }",
        );
        let indexes = vec![(uri, idx)];
        let loc = find_method_in_class_hierarchy("Foo", "bar", &indexes);
        assert!(loc.is_some(), "expected bar() in Foo");
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn hierarchy_finds_method_in_parent() {
        let (base_uri, base_idx) = make_index(
            "/Base.php",
            "<?php\nclass Base { public function render(): void {} }",
        );
        let (cu, ci) = make_index("/Child.php", "<?php\nclass Child extends Base {}");
        let indexes = vec![(base_uri.clone(), base_idx), (cu, ci)];
        let loc = find_method_in_class_hierarchy("Child", "render", &indexes);
        assert!(loc.is_some(), "expected render() found via parent Base");
        assert_eq!(loc.unwrap().uri, base_uri);
    }

    #[test]
    fn hierarchy_finds_method_in_trait() {
        let (trait_uri, trait_idx) = make_index(
            "/Renderable.php",
            "<?php\ntrait Renderable { public function render(): void {} }",
        );
        let (pu, pi) = make_index("/Page.php", "<?php\nclass Page { use Renderable; }");
        let indexes = vec![(trait_uri.clone(), trait_idx), (pu, pi)];
        let loc = find_method_in_class_hierarchy("Page", "render", &indexes);
        assert!(loc.is_some(), "expected render() found via trait");
        assert_eq!(loc.unwrap().uri, trait_uri);
    }

    #[test]
    fn hierarchy_returns_none_for_missing_method() {
        let (uri, idx) = make_index("/Foo.php", "<?php\nclass Foo {}");
        let indexes = vec![(uri, idx)];
        assert!(find_method_in_class_hierarchy("Foo", "missing", &indexes).is_none());
    }

    #[test]
    fn hierarchy_handles_cycle_without_panic() {
        // Bogus source where A extends B extends A — must not loop forever.
        let (ua, ia) = make_index("/A.php", "<?php\nclass A extends B {}");
        let (ub, ib) = make_index("/B.php", "<?php\nclass B extends A {}");
        let indexes = vec![(ua, ia), (ub, ib)];
        let loc = find_method_in_class_hierarchy("A", "missing", &indexes);
        assert!(loc.is_none());
    }
}
