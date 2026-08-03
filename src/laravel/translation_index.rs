//! `lang/**` (and legacy `resources/lang/**`) index powering go-to-definition
//! and completion for `__('a.b')` / `trans('a.b')` calls, plus the
//! literal-string JSON translation convention (`__('Original string')`).
//!
//! Laravel's current default location is `lang/{locale}/*.php` +
//! `lang/{locale}.json` at the project root; older/legacy projects (and
//! some packages) keep the same layout under `resources/lang/`. Both roots
//! are scanned — `lang/` first, so it wins a key collision. Within each
//! root, the `en` locale is visited before others (then alphabetically), so
//! the first locale to define a key wins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use php_ast::{ArrayElement, ExprKind, StmtKind};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CompletionItem, CompletionItemKind, Location,
    Position, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::analysis::diagnostics::parse_document_no_diags;
use crate::document::ast::{SourceView, offset_to_position};

#[derive(Debug, Default, Clone)]
pub struct TranslationIndex {
    keys: HashMap<String, Location>,
}

impl TranslationIndex {
    pub fn get(&self, key: &str) -> Option<&Location> {
        self.keys.get(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.keys.keys().map(String::as_str)
    }

    /// The translation key whose declaration contains `position`, if any —
    /// the reverse of `get`, used to recognize a find-references request
    /// starting from the definition site.
    pub fn key_at(&self, uri: &Uri, position: Position) -> Option<&str> {
        crate::laravel::location_lookup::key_at(&self.keys, uri, position)
    }

    pub(super) fn load(root: &Path) -> Self {
        let mut keys = HashMap::new();
        for lang_root in [root.join("lang"), root.join("resources").join("lang")] {
            load_lang_root(&lang_root, &mut keys);
        }
        Self { keys }
    }
}

fn load_lang_root(lang_root: &Path, out: &mut HashMap<String, Location>) {
    let Ok(entries) = std::fs::read_dir(lang_root) else {
        return;
    };
    let mut locale_dirs: Vec<PathBuf> = Vec::new();
    let mut json_files: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            locale_dirs.push(path);
        } else if path.extension().is_some_and(|e| e == "json") {
            json_files.push(path);
        }
    }
    // "en" first, then alphabetical — the first locale to define a key wins.
    locale_dirs.sort_by_key(|p| {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        (name != "en", name)
    });
    for dir in &locale_dirs {
        load_locale_php_files(dir, out);
    }
    json_files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    for file in &json_files {
        load_locale_json_file(file, out);
    }
}

/// Every `.php` file under one locale directory (e.g. `lang/en/`), including
/// nested subdirectories — a file's group name is its path relative to the
/// locale directory with the `.php` extension stripped and directory
/// separators kept as `/` (matching Laravel's own file-path-as-group-name
/// convention), so `lang/en/vendor/auth.php` becomes group `vendor/auth`.
fn load_locale_php_files(locale_dir: &Path, out: &mut HashMap<String, Location>) {
    for path in super::fs_walk::php_files_recursive(locale_dir) {
        let Some(group) = slash_prefix(locale_dir, &path) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(uri) = Uri::from_file_path(&path) else {
            continue;
        };
        let doc = parse_document_no_diags(&text);
        let sv = doc.view();
        for stmt in doc.program().stmts.iter() {
            if let StmtKind::Return(Some(expr)) = &stmt.kind
                && let ExprKind::Array(elements) = &expr.kind
            {
                collect_array_keys(elements, sv, &uri, &group, out);
            }
        }
    }
}

/// `path`'s location relative to `locale_dir` as a slash-joined group name —
/// directory components joined with the file stem, e.g. `en/vendor/auth.php`
/// under locale dir `en/` becomes `vendor/auth`.
fn slash_prefix(locale_dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(locale_dir).ok()?;
    let mut components: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let stem = Path::new(components.pop()?).file_stem()?.to_str()?;
    components.push(stem);
    Some(components.join("/"))
}

