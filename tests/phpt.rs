//! `.phpt` fixture suite: each file under `tests/phpt/` is one test that drives
//! a real server over the LSP wire protocol.
//!
//! # Format
//!
//! ```text
//! ===description===
//! Hover on a method shows its signature.
//! ===config===
//! {"phpVersion": "8.1"}
//! ===request===
//! hover
//! ===file:src/Greeter.php===
//! <?php
//! class Greeter { public function hello(): string { return ''; } }
//! ===file:main.php===
//! <?php
//! (new Greeter)->hel$0lo();
//! ===expect===
//! <rendered response>
//! ```
//!
//! Header sections, all optional and before the first file section:
//! - `===description===` — free text, ignored by the runner.
//! - `===ignore===` — the test is reported as ignored.
//! - `===allow-invalid-php===` — skip the `php -l` check on opened files.
//! - `===config===` — JSON `initializationOptions`, deep-merged over the defaults.
//! - `===request===` — one line, `<method> [args…]`; see [`Request`].
//!
//! File sections:
//! - `===file:path===` — opened with `didOpen`; may hold `$0` markers and
//!   `// ^^^` annotations (the `tests/common/fixture.rs` DSL).
//! - `===file===` — shorthand for `===file:main.php===`.
//! - `===disk:path===` — written to disk only, never opened. Any `disk` section
//!   roots the server at a temp workspace and waits for the index.
//!
//! `===edit:path===` sections replace an opened file's text with a full-text
//! `didChange`; a `$0` in the new text moves the cursor.
//!
//! Expectations come in two modes:
//! - **Annotations** (no `===expect===`): `// ^^^ <tag>` comments in the file and
//!   edit bodies are the expectation, checked after opening and again once all
//!   edits are applied, against each file's latest text; restate an unedited file
//!   in an edit section to change its annotations. Without `===request===` they are diagnostics
//!   (`// ^^^ error: message`); for location requests they are the result spans,
//!   tagged `def`/`ref` (references), `def` (definition), `decl` (declaration),
//!   `type` (type_definition), `impl` (implementation), `rename` (rename), or
//!   `read`/`write` (document_highlight). `//<^^^` anchors carets at column 0.
//! - **Rendered** (`===expect===`): one after the file sections, then one closing
//!   each group of edits. Set `UPDATE_EXPECT=1` to rewrite them from actual output.

#![allow(dead_code, unused_imports)]

#[path = "common/mod.rs"]
mod common;

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use common::fixture::{self, Cursor, Fixture};
use common::render::{
    assert_highlights_match, collect_navigation_annotations, render_code_actions,
    render_document_links, render_folding_ranges, render_prepare_rename,
};
use common::*;
use libtest_mimic::{Arguments, Failed, Trial};
use serde_json::{Value, json};

fn main() {
    let args = Arguments::from_args();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/phpt");
    let mut paths = Vec::new();
    collect_phpt(&root, &mut paths);
    paths.sort();

    let trials = paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(&root)
                .unwrap()
                .with_extension("")
                .to_string_lossy()
                .replace(['/', '\\'], "::");
            let ignored = std::fs::read_to_string(&path)
                .is_ok_and(|c| c.lines().any(|l| l.trim_end() == "===ignore==="));
            Trial::test(name, move || run_file(&path)).with_ignored_flag(ignored)
        })
        .collect();
    libtest_mimic::run(&args, trials).exit();
}

fn collect_phpt(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_phpt(&path, out);
        } else if path.extension().is_some_and(|e| e == "phpt") {
            out.push(path);
        }
    }
}

fn run_file(path: &Path) -> Result<(), Failed> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
    let label = path.display().to_string();
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let phpt = Phpt::parse(&content, &label);
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run(&phpt))
            .map(|actual| (phpt, actual))
    }));
    let (phpt, actual) = match outcome {
        Ok(Ok(v)) => v,
        Ok(Err(msg)) => return Err(msg.into()),
        Err(panic) => return Err(panic_message(&panic).into()),
    };
    check_expectations(path, &content, &phpt, &actual)
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "test panicked".to_owned())
}

