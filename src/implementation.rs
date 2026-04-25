/// `textDocument/implementation` — find all classes that implement an interface
/// or extend a class with the given name.
use std::sync::Arc;

use php_ast::{NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Url};

use crate::ast::{ParsedDoc, SourceView};

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

/// Phase J — Find implementations via the salsa-memoized workspace aggregate.
/// Uses the pre-built `subtypes_of[word]` reverse map for O(matches) lookups,
/// with an additional pass over the FQN's `subtypes_of` entry when the caller
/// supplied one (covers classes that wrote out the fully-qualified form in
/// their `extends`/`implements` clause). Replaces the old
/// `find_implementations_from_index` which walked every file's classes.
pub fn find_implementations_from_workspace(
    word: &str,
    fqn: Option<&str>,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
) -> Vec<Location> {
    let mut locations = Vec::new();
    let mut push_refs = |key: &str| {
        if let Some(refs) = wi.subtypes_of.get(key) {
            for r in refs {
                if let Some((uri, cls)) = wi.at(*r) {
                    // Re-check with `name_matches` so a bare-name subtype_of
                    // entry survives an FQN-qualified search and vice versa.
                    let extends_match = cls
                        .parent
                        .as_deref()
                        .map(|p| name_matches(p, word, fqn))
                        .unwrap_or(false);
                    let implements_match = cls
                        .implements
                        .iter()
                        .any(|iface| name_matches(iface.as_ref(), word, fqn));
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
        }
    };
    push_refs(word);
    if let Some(f) = fqn
        && f != word
    {
        push_refs(f);
        // Cover `\App\Animal`-style leading-backslash forms.
        let trimmed = f.trim_start_matches('\\');
        if trimmed != f {
            push_refs(trimmed);
        }
    }
    // De-dup: a class may list both the bare name and the FQN of the same
    // parent (unlikely but cheap to guard against).
    locations.sort_by(|a, b| {
        a.uri
            .as_str()
            .cmp(b.uri.as_str())
            .then(a.range.start.line.cmp(&b.range.start.line))
    });
    locations.dedup_by(|a, b| a.uri == b.uri && a.range.start.line == b.range.start.line);
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

    // ── find_implementations_from_workspace ──────────────────────────────────
    //
    // Phase J: these tests build a `WorkspaceIndexData` directly via
    // `from_files` (no AnalysisHost needed) so they exercise the reverse-map
    // shape the backend actually uses in production.

    fn make_index(path: &str, src: &str) -> (Url, std::sync::Arc<crate::file_index::FileIndex>) {
        use crate::file_index::FileIndex;
        let u = uri(path);
        let d = ParsedDoc::parse(src.to_string());
        (u.clone(), std::sync::Arc::new(FileIndex::extract(&d)))
    }

    #[test]
    fn from_workspace_finds_implementing_class() {
        let (circle_uri, circle_idx) = make_index(
            "/circle.php",
            "<?php\nclass Circle implements Drawable {\n    public function draw(): void {}\n}",
        );
        let wi = crate::db::workspace_index::WorkspaceIndexData::from_files(vec![(
            circle_uri.clone(),
            circle_idx,
        )]);
        let locs = find_implementations_from_workspace("Drawable", None, &wi);
        assert_eq!(
            locs.len(),
            1,
            "expected Circle as implementation of Drawable"
        );
        assert_eq!(locs[0].uri, circle_uri);
        assert_eq!(locs[0].range.start.line, 1, "Circle is declared on line 1");
    }

    #[test]
    fn from_workspace_finds_extending_class() {
        let (dog_uri, dog_idx) = make_index("/dog.php", "<?php\nclass Dog extends Animal {}");
        let wi =
            crate::db::workspace_index::WorkspaceIndexData::from_files(vec![(dog_uri, dog_idx)]);
        let locs = find_implementations_from_workspace("Animal", None, &wi);
        assert_eq!(locs.len(), 1, "expected Dog as subclass of Animal");
        assert_eq!(locs[0].range.start.line, 1);
    }

    #[test]
    fn from_workspace_finds_across_multiple_files() {
        let (a_uri, a_idx) = make_index("/a.php", "<?php\nclass Cat extends Animal {}");
        let (b_uri, b_idx) = make_index("/b.php", "<?php\nclass Dog extends Animal {}");
        let wi = crate::db::workspace_index::WorkspaceIndexData::from_files(vec![
            (a_uri, a_idx),
            (b_uri, b_idx),
        ]);
        let locs = find_implementations_from_workspace("Animal", None, &wi);
        assert_eq!(locs.len(), 2, "expected both Cat and Dog");
    }
}
