//! Regression coverage for the `url::Url` -> `tower_lsp_server::ls_types::Uri`
//! migration. The new `Uri` wraps `fluent_uri` (strict RFC 3986) instead of
//! `url::Url` (WHATWG-style) — different percent-encoding/normalization
//! rules mean two independently-constructed URIs for the same file could in
//! principle stop comparing equal, which would show up as silent breakage
//! (a file indexed twice, references never matching) rather than a compile
//! error. These tests exercise the exact file-path shapes this codebase
//! relies on: spaces, non-ASCII characters, and cross-file lookups through
//! `DocumentStore`'s `Uri`-keyed maps.

use super::*;
use tower_lsp_server::ls_types::Uri;

/// A `Uri`'s own wire-format string (what the server actually sends the
/// client, e.g. in `publishDiagnostics` or a `definition` response) must
/// parse back to an equal, equally-hashable `Uri` — this is exactly the
/// round trip a real editor performs: it echoes back a URI the server gave
/// it earlier (e.g. in `textDocument/didClose` after a `definition` reply),
/// and `DocumentStore`'s `Uri`-keyed maps must recognize it as the same file.
///
/// Note: this parses the *percent-encoded* wire form, not a raw path with an
/// unencoded space — a real LSP client never sends the latter (RFC 3986
/// requires encoding), so testing against it would exercise a case that
/// can't occur on the wire.
#[test]
fn from_file_path_wire_string_roundtrips_for_space_and_unicode_paths() {
    for path in [
        "/tmp/php-lsp test dir/Foo.php",
        "/tmp/php-lsp-é-测试/Bar.php",
        "/tmp/has spaces/and-é-unicode/Baz.php",
    ] {
        let from_path = Uri::from_file_path(path)
            .unwrap_or_else(|| panic!("from_file_path failed for {path:?}"));
        let wire_string = from_path.as_str().to_string();
        let parsed: Uri = wire_string
            .parse()
            .unwrap_or_else(|e| panic!("re-parsing {wire_string:?} (from {path:?}) failed: {e:?}"));
        assert_eq!(
            from_path, parsed,
            "Uri::from_file_path and re-parsing its own wire string disagree for {path:?}"
        );

        // Hash equality matters as much as `==`: DocumentStore's DashMap
        // relies on it to look up the same file regardless of which
        // construction path produced the key.
        let mut hasher_a = std::collections::hash_map::DefaultHasher::new();
        let mut hasher_b = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&from_path, &mut hasher_a);
        std::hash::Hash::hash(&parsed, &mut hasher_b);
        assert_eq!(
            std::hash::Hasher::finish(&hasher_a),
            std::hash::Hasher::finish(&hasher_b),
            "hash mismatch between equal Uris for {path:?}"
        );
    }
}

/// `to_file_path` must round-trip back to the original path for names
/// containing spaces and non-ASCII characters — the two most common ways a
/// percent-encoding bug would show up (e.g. a workspace under a OneDrive
/// path, or a project directory with accented characters).
#[test]
fn to_file_path_roundtrips_space_and_unicode_paths() {
    for path in [
        "/tmp/php-lsp test dir/Foo.php",
        "/tmp/php-lsp-é-测试/Bar.php",
    ] {
        let uri = Uri::from_file_path(path).unwrap();
        let back = uri
            .to_file_path()
            .unwrap_or_else(|| panic!("to_file_path failed for {path:?}"));
        assert_eq!(
            back.as_ref(),
            std::path::Path::new(path),
            "round-trip mismatch for {path:?}"
        );
    }
}

/// End-to-end: a workspace root whose directory name contains a space and
/// non-ASCII characters must still support basic navigation. This exercises
/// the full wire path — client-sent `didOpen`/`hover` URIs, workspace
/// scanning's `from_file_path` construction, and `DocumentStore`'s
/// `Uri`-keyed lookups — all of which must agree on the same identity for
/// the same file.
#[tokio::test]
async fn workspace_root_with_space_and_unicode_supports_hover_and_definition() {
    let tmp = tempfile::Builder::new()
        .prefix("php-lsp test é 测试 ")
        .tempdir()
        .expect("create tempdir with unicode/space prefix");
    let root = tmp.path();

    std::fs::write(
        root.join("Greeter.php"),
        "<?php\nclass Greeter {\n    public function hello(): string {\n        return 'hi';\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("main.php"),
        "<?php\n$g = new Greeter();\n$g->hello();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(root).await;
    server.wait_for_index_ready().await;
    server
        .open("main.php", "<?php\n$g = new Greeter();\n$g->hello();\n")
        .await;

    // Go-to-definition on `Greeter` must resolve across files — this only
    // works if the background-scanned Greeter.php (indexed via
    // `Uri::from_file_path`) and the open main.php (whose reference lookup
    // goes through the same `Uri`-keyed index) agree on file identity.
    // `definition` may reply with a scalar Location or an array, hence
    // `render_locations` (shared with every other navigation test) rather
    // than assuming array shape.
    let resp = server.definition("main.php", 1, 10).await;
    let rendered = render_locations(&resp, &server.uri(""));
    assert!(
        rendered.contains("Greeter.php"),
        "expected definition to point at Greeter.php, got {rendered:?} (raw: {resp:?})"
    );

    // Hover on the method call must also resolve through the same index.
    let hover = server.hover("main.php", 2, 4).await;
    assert!(
        hover["error"].is_null(),
        "hover errored in a space+unicode workspace root: {hover:?}"
    );
    assert!(
        !hover["result"].is_null(),
        "expected hover info for hello() in a space+unicode workspace root"
    );
}