// ---------- parsing ----------

#[derive(Debug)]
struct Section {
    kind: SectionKind,
    /// Line index of the first body line (just after the `===…===` marker).
    body_start: usize,
    /// Line index one past the last body line.
    body_end: usize,
    body: String,
}

#[derive(Debug, Clone, PartialEq)]
enum SectionKind {
    Description,
    Ignore,
    AllowInvalidPhp,
    Config,
    Request,
    File(String),
    Disk(String),
    Expect,
    Edit(String),
}

impl SectionKind {
    fn from_marker(line: &str) -> Option<Self> {
        let inner = line.trim_end().strip_prefix("===")?.strip_suffix("===")?;
        let (name, arg) = match inner.split_once(':') {
            Some((n, a)) => (n, Some(a.trim().trim_start_matches('/').to_owned())),
            None => (inner, None),
        };
        Some(match (name, arg) {
            ("description", None) => Self::Description,
            ("ignore", None) => Self::Ignore,
            ("allow-invalid-php", None) => Self::AllowInvalidPhp,
            ("config", None) => Self::Config,
            ("request", None) => Self::Request,
            ("expect", None) => Self::Expect,
            ("file", None) => Self::File("main.php".to_owned()),
            ("file", Some(p)) => Self::File(p),
            ("disk", Some(p)) => Self::Disk(p),
            ("edit", Some(p)) => Self::Edit(p),
            _ => return None,
        })
    }

    fn is_header(&self) -> bool {
        matches!(
            self,
            Self::Description | Self::Ignore | Self::AllowInvalidPhp | Self::Config | Self::Request
        )
    }
}

struct Phpt {
    sections: Vec<Section>,
    config: Value,
    lint_php: bool,
    request: Option<Request>,
    /// `(path, text)` of every `===file:…===` section, in order.
    open_files: Vec<(String, String)>,
    disk_files: Vec<(String, String)>,
    /// One entry per edit group: the files it replaces.
    edit_groups: Vec<Vec<(String, String)>>,
    /// Indices into `sections` of each `===expect===`, in order.
    expect_sections: Vec<usize>,
}

impl Phpt {
    fn parse(content: &str, label: &str) -> Self {
        let lines: Vec<&str> = content.split_inclusive('\n').collect();
        let mut sections: Vec<Section> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            match SectionKind::from_marker(line) {
                Some(kind) => {
                    if let Some(prev) = sections.last_mut() {
                        prev.body_end = i;
                    }
                    sections.push(Section {
                        kind,
                        body_start: i + 1,
                        body_end: lines.len(),
                        body: String::new(),
                    });
                }
                None if line.trim_start().starts_with("===")
                    && line.trim_end().ends_with("===") =>
                {
                    panic!(
                        "{label}:{}: unknown section marker {}",
                        i + 1,
                        line.trim_end()
                    )
                }
                None if sections.is_empty() && !line.trim().is_empty() => {
                    panic!("{label}:{}: text before the first section", i + 1)
                }
                None => {}
            }
        }
        for s in &mut sections {
            s.body = lines[s.body_start..s.body_end].concat();
        }

        let first_file = sections
            .iter()
            .position(|s| matches!(s.kind, SectionKind::File(_) | SectionKind::Disk(_)))
            .unwrap_or_else(|| panic!("{label}: no ===file:…=== or ===disk:…=== section"));
        for s in &sections[..first_file] {
            assert!(
                s.kind.is_header(),
                "{label}: {:?} must come after the file sections",
                s.kind
            );
        }
        for kind in [
            SectionKind::Description,
            SectionKind::Ignore,
            SectionKind::AllowInvalidPhp,
            SectionKind::Config,
            SectionKind::Request,
        ] {
            let n = sections.iter().filter(|s| s.kind == kind).count();
            assert!(n <= 1, "{label}: {kind:?} appears {n} times");
        }

