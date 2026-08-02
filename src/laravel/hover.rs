//! Hover for Laravel string-key calls (`env`, `config`, `view`, `trans`/`__`,
//! `route`, `asset`, middleware aliases) — reuses the same domain indexes as
//! go-to-definition and completion, so a hover is only ever offered where a
//! jump would also succeed.

use std::path::Path;

use tower_lsp_server::ls_types::{Hover, HoverContents, Location, MarkupContent, MarkupKind};

/// Builds a hover for a resolved Laravel string-key `location`. `heading` is
/// the call as written (e.g. `config('app.name')`); when `show_snippet` is
/// true, the defining line is read back from disk and shown as a fenced code
/// block in `lang` — skipped for domains like `view`/`asset` where the
/// location is a zero-width file marker rather than a line worth quoting.
pub(crate) fn key_hover(
    root: Option<&Path>,
    location: &Location,
    heading: &str,
    lang: &str,
    show_snippet: bool,
) -> Hover {
    let mut value = format!("**{heading}**\n\n");
    if show_snippet && let Some(line) = defining_line(location) {
        value.push_str(&format!("```{lang}\n{}\n```\n\n", line.trim()));
    }
    value.push_str(&format!("`{}`", display_path(location, root)));
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: None,
    }
}

/// The source line `location` points at, read fresh from disk — the domain
/// indexes only retain `Location`s, not the resolved value, so hover re-reads
/// the (already scanned, on-disk) file rather than threading value storage
/// through every index.
fn defining_line(location: &Location) -> Option<String> {
    let path = location.uri.to_file_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .nth(location.range.start.line as usize)
        .map(str::to_string)
}

/// `location`'s file path, relative to `root` when it's an ancestor —
/// otherwise the full path, or the raw URI as a last resort (e.g. a URI that
/// isn't a `file://` path at all, which shouldn't happen for these
/// filesystem-scanned indexes but costs nothing to guard against).
fn display_path(location: &Location, root: Option<&Path>) -> String {
    let Some(path) = location.uri.to_file_path() else {
        return location.uri.as_str().to_string();
    };
    let shown = root
        .and_then(|r| path.strip_prefix(r).ok())
        .unwrap_or(&path);
    shown.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::{Position, Range, Uri};

    fn loc(uri: &Uri, line: u32, sc: u32, ec: u32) -> Location {
        Location {
            uri: uri.clone(),
            range: Range {
                start: Position {
                    line,
                    character: sc,
                },
                end: Position {
                    line,
                    character: ec,
                },
            },
        }
    }

    #[test]
    fn key_hover_shows_heading_snippet_and_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".env"), "APP_NAME=Test\n").unwrap();
        let uri = Uri::from_file_path(tmp.path().join(".env")).unwrap();
        let location = loc(&uri, 0, 0, 8);
        let hover = key_hover(
            Some(tmp.path()),
            &location,
            "env('APP_NAME')",
            "properties",
            true,
        );
        let HoverContents::Markup(content) = hover.contents else {
            panic!("expected markup contents");
        };
        assert!(content.value.contains("env('APP_NAME')"));
        assert!(content.value.contains("APP_NAME=Test"));
        assert!(content.value.contains(".env"));
    }

    #[test]
    fn key_hover_skips_snippet_when_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("welcome.blade.php"), "<h1>Hi</h1>").unwrap();
        let uri = Uri::from_file_path(tmp.path().join("welcome.blade.php")).unwrap();
        let location = loc(&uri, 0, 0, 0);
        let hover = key_hover(Some(tmp.path()), &location, "view('welcome')", "php", false);
        let HoverContents::Markup(content) = hover.contents else {
            panic!("expected markup contents");
        };
        assert!(!content.value.contains("```"));
        assert!(content.value.contains("welcome.blade.php"));
    }

    #[test]
    fn display_path_is_relative_to_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("config")).unwrap();
        std::fs::write(tmp.path().join("config").join("app.php"), "<?php\n").unwrap();
        let uri = Uri::from_file_path(tmp.path().join("config").join("app.php")).unwrap();
        let location = loc(&uri, 0, 0, 0);
        assert_eq!(display_path(&location, Some(tmp.path())), "config/app.php");
    }

    #[test]
    fn display_path_falls_back_to_full_path_without_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".env"), "X=1\n").unwrap();
        let uri = Uri::from_file_path(tmp.path().join(".env")).unwrap();
        let location = loc(&uri, 0, 0, 0);
        assert!(display_path(&location, None).ends_with(".env"));
    }
}
