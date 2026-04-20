/// `textDocument/implementation` — find all classes that implement an interface
/// or extend a class with the given name.
use std::collections::HashMap;
use std::sync::Arc;

use php_ast::{NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Url};

use crate::ast::{ParsedDoc, SourceView};
use crate::util::word_at;

/// Returns `true` when the name written in an `extends`/`implements` clause
/// (given as its `to_string_repr()` string) refers to the symbol we are
/// searching for.
///
/// Two forms are accepted:
/// - Short-name match: `repr == word`
///   Covers the common case where both files use the same unqualified name.
/// - FQN match: `repr` (with any leading `\` stripped) `== fqn`
///   Covers files that write the fully-qualified form (`\App\Animal` or
///   `App\Animal`) while the cursor file imports the class with a `use`
///   statement and the cursor sits on the short alias.
#[inline]
fn name_matches(repr: &str, word: &str, fqn: Option<&str>) -> bool {
    repr == word || fqn.is_some_and(|f| repr.trim_start_matches('\\') == f)
}

/// Return all `Location`s where a class declares `extends Name` or
/// `implements Name`.
///
/// `fqn` is the fully-qualified name of the symbol (e.g. `"App\\Animal"`),
/// resolved from the calling file's `use` imports. When provided, extends/
/// implements clauses that spell out the FQN form (`\App\Animal` or
/// `App\Animal`) are also matched, in addition to the bare `word`.
pub fn find_implementations(
    word: &str,
    fqn: Option<&str>,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<Location> {
    let mut locations = Vec::new();
    for (uri, doc) in all_docs {
        let sv = doc.view();
        collect_implementations(&doc.program().stmts, word, fqn, sv, uri, &mut locations);
    }
    locations
}

/// Convenience wrapper: extract word at `position`, resolve it through
/// `file_imports`, then call `find_implementations`.
///
/// `file_imports` maps short names to their fully-qualified names as built
/// from the `use` statements in the current file (e.g. `"Animal"` →
/// `"App\\Animal"`). This allows goto_implementation to find classes that
/// write the FQN form in their `extends`/`implements` clause.
#[allow(dead_code)]
pub fn goto_implementation(
    source: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
    file_imports: &HashMap<String, String>,
) -> Vec<Location> {
    let word = match word_at(source, position) {
        Some(w) => w,
        None => return vec![],
    };
    let fqn = file_imports.get(&word).map(|s| s.as_str());
    find_implementations(&word, fqn, all_docs)
}

/// Find implementations using `FileIndex` entries (memory-efficient cross-file search).
pub fn find_implementations_from_index(
    word: &str,
    fqn: Option<&str>,
    indexes: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::file_index::FileIndex>,
    )],
) -> Vec<Location> {
    let mut locations = Vec::new();
    for (uri, idx) in indexes {
        for cls in &idx.classes {
            let extends_match = cls
                .parent
                .as_deref()
                .map(|p| name_matches(p, word, fqn))
                .unwrap_or(false);
            let implements_match = cls
                .implements
                .iter()
                .any(|iface| name_matches(iface, word, fqn));
            if extends_match || implements_match {
                let pos = tower_lsp::lsp_types::Position {
                    line: cls.start_line,
                    character: 0,
                };
                locations.push(Location {
                    uri: uri.clone(),
                    range: tower_lsp::lsp_types::Range {
                        start: pos,
                        end: pos,
                    },
                });
            }
        }
    }
    locations
}