        let header = |kind: SectionKind| {
            sections[..first_file]
                .iter()
                .find(|s| s.kind == kind)
                .map(|s| s.body.trim().to_owned())
        };
        let config = header(SectionKind::Config)
            .map(|c| {
                serde_json::from_str::<Value>(&c)
                    .unwrap_or_else(|e| panic!("{label}: ===config=== is not JSON: {e}"))
            })
            .unwrap_or(json!({}));
        let request = header(SectionKind::Request).map(|r| Request::parse(&r, label));

        let mut open_files = Vec::new();
        let mut disk_files = Vec::new();
        let mut edit_groups: Vec<Vec<(String, String)>> = Vec::new();
        let mut expect_sections = Vec::new();
        let annotation_mode = !sections.iter().any(|s| s.kind == SectionKind::Expect);
        let mut in_edit_group = false;
        for (i, s) in sections.iter().enumerate().skip(first_file) {
            match &s.kind {
                SectionKind::File(p) | SectionKind::Disk(p) => {
                    assert!(
                        expect_sections.is_empty(),
                        "{label}: file section {p} after ===expect===; use ===edit:{p}==="
                    );
                    let entry = (p.clone(), s.body.clone());
                    if matches!(s.kind, SectionKind::File(_)) {
                        open_files.push(entry);
                    } else {
                        disk_files.push(entry);
                    }
                }
                SectionKind::Expect => {
                    assert!(
                        expect_sections.len() == edit_groups.len(),
                        "{label}: two ===expect=== sections with no ===edit:…=== between them"
                    );
                    expect_sections.push(i);
                    in_edit_group = false;
                }
                SectionKind::Edit(p) => {
                    assert!(
                        annotation_mode || !expect_sections.is_empty(),
                        "{label}: ===edit:{p}=== before the first ===expect==="
                    );
                    assert!(
                        open_files.iter().any(|(f, _)| f == p),
                        "{label}: ===edit:{p}=== names no ===file:{p}=== section"
                    );
                    if !in_edit_group {
                        edit_groups.push(Vec::new());
                        in_edit_group = true;
                    }
                    edit_groups
                        .last_mut()
                        .unwrap()
                        .push((p.clone(), s.body.clone()));
                }
                other => panic!("{label}: {other:?} must come before the file sections"),
            }
        }

        if annotation_mode {
            assert!(
                request.as_ref().is_none_or(Request::has_annotation_form),
                "{label}: {request:?} has no annotation form; add an ===expect=== section"
            );
        } else {
            assert!(
                request.is_some(),
                "{label}: ===expect=== needs a ===request===; diagnostics without one use `// ^^^` annotations"
            );
            assert!(
                expect_sections.len() == edit_groups.len() + 1,
                "{label}: needs one ===expect=== after the files and one after each edit group"
            );
        }

        let lint_php = !sections[..first_file]
            .iter()
            .any(|s| s.kind == SectionKind::AllowInvalidPhp);

        Phpt {
            sections,
            config,
            lint_php,
            request,
            open_files,
            disk_files,
            edit_groups,
            expect_sections,
        }
    }
}

/// The `===request===` line.
#[derive(Debug)]
enum Request {
    Hover,
    Definition,
    Declaration,
    TypeDefinition,
    Implementation,
    References {
        include_declaration: bool,
    },
    Completion {
        ordered: bool,
    },
    SignatureHelp,
    DocumentHighlight,
    DocumentSymbols,
    WorkspaceSymbols(String),
    InlayHints,
    DocumentLinks,
    FoldingRange,
    CodeActions,
    Rename(String),
    PrepareRename,
    /// Pull diagnostics for every opened file.
    Diagnostics,
}

