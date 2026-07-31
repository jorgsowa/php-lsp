use tower_lsp_server::ls_types::{CompletionItem, CompletionItemKind, Position, Uri};

use crate::text::utf16_offset_to_byte;

/// Returns the path prefix typed inside a string on an include/require line, or None.
/// Only triggers for relative paths (starting with `./`, `../`, or empty after the quote)
/// so that absolute-path strings are left alone.
pub(super) fn include_path_prefix(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    // Check if line contains include/require keyword (may be after <?php)
    if !line.contains("include") && !line.contains("require") {
        return None;
    }
    // Find the string being typed
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..col];
    let quote_pos = before.rfind(['\'', '"'])?;
    let typed = &before[quote_pos + 1..];
    // Only offer completions for relative paths (./  ../  or empty start)
    // and not for absolute paths (starting with /) or PHP stream wrappers.
    if typed.starts_with('/') || typed.contains("://") {
        return None;
    }
    Some(typed.to_string())
}

/// Build completion items for include/require path strings.
///
/// `prefix` is the partial path typed so far (e.g. `"../lib/"` or `"./"`).
/// The returned `insert_text` for each item is the full replacement text
/// from the opening quote to the end of the completed entry, so that the
/// LSP client can replace the whole typed path (not just the last segment).
pub(super) fn include_path_completions(doc_uri: &Uri, prefix: &str) -> Vec<CompletionItem> {
    use std::path::Path;

    let doc_path = match doc_uri.to_file_path() {
        Some(p) => p,
        None => return vec![],
    };
    let doc_dir = match doc_path.parent() {
        Some(d) => d.to_path_buf(),
        None => return vec![],
    };

    // Split prefix into a directory part (already traversed) and the partial filename.
    let (dir_prefix, typed_file) = if prefix.ends_with('/') || prefix.ends_with('\\') {
        (prefix.to_string(), String::new())
    } else {
        let p = Path::new(prefix);
        let parent = p
            .parent()
            .map(|p| {
                let s = p.to_string_lossy();
                if s.is_empty() {
                    String::new()
                } else {
                    format!("{}/", s)
                }
            })
            .unwrap_or_default();
        let file = p
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        (parent, file)
    };

    let dir_to_list = doc_dir.join(&dir_prefix);

    let entries = match std::fs::read_dir(&dir_to_list) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut items = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Skip hidden files/dirs unless the prefix already starts with a dot.
        if name.starts_with('.') && !typed_file.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let is_php = name.ends_with(".php") || name.ends_with(".inc") || name.ends_with(".phtml");
        if !is_dir && !is_php {
            continue;
        }
        let entry_name = if is_dir {
            format!("{}/", name)
        } else {
            name.clone()
        };
        // insert_text is the full path from the opening quote so the whole
        // typed prefix (e.g. "../lib/") is preserved in the replacement.
        let insert_text = format!("{}{}", dir_prefix, entry_name);
        items.push(CompletionItem {
            label: name,
            kind: Some(if is_dir {
                CompletionItemKind::FOLDER
            } else {
                CompletionItemKind::FILE
            }),
            insert_text: Some(insert_text),
            ..Default::default()
        });
    }
    items.sort_by(|a, b| {
        // Directories first, then files
        let a_dir = a.kind == Some(CompletionItemKind::FOLDER);
        let b_dir = b.kind == Some(CompletionItemKind::FOLDER);
        b_dir.cmp(&a_dir).then(a.label.cmp(&b.label))
    });
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn include_path_prefix_returns_none_for_non_include_line() {
        let src = "<?php\n$x = 'some string';";
        let pos = Position {
            line: 1,
            character: 14,
        };
        assert!(
            include_path_prefix(src, pos).is_none(),
            "should not trigger on non-include line"
        );
    }

    #[test]
    fn include_path_prefix_returns_none_for_absolute_path() {
        let src = "<?php\nrequire '/absolute/path/file.php';";
        let pos = Position {
            line: 1,
            character: 30,
        };
        assert!(
            include_path_prefix(src, pos).is_none(),
            "should not trigger for absolute paths"
        );
    }

    #[test]
    fn include_path_prefix_returns_none_for_stream_wrapper() {
        let src = "<?php\nrequire 'phar://archive.phar/file.php';";
        let pos = Position {
            line: 1,
            character: 35,
        };
        assert!(
            include_path_prefix(src, pos).is_none(),
            "should not trigger for stream wrappers"
        );
    }

    #[test]
    fn include_path_prefix_returns_relative_dot_slash() {
        let src = "<?php\nrequire './lib/Helper";
        let pos = Position {
            line: 1,
            character: 23,
        };
        let result = include_path_prefix(src, pos);
        assert_eq!(
            result.as_deref(),
            Some("./lib/Helper"),
            "should return the typed relative path prefix"
        );
    }

    #[test]
    fn include_path_prefix_returns_double_dot_prefix() {
        let src = "<?php\ninclude '../utils/";
        let pos = Position {
            line: 1,
            character: 22,
        };
        let result = include_path_prefix(src, pos);
        assert_eq!(
            result.as_deref(),
            Some("../utils/"),
            "should return ../utils/ prefix"
        );
    }

    #[test]
    fn include_path_prefix_returns_empty_for_bare_quote() {
        let src = "<?php\nrequire '";
        let pos = Position {
            line: 1,
            character: 10,
        };
        let result = include_path_prefix(src, pos);
        assert_eq!(
            result.as_deref(),
            Some(""),
            "bare quote should return empty prefix (list current dir)"
        );
    }

    #[test]
    fn include_path_completions_lists_relative_directory() {
        use std::fs;

        let tmp = tempfile::tempdir().expect("tmpdir");
        let subdir = tmp.path().join("lib");
        fs::create_dir_all(&subdir).expect("create lib dir");
        fs::write(subdir.join("Helper.php"), "<?php").expect("write Helper.php");
        fs::write(subdir.join("Utils.php"), "<?php").expect("write Utils.php");
        // Non-PHP file that should be excluded
        fs::write(subdir.join("README.md"), "# readme").expect("write README.md");

        let doc_path = tmp.path().join("index.php");
        let doc_uri = Uri::from_file_path(&doc_path).expect("doc uri");

        // Prefix "./lib/" — should list the lib directory contents
        let items = include_path_completions(&doc_uri, "./lib/");
        let ls: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(ls.contains(&"Helper.php"), "should list Helper.php");
        assert!(ls.contains(&"Utils.php"), "should list Utils.php");
        assert!(
            !ls.contains(&"README.md"),
            "non-PHP files should be excluded"
        );
    }

    #[test]
    fn include_path_completions_insert_text_includes_directory_prefix() {
        use std::fs;

        let tmp = tempfile::tempdir().expect("tmpdir");
        let subdir = tmp.path().join("src");
        fs::create_dir_all(&subdir).expect("create src dir");
        fs::write(subdir.join("Boot.php"), "<?php").expect("write Boot.php");

        let doc_path = tmp.path().join("main.php");
        let doc_uri = Uri::from_file_path(&doc_path).expect("doc uri");

        let items = include_path_completions(&doc_uri, "./src/");
        let boot = items.iter().find(|i| i.label == "Boot.php");
        assert!(boot.is_some(), "Boot.php should be in completions");
        assert_eq!(
            boot.unwrap().insert_text.as_deref(),
            Some("./src/Boot.php"),
            "insert_text should include the directory prefix"
        );
    }

    #[test]
    fn include_path_completions_is_empty_for_non_existent_directory() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let doc_path = tmp.path().join("index.php");
        let doc_uri = Uri::from_file_path(&doc_path).expect("doc uri");

        let items = include_path_completions(&doc_uri, "./nonexistent/");
        assert!(
            items.is_empty(),
            "should return empty list for non-existent directory"
        );
    }

    #[test]
    fn include_path_completions_dir_entries_have_folder_kind() {
        use std::fs;

        let tmp = tempfile::tempdir().expect("tmpdir");
        let subdir = tmp.path().join("modules");
        fs::create_dir_all(&subdir).expect("create modules dir");

        let doc_path = tmp.path().join("index.php");
        let doc_uri = Uri::from_file_path(&doc_path).expect("doc uri");

        let items = include_path_completions(&doc_uri, "");
        let modules = items.iter().find(|i| i.label == "modules");
        assert!(modules.is_some(), "modules dir should be in completions");
        assert_eq!(
            modules.unwrap().kind,
            Some(CompletionItemKind::FOLDER),
            "directory should have FOLDER kind"
        );
        assert_eq!(
            modules.unwrap().insert_text.as_deref(),
            Some("modules/"),
            "directory insert_text should end with /"
        );
    }
}
