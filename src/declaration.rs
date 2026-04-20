/// `textDocument/declaration` — jump to the abstract or interface declaration of a symbol.
///
/// In PHP the distinction between declaration and definition matters for:
///   - Interface methods (declared but never given a body)
///   - Abstract class methods
///
/// For concrete symbols with no abstract counterpart this falls back to the same
/// result as go-to-definition so the request is never empty-handed.
use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Url};

use crate::ast::{ParsedDoc, SourceView};
use crate::util::word_at;

/// Find the abstract or interface declaration of `word`.
/// Prefers abstract/interface declarations; falls back to any declaration.
pub fn goto_declaration(
    source: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Location> {
    let word = word_at(source, position)?;

    // First pass: look for an abstract or interface declaration
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(range) = find_abstract_declaration(sv, &doc.program().stmts, &word) {
            return Some(Location {
                uri: uri.clone(),
                range,
            });
        }
    }

    // Second pass: any declaration (same as goto_definition)
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(range) = find_any_declaration(sv, &doc.program().stmts, &word) {
            return Some(Location {
                uri: uri.clone(),
                range,
            });
        }
    }

    None
}

fn find_abstract_declaration(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    word: &str,
) -> Option<tower_lsp::lsp_types::Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Interface(i) => {
                // Interface methods are declarations without bodies
                for member in i.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == word
                    {
                        return Some(sv.name_range(m.name));
                    }
                }
                if i.name == word {
                    return Some(sv.name_range(i.name));
                }
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.is_abstract
                        && m.name == word
                    {
                        return Some(sv.name_range(m.name));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = find_abstract_declaration(sv, inner, word)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_any_declaration(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    word: &str,
) -> Option<tower_lsp::lsp_types::Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                return Some(sv.name_range(f.name));
            }
            StmtKind::Class(c) if c.name == Some(word) => {
                return Some(sv.name_range(c.name.expect("match guard ensures Some")));
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == word
                    {
                        return Some(sv.name_range(m.name));
                    }
                }
            }
            StmtKind::Interface(i) if i.name == word => {
                return Some(sv.name_range(i.name));
            }
            StmtKind::Trait(t) if t.name == word => {
                return Some(sv.name_range(t.name));
            }
            StmtKind::Enum(e) if e.name == word => {
                return Some(sv.name_range(e.name));
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name == word
                    {
                        return Some(sv.name_range(m.name));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = find_any_declaration(sv, inner, word)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find abstract or interface declaration using `FileIndex` entries.
pub fn goto_declaration_from_index(
    source: &str,
    indexes: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::file_index::FileIndex>,
    )],
    position: tower_lsp::lsp_types::Position,
) -> Option<Location> {
    use crate::file_index::ClassKind;
    use crate::util::word_at;
    let word = word_at(source, position)?;

    let line_range = |line: u32| -> tower_lsp::lsp_types::Range {
        let p = tower_lsp::lsp_types::Position { line, character: 0 };
        tower_lsp::lsp_types::Range { start: p, end: p }
    };

    // First pass: abstract/interface declarations.
    for (uri, idx) in indexes {
        for cls in &idx.classes {
            if cls.kind == ClassKind::Interface {
                // Interface itself.
                if cls.name == word {
                    return Some(Location {
                        uri: uri.clone(),
                        range: line_range(cls.start_line),
                    });
                }
                // Abstract method in interface.
                for m in &cls.methods {
                    if m.name == word {
                        return Some(Location {
                            uri: uri.clone(),
                            range: line_range(m.start_line),
                        });
                    }
                }
            } else if cls.is_abstract {
                for m in &cls.methods {
                    if m.is_abstract && m.name == word {
                        return Some(Location {
                            uri: uri.clone(),
                            range: line_range(m.start_line),
                        });
                    }
                }
            }
        }
    }

    // Second pass: any declaration.
    for (uri, idx) in indexes {
        for f in &idx.functions {
            if f.name == word {
                return Some(Location {
                    uri: uri.clone(),
                    range: line_range(f.start_line),
                });
            }
        }
        for cls in &idx.classes {
            if cls.name == word {
                return Some(Location {
                    uri: uri.clone(),
                    range: line_range(cls.start_line),
                });
            }
            for m in &cls.methods {
                if m.name == word {
                    return Some(Location {
                        uri: uri.clone(),
                        range: line_range(m.start_line),
                    });
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(path: &str, src: &str) -> (Url, Arc<ParsedDoc>) {
        (uri(path), Arc::new(ParsedDoc::parse(src.to_string())))
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn finds_interface_method_declaration() {
        let src = "<?php\ninterface Logger { public function log(string $msg): void; }\nclass FileLogger implements Logger { public function log(string $msg): void {} }";
        let docs = vec![doc("/a.php", src)];
        let loc = goto_declaration(src, &docs, pos(2, 53));
        assert!(loc.is_some(), "expected a declaration location");
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn finds_abstract_method_declaration() {
        let src = "<?php\nabstract class Base { abstract public function build(): void; }\nclass Impl extends Base { public function build(): void {} }";
        let docs = vec![doc("/a.php", src)];
        let loc = goto_declaration(src, &docs, pos(2, 42));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn falls_back_to_definition_for_concrete_function() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let loc = goto_declaration(src, &docs, pos(2, 2));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn finds_interface_name_declaration() {
        let src = "<?php\ninterface Countable {}";
        let docs = vec![doc("/a.php", src)];
        let loc = goto_declaration(src, &docs, pos(1, 12));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn cross_file_interface_declaration() {
        let impl_src =
            "<?php\nclass Repo implements Countable { public function count(): int { return 0; } }";
        let iface_src = "<?php\ninterface Countable { public function count(): int; }";
        let iface_uri = uri("/iface.php");
        let docs = vec![
            doc("/impl.php", impl_src),
            (
                iface_uri.clone(),
                Arc::new(ParsedDoc::parse(iface_src.to_string())),
            ),
        ];
        let loc = goto_declaration(impl_src, &docs, pos(1, 51));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().uri, iface_uri);
    }

    #[test]
    fn returns_none_for_unknown_word() {
        let src = "<?php\n$x = 1;";
        let docs = vec![doc("/a.php", src)];
        let loc = goto_declaration(src, &docs, pos(1, 1));
        assert!(loc.is_none());
    }

    #[test]
    fn finds_enum_method_declaration() {
        let src = "<?php\nenum Suit { public function label(): string; }\nclass Backing implements SomeInterface { public function label(): string { return ''; } }";
        let docs = vec![doc("/a.php", src)];
        // Position the cursor on the enum method name "label" (line 1, col ~29)
        let loc = goto_declaration(src, &docs, pos(1, 29));
        assert!(
            loc.is_some(),
            "expected declaration location for enum method"
        );
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    // ── goto_declaration_from_index ───────────────────────────────────────────

    fn make_index(path: &str, src: &str) -> (Url, std::sync::Arc<crate::file_index::FileIndex>) {
        use crate::file_index::FileIndex;
        let u = uri(path);
        let d = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&u, &d);
        (u, std::sync::Arc::new(idx))
    }

    #[test]
    fn from_index_finds_abstract_method() {
        // abstract speak() is in animal.php; concrete speak() is in cat.php.
        // Cursor in cat.php source must resolve to the abstract declaration.
        let (animal_uri, animal_idx) = make_index(
            "/animal.php",
            "<?php\nabstract class Animal {\n    abstract public function speak(): string;\n}",
        );
        let cat_src = "<?php\nclass Cat extends Animal {\n    public function speak(): string { return 'meow'; }\n}";
        let (cat_uri, cat_idx) = make_index("/cat.php", cat_src);

        let indexes = vec![(animal_uri.clone(), animal_idx), (cat_uri, cat_idx)];
        // "    public function " = 20 chars → 's' of speak is at char 20 on line 2.
        let loc = goto_declaration_from_index(cat_src, &indexes, pos(2, 20));
        assert!(loc.is_some(), "expected abstract declaration");
        let loc = loc.unwrap();
        assert_eq!(loc.uri, animal_uri, "should point to animal.php");
        // "    abstract public function " = 28 chars → speak starts at char 28 on line 2.
        assert_eq!(
            loc.range.start.line, 2,
            "abstract speak is on line 2 of animal.php"
        );
    }

    #[test]
    fn from_index_finds_interface_method() {
        let (iface_uri, iface_idx) = make_index(
            "/logger.php",
            "<?php\ninterface Logger {\n    public function log(string $msg): void;\n}",
        );
        let impl_src = "<?php\nclass FileLogger implements Logger {\n    public function log(string $msg): void {}\n}";
        let (impl_uri, impl_idx) = make_index("/file_logger.php", impl_src);

        let indexes = vec![(iface_uri.clone(), iface_idx), (impl_uri, impl_idx)];
        // "    public function " = 20 chars → 'l' of log at char 20 on line 2.
        let loc = goto_declaration_from_index(impl_src, &indexes, pos(2, 20));
        assert!(loc.is_some(), "expected interface method declaration");
        let loc = loc.unwrap();
        assert_eq!(loc.uri, iface_uri, "should point to logger.php");
        assert_eq!(
            loc.range.start.line, 2,
            "interface log is on line 2 of logger.php"
        );
    }
}