fn collect_array_keys(
    elements: &[ArrayElement<'_, '_>],
    sv: SourceView<'_>,
    uri: &Uri,
    prefix: &str,
    out: &mut HashMap<String, Location>,
) {
    for el in elements {
        let Some(key_expr) = &el.key else { continue };
        let ExprKind::String(key) = &key_expr.kind else {
            continue;
        };
        let dotted = format!("{prefix}.{key}");
        // `span.start`/`span.end` point at the surrounding quotes (see
        // `editing::document_link::link_from_path_expr`); trim one byte off
        // each side to land on the key text itself.
        let range = Range {
            start: sv.position_of(key_expr.span.start + 1),
            end: sv.position_of(key_expr.span.end - 1),
        };
        out.entry(dotted.clone()).or_insert_with(|| Location {
            uri: uri.clone(),
            range,
        });
        if let ExprKind::Array(nested) = &el.value.kind {
            collect_array_keys(nested, sv, uri, &dotted, out);
        }
    }
}

/// `lang/{locale}.json` — a flat map from the literal default-language
/// string to its translation. The map *key itself* (not a dotted path) is
/// what `__('...')` is called with.
fn load_locale_json_file(path: &Path, out: &mut HashMap<String, Location>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(map) = json.as_object() else {
        return;
    };
    let Some(uri) = Uri::from_file_path(path) else {
        return;
    };
    for key in map.keys() {
        if out.contains_key(key.as_str()) {
            continue;
        }
        if let Some(range) = find_json_key_range(&text, key) {
            out.insert(
                key.clone(),
                Location {
                    uri: uri.clone(),
                    range,
                },
            );
        }
    }
}

/// Locate `key`'s first `"key"` occurrence in raw JSON text and return the
/// range of the key text itself (quotes excluded). Manual text search
/// because `serde_json::Value` doesn't retain source spans. Shared with
/// `mix_index`/`vite_index`, which index a differently-shaped JSON manifest
/// the same way.
pub(super) fn find_json_key_range(text: &str, key: &str) -> Option<Range> {
    let escaped = key.replace('\\', "\\\\").replace('"', "\\\"");
    let needle = format!("\"{escaped}\"");
    let quote_byte = text.find(&needle)?;
    let byte_start = (quote_byte + 1) as u32;
    let byte_end = byte_start + escaped.len() as u32;
    let line_starts = line_starts_of(text);
    Some(Range {
        start: offset_to_position(text, &line_starts, byte_start),
        end: offset_to_position(text, &line_starts, byte_end),
    })
}

fn line_starts_of(text: &str) -> Vec<u32> {
    std::iter::once(0u32)
        .chain(text.match_indices('\n').map(|(i, _)| i as u32 + 1))
        .collect()
}

/// `lang/en.json`, falling back to the legacy `resources/lang/en.json`,
/// then the first `*.json` file found under either root (`lang/` before
/// `resources/lang/`) — matches `load_lang_root`'s own root preference.
fn target_translation_json(root: &Path) -> Option<PathBuf> {
    let lang_roots = [root.join("lang"), root.join("resources").join("lang")];
    for lang_root in &lang_roots {
        let en = lang_root.join("en.json");
        if en.is_file() {
            return Some(en);
        }
    }
    for lang_root in &lang_roots {
        let Ok(entries) = std::fs::read_dir(lang_root) else {
            continue;
        };
        let mut json_files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        json_files.sort();
        if let Some(first) = json_files.into_iter().next() {
            return Some(first);
        }
    }
    None
}