impl Request {
    fn parse(line: &str, label: &str) -> Self {
        let mut words = line.split_whitespace();
        let method = words
            .next()
            .unwrap_or_else(|| panic!("{label}: empty ===request==="));
        let args: Vec<&str> = words.collect();
        let flag = |name: &str| args.contains(&name);
        let one_arg = || match args.as_slice() {
            [a] => a.to_string(),
            _ => panic!("{label}: `{method}` takes exactly one argument"),
        };
        match method {
            "hover" => Self::Hover,
            "definition" => Self::Definition,
            "declaration" => Self::Declaration,
            "type_definition" => Self::TypeDefinition,
            "implementation" => Self::Implementation,
            "references" => Self::References {
                include_declaration: flag("include_declaration"),
            },
            "completion" => Self::Completion {
                ordered: flag("ordered"),
            },
            "signature_help" => Self::SignatureHelp,
            "document_highlight" => Self::DocumentHighlight,
            "document_symbols" => Self::DocumentSymbols,
            "workspace_symbols" => Self::WorkspaceSymbols(args.join(" ")),
            "inlay_hints" => Self::InlayHints,
            "document_links" => Self::DocumentLinks,
            "folding_range" => Self::FoldingRange,
            "code_actions" => Self::CodeActions,
            "rename" => Self::Rename(one_arg()),
            "prepare_rename" => Self::PrepareRename,
            "diagnostics" => Self::Diagnostics,
            other => panic!("{label}: unknown request `{other}`"),
        }
    }

    fn has_annotation_form(&self) -> bool {
        matches!(
            self,
            Self::Definition
                | Self::Declaration
                | Self::TypeDefinition
                | Self::Implementation
                | Self::References { .. }
                | Self::DocumentHighlight
                | Self::Rename(_)
        )
    }

    fn needs_cursor(&self) -> bool {
        !matches!(
            self,
            Self::DocumentSymbols
                | Self::WorkspaceSymbols(_)
                | Self::InlayHints
                | Self::DocumentLinks
                | Self::FoldingRange
                | Self::Diagnostics
        )
    }
}

// ---------- running ----------

fn default_options() -> Value {
    json!({ "diagnostics": { "enabled": true } })
}

fn merge_json(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                merge_json(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (b, o) => *b = o.clone(),
    }
}

fn dsl_source(files: &[(String, String)]) -> String {
    files
        .iter()
        .map(|(path, text)| format!("//- /{path}\n{text}"))
        .collect()
}

/// Runs the fixture and returns one rendered output per `===expect===`.
async fn run(phpt: &Phpt) -> Result<Vec<String>, String> {
    let mut options = default_options();
    merge_json(&mut options, &phpt.config);

    // Rooted servers need the opened files on disk too, as an editor would have them.
    let workspace = if phpt.disk_files.is_empty() {
        None
    } else {
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let opened = fixture::parse(&dsl_source(&phpt.open_files));
        let on_disk = opened
            .files
            .iter()
            .map(|f| (f.path.clone(), f.text.clone()))
            .chain(phpt.disk_files.iter().cloned());
        for (path, text) in on_disk {
            let full = dir.path().join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&full, text).map_err(|e| e.to_string())?;
        }
        Some(dir)
    };
    let mut server = match &workspace {
        Some(dir) => {
            let mut s = TestServer::with_root_and_options(dir.path(), options).await;
            s.wait_for_index_ready().await;
            s
        }
        None => TestServer::new_with_options(options).await.0,
    };

    if phpt.open_files.is_empty() {
        return Err("no ===file:…=== section to open".to_owned());
    }
    server.validate_syntax(phpt.lint_php);
    let opened = server.open_fixture(&dsl_source(&phpt.open_files)).await;

    let annotation_mode = phpt.expect_sections.is_empty();
    let mut state = Fixture {
        files: opened.fixture.files.clone(),
        cursor: opened.fixture.cursor.clone(),
        range: opened.fixture.range.clone(),
    };
    let mut outputs = Vec::new();
    if annotation_mode {
        match &phpt.request {
            None => {
                for file in &state.files {
                    fixture::assert_diagnostics(
                        opened.diagnostics_for(&file.path),
                        &file.annotations,
                    );
                }
            }
            Some(request) => assert_annotations(&mut server, request, &state).await,
        }
    } else {
        let request = phpt.request.as_ref().unwrap();
        outputs.push(execute(&mut server, request, &state).await);
    }

    for (version, group) in (2..).zip(&phpt.edit_groups) {
        for (path, body) in group {
            let edited = fixture::parse(&format!("//- /{path}\n{body}"));
            if edited.cursor.is_some() {
                state.cursor = edited.cursor.clone();
                state.range = edited.range.clone();
            }
            let new_file = edited.files.into_iter().next().unwrap();
            server.change(path, version, &new_file.text).await;
            let slot = state.files.iter_mut().find(|f| &f.path == path).unwrap();
            *slot = new_file;
        }
        match (&phpt.request, annotation_mode) {
            (None, _) => {
                for file in &state.files {
                    let pulled = server.pull_diagnostics(&file.path).await;
                    let as_push = json!({ "params": { "diagnostics": pulled["result"]["items"] } });
                    fixture::assert_diagnostics(&as_push, &file.annotations);
                }
            }
            (Some(request), true) => assert_annotations(&mut server, request, &state).await,
            (Some(request), false) => outputs.push(execute(&mut server, request, &state).await),
        }
    }
    drop(workspace);
    Ok(outputs)
}