fn collect_implementations(
    stmts: &[Stmt<'_, '_>],
    word: &str,
    fqn: Option<&str>,
    sv: SourceView<'_>,
    uri: &Url,
    out: &mut Vec<Location>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let extends_match = c
                    .extends
                    .as_ref()
                    .map(|e| name_matches(e.to_string_repr().as_ref(), word, fqn))
                    .unwrap_or(false);

                let implements_match = c
                    .implements
                    .iter()
                    .any(|iface| name_matches(iface.to_string_repr().as_ref(), word, fqn));

                if (extends_match || implements_match)
                    && let Some(class_name) = c.name
                {
                    out.push(Location {
                        uri: uri.clone(),
                        range: sv.name_range(class_name),
                    });
                }
            }
            StmtKind::Enum(e) => {
                let implements_match = e
                    .implements
                    .iter()
                    .any(|iface| name_matches(iface.to_string_repr().as_ref(), word, fqn));
                if implements_match {
                    out.push(Location {
                        uri: uri.clone(),
                        range: sv.name_range(e.name),
                    });
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_implementations(inner, word, fqn, sv, uri, out);
                }
            }
            _ => {}
        }
    }
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

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn no_imports() -> HashMap<String, String> {
        HashMap::new()
    }

    fn imports(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ── find_implementations ──────────────────────────────────────────────────

    #[test]
    fn finds_class_implementing_interface() {
        let src = "<?php\ninterface Countable {}\nclass MyList implements Countable {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Countable", None, &docs);
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].range.start.line, 2);
    }

    #[test]
    fn finds_class_extending_parent() {
        let src = "<?php\nclass Animal {}\nclass Dog extends Animal {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Animal", None, &docs);
        assert_eq!(locs.len(), 1);
    }

    #[test]
    fn no_implementations_for_unknown_name() {
        let src = "<?php\nclass Foo {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Bar", None, &docs);
        assert!(locs.is_empty());
    }

    #[test]
    fn finds_across_multiple_docs() {
        let a = doc("/a.php", "<?php\nclass DogA extends Animal {}");
        let b = doc("/b.php", "<?php\nclass DogB extends Animal {}");
        let locs = find_implementations("Animal", None, &[a, b]);
        assert_eq!(locs.len(), 2);
    }

    #[test]
    fn class_implementing_multiple_interfaces() {
        let src = "<?php\nclass Repo implements Countable, Serializable {}";
        let docs = vec![doc("/a.php", src)];
        let countable = find_implementations("Countable", None, &docs);
        let serializable = find_implementations("Serializable", None, &docs);
        assert_eq!(countable.len(), 1);
        assert_eq!(serializable.len(), 1);
    }

    #[test]
    fn enum_implementing_interface_is_found() {
        // PHP 8.1+ enums can implement interfaces.
        let src = "<?php\ninterface HasLabel {}\nenum Status: string implements HasLabel {\n    case Active = 'active';\n}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("HasLabel", None, &docs);
        assert_eq!(
            locs.len(),
            1,
            "expected enum Status as implementation of HasLabel, got: {:?}",
            locs
        );
        assert_eq!(
            locs[0].range.start.line, 2,
            "enum declaration should be on line 2"
        );
    }

    #[test]
    fn multiple_classes_in_same_doc_all_found() {
        // Three concrete classes all extend the same base.
        let src = "<?php\nclass Base {}\nclass A extends Base {}\nclass B extends Base {}\nclass C extends Base {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Base", None, &docs);
        assert_eq!(locs.len(), 3);
        let names: Vec<u32> = locs.iter().map(|l| l.range.start.line).collect();
        assert!(names.contains(&2));
        assert!(names.contains(&3));
        assert!(names.contains(&4));
    }

    #[test]
    fn class_that_extends_and_implements_produces_one_location() {
        // `class Child extends Parent implements Iface {}` — Child satisfies both
        // a search for "Parent" and for "Iface", but each search yields exactly
        // one Location (not two).
        let src = "<?php\nclass Child extends Parent implements Iface {}";
        let docs = vec![doc("/a.php", src)];
        assert_eq!(find_implementations("Parent", None, &docs).len(), 1);
        assert_eq!(find_implementations("Iface", None, &docs).len(), 1);
    }

    #[test]
    fn partial_name_match_is_not_returned() {
        // "Animal" must not match a class named "AnimalHouse".
        let src = "<?php\nclass AnimalHouse extends Creature {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Animal", None, &docs);
        assert!(
            locs.is_empty(),
            "partial name 'Animal' must not match 'AnimalHouse extends Creature'"
        );
    }

    #[test]
    fn empty_docs_returns_empty() {
        let locs = find_implementations("Animal", None, &[]);
        assert!(locs.is_empty());
    }

    #[test]
    fn braced_namespace_class_is_found() {
        // Classes inside `namespace Foo { ... }` (braced form) must be reachable.
        let src = "<?php\nnamespace App {\n    class Dog extends Animal {}\n}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Animal", None, &docs);
        assert_eq!(
            locs.len(),
            1,
            "expected Dog inside braced namespace, got: {locs:?}"
        );
        assert_eq!(locs[0].range.start.line, 2);
    }

    #[test]
    fn unbraced_namespace_class_is_found() {
        // Classes after `namespace Foo;` (unbraced form) appear as top-level
        // siblings in the AST and must be found without special handling.
        let src = "<?php\nnamespace App;\nclass Dog extends Animal {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Animal", None, &docs);
        assert_eq!(
            locs.len(),
            1,
            "expected Dog inside unbraced namespace, got: {locs:?}"
        );
        assert_eq!(locs[0].range.start.line, 2);
    }

    #[test]
    fn fully_qualified_extends_does_not_match_without_fqn_context() {
        // Without a resolved FQN (fqn=None), `extends \Animal` does NOT match a
        // search for bare "Animal". This is correct: the caller must supply the
        // FQN when it is available (via goto_implementation + file_imports).
        let src = "<?php\nclass Dog extends \\Animal {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Animal", None, &docs);
        assert!(
            locs.is_empty(),
            "without FQN context, '\\\\Animal' must not match bare 'Animal'"
        );
    }

    #[test]
    fn fqn_context_finds_fully_qualified_extends() {
        // With fqn=Some("App\\Animal"), `extends \App\Animal` IS found.
        let src = "<?php\nclass Dog extends \\App\\Animal {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Animal", Some("App\\Animal"), &docs);
        assert_eq!(
            locs.len(),
            1,
            "FQN-aware search must find 'extends \\\\App\\\\Animal', got: {locs:?}"
        );
    }

    #[test]
    fn fqn_context_finds_qualified_extends_without_leading_backslash() {
        // `extends App\Animal` (no leading `\`) is also matched by the FQN.
        let src = "<?php\nclass Dog extends App\\Animal {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Animal", Some("App\\Animal"), &docs);
        assert_eq!(
            locs.len(),
            1,
            "FQN-aware search must find 'extends App\\\\Animal', got: {locs:?}"
        );
    }

    #[test]
    fn fqn_context_still_matches_short_name_form() {
        // When fqn is provided, the bare short-name form is still matched so that
        // classes in the same namespace (which write `extends Animal`) are included.
        let src = "<?php\nclass Dog extends Animal {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Animal", Some("App\\Animal"), &docs);
        assert_eq!(
            locs.len(),
            1,
            "short-name form must still match when FQN is provided, got: {locs:?}"
        );
    }

    #[test]
    fn anonymous_class_does_not_cause_panic() {
        // Anonymous classes have no name (c.name == None) and must be skipped
        // silently without panicking.
        let src = "<?php\n$x = new class extends Animal {};";
        let docs = vec![doc("/a.php", src)];
        // We only care that this doesn't panic; anonymous classes have no name
        // to report a Location for.
        let _ = find_implementations("Animal", None, &docs);
    }

    #[test]
    fn location_uri_matches_source_doc() {
        let a = doc("/src/Dog.php", "<?php\nclass Dog extends Animal {}");
        let b = doc("/src/Cat.php", "<?php\nclass Cat extends Animal {}");
        let locs = find_implementations("Animal", None, &[a, b]);
        assert_eq!(locs.len(), 2);
        let uris: Vec<&str> = locs.iter().map(|l| l.uri.path()).collect();
        assert!(uris.contains(&"/src/Dog.php"));
        assert!(uris.contains(&"/src/Cat.php"));
    }

    // ── goto_implementation ───────────────────────────────────────────────────

    #[test]
    fn goto_implementation_uses_cursor_word() {
        let src = "<?php\ninterface Countable {}\nclass Repo implements Countable {}";
        let docs = vec![doc("/a.php", src)];
        let locs = goto_implementation(src, &docs, pos(1, 12), &no_imports());
        assert!(!locs.is_empty());
    }

    #[test]
    fn goto_implementation_on_whitespace_returns_empty() {
        // Cursor on a space — word_at returns None → empty result, no panic.
        let src = "<?php\nclass Dog extends Animal {}";
        let docs = vec![doc("/a.php", src)];
        // character 0 on line 0 is '<' — not an identifier char; word_at returns None
        let locs = goto_implementation(src, &docs, pos(0, 0), &no_imports());
        assert!(locs.is_empty());
    }

    #[test]
    fn goto_implementation_on_unimplemented_name_returns_empty() {
        let src = "<?php\nclass Standalone {}";
        let docs = vec![doc("/a.php", src)];
        // cursor on "Standalone" — no other class extends it
        let locs = goto_implementation(src, &docs, pos(1, 8), &no_imports());
        assert!(locs.is_empty());
    }

    #[test]
    fn goto_implementation_resolves_use_import_to_fqn() {
        // The cursor file imports `use App\Animal;` — the short name "Animal"
        // should be resolved to "App\Animal" and match `extends \App\Animal`.
        let cursor_src = "<?php\nuse App\\Animal;\ninterface Animal {}";
        let impl_src = "<?php\nclass Dog extends \\App\\Animal {}";
        let docs = vec![doc("/a.php", cursor_src), doc("/b.php", impl_src)];
        let file_imports = imports(&[("Animal", "App\\Animal")]);
        // cursor on "Animal" in the interface declaration (line 2, col 12)
        let locs = goto_implementation(cursor_src, &docs, pos(2, 12), &file_imports);
        assert_eq!(
            locs.len(),
            1,
            "expected Dog (extends \\\\App\\\\Animal) found via use-import FQN, got: {locs:?}"
        );
    }

    #[test]
    fn goto_implementation_resolves_use_import_relative_fqn() {
        // Same as above but with `extends App\Animal` (no leading `\`).
        let cursor_src = "<?php\nuse App\\Animal;\ninterface Animal {}";
        let impl_src = "<?php\nclass Dog extends App\\Animal {}";
        let docs = vec![doc("/a.php", cursor_src), doc("/b.php", impl_src)];
        let file_imports = imports(&[("Animal", "App\\Animal")]);
        let locs = goto_implementation(cursor_src, &docs, pos(2, 12), &file_imports);
        assert_eq!(
            locs.len(),
            1,
            "expected Dog (extends App\\\\Animal) found via use-import FQN, got: {locs:?}"
        );
    }

    // ── find_implementations_from_index ───────────────────────────────────────

    fn make_index(path: &str, src: &str) -> (Url, std::sync::Arc<crate::file_index::FileIndex>) {
        use crate::file_index::FileIndex;
        let u = uri(path);
        let d = ParsedDoc::parse(src.to_string());
        (u.clone(), std::sync::Arc::new(FileIndex::extract(&u, &d)))
    }

    #[test]
    fn from_index_finds_implementing_class() {
        let (circle_uri, circle_idx) = make_index(
            "/circle.php",
            "<?php\nclass Circle implements Drawable {\n    public function draw(): void {}\n}",
        );
        let indexes = vec![(circle_uri.clone(), circle_idx)];
        let locs = find_implementations_from_index("Drawable", None, &indexes);
        assert_eq!(
            locs.len(),
            1,
            "expected Circle as implementation of Drawable"
        );
        assert_eq!(locs[0].uri, circle_uri);
        assert_eq!(locs[0].range.start.line, 1, "Circle is declared on line 1");
    }

    #[test]
    fn from_index_finds_extending_class() {
        let (dog_uri, dog_idx) = make_index("/dog.php", "<?php\nclass Dog extends Animal {}");
        let indexes = vec![(dog_uri.clone(), dog_idx)];
        let locs = find_implementations_from_index("Animal", None, &indexes);
        assert_eq!(locs.len(), 1, "expected Dog as subclass of Animal");
        assert_eq!(locs[0].range.start.line, 1);
    }

    #[test]
    fn from_index_finds_across_multiple_files() {
        let (a_uri, a_idx) = make_index("/a.php", "<?php\nclass Cat extends Animal {}");
        let (b_uri, b_idx) = make_index("/b.php", "<?php\nclass Dog extends Animal {}");
        let indexes = vec![(a_uri, a_idx), (b_uri, b_idx)];
        let locs = find_implementations_from_index("Animal", None, &indexes);
        assert_eq!(locs.len(), 2, "expected both Cat and Dog");
    }
}