/// The `<group>.php` file backing a dotted `group.rest` key, if `group` has
/// one somewhere under `lang/`/`resources/lang/` (any locale). When present,
/// Laravel resolves the key via that array file (returning the key itself if
/// the specific entry is missing) and never falls back to the JSON literal
/// convention — so adding `key` verbatim to a JSON file would not fix the
/// actual lookup.
fn group_php_file_path(root: &Path, group: &str) -> Option<PathBuf> {
    for lang_root in [root.join("lang"), root.join("resources").join("lang")] {
        let Ok(entries) = std::fs::read_dir(&lang_root) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let candidate = entry.path().join(format!("{group}.php"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Quickfix for a dotted `group.item` miss whose `<group>.php` array file
/// exists but lacks `item` — inserts `'item' => '', ` right after the
/// `return [...]` array's opening `[`, which is always syntactically valid
/// regardless of the array's existing contents (same insertion technique as
/// `generate_validation_rules_action`, which sidesteps the risk of producing
/// invalid PHP that previously left this gap unfixed). Only offered for a
/// single-level `group.item` miss — a deeper `group.a.b` miss would need
/// locating (or building) a nested array and is left as a gap.
fn missing_translation_group_key_action(root: &Path, key: &str) -> Option<CodeActionOrCommand> {
    let (group, item) = key.split_once('.')?;
    if item.contains('.') {
        return None;
    }
    let path = group_php_file_path(root, group)?;
    let text = std::fs::read_to_string(&path).ok()?;
    let uri = Uri::from_file_path(&path)?;
    let doc = parse_document_no_diags(&text);
    let sv = doc.view();
    let array_span = doc
        .program()
        .stmts
        .iter()
        .find_map(|stmt| match &stmt.kind {
            StmtKind::Return(Some(expr)) if matches!(expr.kind, ExprKind::Array(_)) => {
                Some(expr.span)
            }
            _ => None,
        })?;
    let open_bracket = text[array_span.start as usize..array_span.end as usize]
        .find('[')
        .map(|i| array_span.start + i as u32 + 1)?;
    let pos = sv.position_of(open_bracket);
    let escaped_item = item.replace('\\', "\\\\").replace('\'', "\\'");
    let edit = TextEdit {
        range: Range {
            start: pos,
            end: pos,
        },
        new_text: format!("'{escaped_item}' => '', "),
    };
    let mut changes = HashMap::new();
    changes.insert(uri, vec![edit]);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("group.php");
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Add '{item}' to {file_name}"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

/// Quickfix offered when `__('...')`/`trans('...')` resolves to no
/// declaration anywhere in `lang/`/`resources/lang/`. Dotted `group.item`
/// misses with a matching `<group>.php` array file are fixed by inserting
/// into that array (see [`missing_translation_group_key_action`]); everything
/// else falls back to the JSON literal-translation convention.
pub(crate) fn missing_translation_json_key_action(
    root: &Path,
    key: &str,
) -> Option<CodeActionOrCommand> {
    if let Some((group, _)) = key.split_once('.')
        && group_php_file_path(root, group).is_some()
    {
        return missing_translation_group_key_action(root, key);
    }
    let path = target_translation_json(root)?;
    let text = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let map = json.as_object()?;
    let uri = Uri::from_file_path(&path)?;
    let brace = text.find('{')?;
    let insert_offset = (brace + 1) as u32;
    let line_starts = line_starts_of(&text);
    let pos = offset_to_position(&text, &line_starts, insert_offset);
    let key_literal = serde_json::to_string(key).ok()?;
    let new_text = if map.is_empty() {
        format!("{key_literal}: \"\"")
    } else {
        format!("{key_literal}: \"\", ")
    };
    let edit = TextEdit {
        range: Range {
            start: pos,
            end: pos,
        },
        new_text,
    };
    let mut changes = HashMap::new();
    changes.insert(uri, vec![edit]);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("lang.json");
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Add {key_literal} to {file_name}"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

/// Completion items for translation keys starting with `prefix`.
pub(crate) fn translation_completions(
    index: &TranslationIndex,
    prefix: &str,
) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = index
        .keys()
        .filter(|key| key.starts_with(prefix))
        .map(|key| CompletionItem {
            label: key.to_string(),
            kind: Some(CompletionItemKind::TEXT),
            insert_text: Some(key.to_string()),
            ..Default::default()
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_php_lang(root: &Path, lang_dir: &str, locale: &str, name: &str, contents: &str) {
        let dir = root.join(lang_dir).join(locale);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn indexes_nested_php_keys_under_lang() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn [\n    'failed' => 'These credentials do not match.',\n];\n",
        );
        let idx = TranslationIndex::load(tmp.path());
        let loc = idx.get("auth.failed").unwrap();
        assert!(loc.uri.as_str().ends_with("lang/en/auth.php"));
        assert_eq!(
            loc.range,
            Range {
                start: Position {
                    line: 2,
                    character: 5,
                },
                end: Position {
                    line: 2,
                    character: 11,
                },
            }
        );
    }

    #[test]
    fn prefers_en_locale_on_key_collision() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "lang",
            "es",
            "auth.php",
            "<?php\nreturn ['failed' => 'Spanish'];\n",
        );
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn ['failed' => 'English'];\n",
        );
        let idx = TranslationIndex::load(tmp.path());
        let loc = idx.get("auth.failed").unwrap();
        assert!(loc.uri.as_str().contains("/en/"));
    }

    #[test]
    fn prefers_root_lang_over_resources_lang() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "resources/lang",
            "en",
            "auth.php",
            "<?php\nreturn ['failed' => 'Legacy'];\n",
        );
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn ['failed' => 'Current'];\n",
        );
        let idx = TranslationIndex::load(tmp.path());
        let loc = idx.get("auth.failed").unwrap();
        assert!(!loc.uri.as_str().contains("resources"));
    }

    #[test]
    fn indexes_json_literal_translation_keys() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("lang")).unwrap();
        std::fs::write(
            tmp.path().join("lang").join("es.json"),
            r#"{"I love programming.": "Me encanta programar."}"#,
        )
        .unwrap();
        let idx = TranslationIndex::load(tmp.path());
        let loc = idx.get("I love programming.").unwrap();
        assert!(loc.uri.as_str().ends_with("es.json"));
        assert_eq!(loc.range.start.line, 0);
    }

    #[test]
    fn indexes_nested_locale_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn ['failed' => 'top'];\n",
        );
        let nested_dir = tmp.path().join("lang").join("en").join("vendor");
        std::fs::create_dir_all(&nested_dir).unwrap();
        std::fs::write(
            nested_dir.join("auth.php"),
            "<?php\nreturn ['failed' => 'nested'];\n",
        )
        .unwrap();
        let idx = TranslationIndex::load(tmp.path());
        assert!(idx.get("auth.failed").is_some());
        let nested = idx.get("vendor/auth.failed").unwrap();
        assert!(nested.uri.as_str().ends_with("lang/en/vendor/auth.php"));
    }

    #[test]
    fn missing_lang_dirs_yield_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = TranslationIndex::load(tmp.path());
        assert_eq!(idx.keys().count(), 0);
    }

    #[test]
    fn translation_completions_filters_by_prefix_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn [\n    'failed' => 'x',\n    'throttle' => 'y',\n];\n",
        );
        let idx = TranslationIndex::load(tmp.path());
        let items = translation_completions(&idx, "auth.");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["auth.failed", "auth.throttle"]);
    }

    #[test]
    fn missing_translation_json_key_action_inserts_into_existing_object() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("lang")).unwrap();
        std::fs::write(
            tmp.path().join("lang").join("en.json"),
            r#"{"Hello": "Hello"}"#,
        )
        .unwrap();
        let CodeActionOrCommand::CodeAction(action) =
            missing_translation_json_key_action(tmp.path(), "Goodbye").unwrap()
        else {
            panic!("expected a CodeAction");
        };
        assert_eq!(action.title, r#"Add "Goodbye" to en.json"#);
        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let edits = changes.values().next().unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, r#""Goodbye": "", "#);
        assert_eq!(edits[0].range.start.character, 1);
    }

    #[test]
    fn missing_translation_json_key_action_handles_empty_object() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("lang")).unwrap();
        std::fs::write(tmp.path().join("lang").join("en.json"), "{}").unwrap();
        let CodeActionOrCommand::CodeAction(action) =
            missing_translation_json_key_action(tmp.path(), "Hi").unwrap()
        else {
            panic!("expected a CodeAction");
        };
        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let edits = changes.values().next().unwrap();
        assert_eq!(edits[0].new_text, r#""Hi": """#);
    }

    #[test]
    fn missing_translation_json_key_action_escapes_quotes_in_key() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("lang")).unwrap();
        std::fs::write(tmp.path().join("lang").join("en.json"), "{}").unwrap();
        let CodeActionOrCommand::CodeAction(action) =
            missing_translation_json_key_action(tmp.path(), r#"Say "hi""#).unwrap()
        else {
            panic!("expected a CodeAction");
        };
        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let edits = changes.values().next().unwrap();
        assert_eq!(edits[0].new_text, r#""Say \"hi\"": """#);
    }

    #[test]
    fn missing_translation_json_key_action_none_without_any_json_file() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(missing_translation_json_key_action(tmp.path(), "Hi").is_none());
    }

    #[test]
    fn missing_translation_json_key_action_falls_back_to_first_json_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("lang")).unwrap();
        std::fs::write(tmp.path().join("lang").join("es.json"), "{}").unwrap();
        let CodeActionOrCommand::CodeAction(action) =
            missing_translation_json_key_action(tmp.path(), "Hi").unwrap()
        else {
            panic!("expected a CodeAction");
        };
        assert_eq!(action.title, r#"Add "Hi" to es.json"#);
    }

    #[test]
    fn missing_translation_json_key_action_inserts_into_group_file() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn ['failed' => 'x'];\n",
        );
        std::fs::write(tmp.path().join("lang").join("en.json"), "{}").unwrap();
        // `auth.php` exists but lacks `custom` — Laravel resolves via the
        // array file (returning the key itself), never falling back to the
        // JSON literal convention, so the fix must insert into `auth.php`,
        // not `en.json`.
        let CodeActionOrCommand::CodeAction(action) =
            missing_translation_json_key_action(tmp.path(), "auth.custom").unwrap()
        else {
            panic!("expected a CodeAction");
        };
        assert_eq!(action.title, "Add 'custom' to auth.php");
        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let (uri, edits) = changes.iter().next().unwrap();
        assert!(uri.as_str().ends_with("lang/en/auth.php"));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "'custom' => '', ");
    }

    #[test]
    fn missing_translation_json_key_action_none_for_nested_group_miss() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn ['failed' => 'x'];\n",
        );
        std::fs::write(tmp.path().join("lang").join("en.json"), "{}").unwrap();
        // `auth.a.b` is two levels below the group — inserting into a nested
        // array isn't supported, and falling back to the JSON convention
        // would be wrong since `auth.php` exists, so no quickfix at all.
        assert!(missing_translation_json_key_action(tmp.path(), "auth.a.b").is_none());
    }

    #[test]
    fn missing_translation_json_key_action_offered_for_dotted_key_without_group_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("lang")).unwrap();
        std::fs::write(tmp.path().join("lang").join("en.json"), "{}").unwrap();
        // No `messages.php` array file exists anywhere, so Laravel's `__()`
        // falls back to treating the whole dotted string as a JSON literal
        // key — the quickfix must fire in this case.
        let CodeActionOrCommand::CodeAction(action) =
            missing_translation_json_key_action(tmp.path(), "messages.welcome").unwrap()
        else {
            panic!("expected a CodeAction");
        };
        assert_eq!(action.title, r#"Add "messages.welcome" to en.json"#);
    }
}