/// Asserts a location-shaped response against the fixture's `// ^^^ <tag>` annotations.
async fn assert_annotations(server: &mut TestServer, request: &Request, fx: &Fixture) {
    let c = fx
        .cursor
        .clone()
        .unwrap_or_else(|| panic!("request {request:?} needs a $0 cursor"));
    let (resp, tags, label): (Value, &[&str], &str) = match request {
        Request::Definition => (
            server.definition(&c.path, c.line, c.character).await,
            &["def"],
            "definition",
        ),
        Request::Declaration => (
            server.declaration(&c.path, c.line, c.character).await,
            &["decl"],
            "declaration",
        ),
        Request::TypeDefinition => (
            server.type_definition(&c.path, c.line, c.character).await,
            &["type"],
            "type_definition",
        ),
        Request::Implementation => (
            server.implementation(&c.path, c.line, c.character).await,
            &["impl"],
            "implementation",
        ),
        Request::References {
            include_declaration,
        } => (
            server
                .references(&c.path, c.line, c.character, *include_declaration)
                .await,
            &["def", "ref"],
            "references",
        ),
        Request::Rename(new_name) => {
            let resp = server.rename(&c.path, c.line, c.character, new_name).await;
            if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
                panic!("rename errored: {err}");
            }
            (
                server.rename_edit_as_locations(&resp),
                &["rename"],
                "rename",
            )
        }
        Request::DocumentHighlight => {
            let resp = server
                .document_highlight(&c.path, c.line, c.character)
                .await;
            let expected = collect_navigation_annotations(fx, &["read", "write", "ref"]);
            assert_highlights_match(&resp, &expected, &c.path, "document_highlight");
            return;
        }
        other => unreachable!("{other:?} has no annotation form"),
    };
    server.assert_annotated_locations(&resp, fx, &c.path, tags, label);
}

async fn execute(server: &mut TestServer, request: &Request, fx: &Fixture) -> String {
    let (files, cursor, range) = (&fx.files, fx.cursor.as_ref(), fx.range.as_ref());
    let root = server.uri("");
    let c = || -> Cursor {
        assert!(
            !request.needs_cursor() || cursor.is_some(),
            "request {request:?} needs a $0 cursor"
        );
        cursor.cloned().unwrap()
    };
    let first = &files[0].path;
    match request {
        Request::Hover => {
            let c = c();
            render_hover(&server.hover(&c.path, c.line, c.character).await)
        }
        Request::Definition => {
            let c = c();
            render_locations(
                &server.definition(&c.path, c.line, c.character).await,
                &root,
            )
        }
        Request::Declaration => {
            let c = c();
            render_locations(
                &server.declaration(&c.path, c.line, c.character).await,
                &root,
            )
        }
        Request::TypeDefinition => {
            let c = c();
            render_locations(
                &server.type_definition(&c.path, c.line, c.character).await,
                &root,
            )
        }
        Request::Implementation => {
            let c = c();
            render_locations(
                &server.implementation(&c.path, c.line, c.character).await,
                &root,
            )
        }
        Request::References {
            include_declaration,
        } => {
            let c = c();
            let resp = server
                .references(&c.path, c.line, c.character, *include_declaration)
                .await;
            render_locations(&resp, &root)
        }
        Request::Completion { ordered } => {
            let c = c();
            let resp = server.completion(&c.path, c.line, c.character).await;
            if *ordered {
                render_completion_ordered(&resp)
            } else {
                render_completion(&resp)
            }
        }
        Request::SignatureHelp => {
            let c = c();
            render_signature_help(&server.signature_help(&c.path, c.line, c.character).await)
        }
        Request::DocumentHighlight => {
            let c = c();
            render_document_highlight(
                &server
                    .document_highlight(&c.path, c.line, c.character)
                    .await,
            )
        }
        Request::DocumentSymbols => render_document_symbols(&server.document_symbols(first).await),
        Request::WorkspaceSymbols(query) => {
            render_workspace_symbols(&server.workspace_symbols(query).await, &root)
        }
        Request::InlayHints => {
            let lines = files[0].text.lines().count() as u32;
            render_inlay_hints(&server.inlay_hints(first, 0, 0, lines + 1, 0).await)
        }
        Request::DocumentLinks => render_document_links(&server.document_link(first).await, &root),
        Request::FoldingRange => render_folding_ranges(&server.folding_range(first).await),
        Request::CodeActions => {
            let resp = match range {
                Some(r) => server.code_action_at(r).await,
                None => {
                    let c = c();
                    server
                        .code_action(&c.path, c.line, c.character, c.line, c.character)
                        .await
                }
            };
            render_code_actions(&resp)
        }
        Request::Rename(new_name) => {
            let c = c();
            let resp = server.rename(&c.path, c.line, c.character, new_name).await;
            match resp.get("error").filter(|e| !e.is_null()) {
                Some(err) => format!("error: {err}"),
                None => canonicalize_workspace_edit(&resp["result"], &root),
            }
        }
        Request::PrepareRename => {
            let c = c();
            render_prepare_rename(&server.prepare_rename(&c.path, c.line, c.character).await)
        }
        Request::Diagnostics => {
            let mut out = Vec::new();
            for f in files {
                let rendered = render_pull_diagnostics(&server.pull_diagnostics(&f.path).await);
                out.extend(rendered.lines().map(|l| format!("{}: {l}", f.path)));
            }
            out.join("\n")
        }
    }
}

// ---------- expectations ----------

fn normalize(s: &str) -> &str {
    s.trim_end_matches(['\n', '\r', ' '])
}

fn check_expectations(
    path: &Path,
    content: &str,
    phpt: &Phpt,
    actual: &[String],
) -> Result<(), Failed> {
    let expected: Vec<&str> = phpt
        .expect_sections
        .iter()
        .map(|&i| normalize(&phpt.sections[i].body))
        .collect();
    let mismatches: Vec<usize> = (0..expected.len())
        .filter(|&i| expected[i] != normalize(&actual[i]))
        .collect();
    if mismatches.is_empty() {
        return Ok(());
    }
    if std::env::var("UPDATE_EXPECT").as_deref() == Ok("1") {
        rewrite_expects(path, content, phpt, actual);
        return Ok(());
    }
    let mut msg = String::new();
    for i in mismatches {
        let step = if i == 0 {
            "before edits".to_owned()
        } else {
            format!("after edit group {i}")
        };
        msg.push_str(&format!(
            "===expect=== #{} ({step}) mismatch\n--- expected\n{}\n+++ actual\n{}\n\n",
            i + 1,
            expected[i],
            normalize(&actual[i]),
        ));
    }
    msg.push_str(&format!(
        "rerun with UPDATE_EXPECT=1 to rewrite {}",
        path.display()
    ));
    Err(msg.into())
}

fn rewrite_expects(path: &Path, content: &str, phpt: &Phpt, actual: &[String]) {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut cursor = 0;
    for (n, &i) in phpt.expect_sections.iter().enumerate() {
        let s = &phpt.sections[i];
        out.push_str(&lines[cursor..s.body_start].concat());
        let body = normalize(&actual[n]);
        if !body.is_empty() {
            out.push_str(body);
            out.push('\n');
        }
        cursor = s.body_end;
    }
    out.push_str(&lines[cursor..].concat());
    std::fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}
