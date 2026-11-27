//! php-lsp's completion, hover and signature help for a browser worker: a WASI *reactor* (no
//! `_start`) holding one `DocumentStore`, asked about one buffer at a time.
//!
//! No server, no threads, no files: the library builds for wasm32-wasip1 without `backend`, and this
//! calls `completion::filtered_completions_at`, `hover::hover_info_with_lazy_maps` and
//! `editing::signature_help` over a store the host fills with the open files.
//! PHP's built-in classes come from the phpstorm-stubs inside `mir-analyzer`, so nothing is loaded.
//!
//! ABI, all pointers into this module's linear memory:
//!
//! - `hx_alloc(len) -> ptr`, `hx_free(ptr, len)` — buffers the host fills and reads;
//! - `hx_complete(ptr, len) -> ptr` — a JSON request
//!   `{"files": {path: source}, "path", "line", "character", "trigger"?}`; the answer is
//!   `[u32 LE length][JSON array of LSP CompletionItem]` at the returned pointer, freed with
//!   `hx_free(ptr, 4 + length)`;
//! - `hx_hover(ptr, len) -> ptr` and `hx_signature_help(ptr, len) -> ptr` — the same request
//!   (`trigger` is ignored) and the same framing, answering an LSP `Hover` or `SignatureHelp`, or
//!   `null`.
//!
//! `files` is every open file, so a class from another file is known; a file whose text did not change
//! since the last request is not parsed again. `line` and `character` are the LSP position (UTF-16),
//! `trigger` the character that opened the list (`>` after `->`, `:` after `::`, `$`).
//!
//! A panic aborts the instance on this target, so the host re-instantiates after a trap.
//!
//! The crate is empty off wasm: the library keeps these modules private on native builds.
#![cfg(target_family = "wasm")]
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use mir_analyzer::FileAnalysis;
use php_lsp::hover::hover_info_with_lazy_maps;
use php_lsp::{CompletionCtx, DocumentStore, ParsedDoc, filtered_completions_at};
use serde::{Deserialize, Serialize};
use tower_lsp_server::ls_types::{CompletionItem, Hover, Position, SignatureHelp, Uri};

struct Engine {
    docs: Arc<DocumentStore>,
    /// What each file said the last time it was sent, to skip the ones that did not change
    texts: HashMap<String, String>,
}

thread_local! {
    static ENGINE: RefCell<Engine> = RefCell::new(Engine {
        docs: Arc::new(DocumentStore::new()),
        texts: HashMap::new(),
    });
}

#[derive(Deserialize)]
struct Request {
    files: HashMap<String, String>,
    path: String,
    line: u32,
    character: u32,
    trigger: Option<String>,
}

fn uri_of(path: &str) -> Result<Uri, String> {
    Uri::from_file_path(Path::new(path)).ok_or_else(|| format!("not an absolute path: {path}"))
}

/// Brings the store in line with the files of the request and asks `f` about the requested one.
fn with_request<R>(request: &[u8], f: impl FnOnce(&Request, Query<'_>) -> R) -> Result<R, String> {
    let request: Request = serde_json::from_slice(request).map_err(|err| err.to_string())?;

    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();

        for (path, text) in &request.files {
            if engine.texts.get(path) != Some(text) {
                engine.docs.ingest(uri_of(path)?, text);
                engine.texts.insert(path.clone(), text.clone());
            }
        }

        // A file that left the list is emptied, not removed: `DocumentStore::remove` followed by an
        // ingest of the same path leaves the analyzer without the file, and an empty file takes the
        // classes it declared with it just the same.
        let closed: Vec<String> = engine
            .texts
            .iter()
            .filter(|(path, text)| !request.files.contains_key(*path) && !text.is_empty())
            .map(|(path, _)| path.clone())
            .collect();

        for path in closed {
            engine.docs.ingest(uri_of(&path)?, "");
            engine.texts.insert(path, String::new());
        }

        let uri = uri_of(&request.path)?;
        let doc = engine
            .docs
            .get_doc_salsa(&uri)
            .ok_or("the file is not among `files`")?;
        let others = request
            .files
            .keys()
            .filter(|path| **path != request.path)
            .map(|path| uri_of(path))
            .collect::<Result<Vec<_>, _>>()?;
        let analysis = engine.docs.cached_analysis_cancellable(&uri, &|| false);
        let query = Query {
            docs: &engine.docs,
            uri: &uri,
            doc: &doc,
            others: &others,
            analysis: analysis.as_deref(),
            source: &request.files[&request.path],
            position: Position::new(request.line, request.character),
        };

        Ok(f(&request, query))
    })
}

/// The requested file and what the analyzer knows about it
struct Query<'a> {
    docs: &'a Arc<DocumentStore>,
    uri: &'a Uri,
    doc: &'a Arc<ParsedDoc>,
    others: &'a [Uri],
    analysis: Option<&'a FileAnalysis>,
    source: &'a str,
    position: Position,
}

fn complete(request: &[u8]) -> Result<Vec<CompletionItem>, String> {
    with_request(request, |request, query| {
        let other_docs: Vec<_> = query
            .others
            .iter()
            .filter_map(|uri| query.docs.get_doc_salsa(uri))
            .collect();
        let imports = query.doc.file_imports();
        let ctx = CompletionCtx {
            source: Some(query.source),
            position: Some(query.position),
            doc_uri: Some(query.uri),
            file_imports: Some(&imports),
            analysis: query.analysis,
            session: Some(query.docs),
            ..Default::default()
        };

        filtered_completions_at(query.doc, &other_docs, request.trigger.as_deref(), &ctx)
    })
}

fn hover(request: &[u8]) -> Result<Option<Hover>, String> {
    with_request(request, |_, query| {
        let other_docs = query.docs.other_docs(query.uri, query.others);

        hover_info_with_lazy_maps(
            query.source,
            query.doc,
            query.analysis,
            query.position,
            &other_docs,
            || query.docs.other_symbol_maps(query.uri, query.others),
            Some(query.docs),
            None,
        )
    })
}

fn signature_help(request: &[u8]) -> Result<Option<SignatureHelp>, String> {
    with_request(request, |_, query| {
        query
            .docs
            .read_snapshot(|snap| {
                php_lsp::editing::signature_help::signature_help(
                    query.source,
                    query.doc,
                    query.position,
                    query.analysis,
                    Some(snap.db()),
                )
            })
            .ok()
            .flatten()
    })
}

/// `[u32 LE length][json]`, handed over to the host
fn framed(json: Vec<u8>) -> *mut u8 {
    let mut out = Vec::with_capacity(4 + json.len());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    Box::into_raw(out.into_boxed_slice()).cast()
}

/// An answer that is `null` when the engine has nothing or the request is broken
fn optional<T: Serialize>(answer: Result<Option<T>, String>) -> *mut u8 {
    framed(
        answer
            .ok()
            .flatten()
            .and_then(|value| serde_json::to_vec(&value).ok())
            .unwrap_or_else(|| b"null".to_vec()),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn hx_alloc(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len].into_boxed_slice()).cast()
}

/// # Safety
/// `ptr` and `len` come from `hx_alloc(len)` (or the pointer `hx_complete` returned, with its full
/// `4 + length`) and are freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hx_free(ptr: *mut u8, len: usize) {
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) });
}

/// # Safety
/// `ptr` points at `len` initialised bytes from `hx_alloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hx_complete(ptr: *const u8, len: usize) -> *mut u8 {
    framed(
        match complete(unsafe { std::slice::from_raw_parts(ptr, len) }) {
            Ok(items) => serde_json::to_vec(&items).unwrap_or_else(|_| b"[]".to_vec()),
            Err(_) => b"[]".to_vec(),
        },
    )
}

/// # Safety
/// `ptr` points at `len` initialised bytes from `hx_alloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hx_hover(ptr: *const u8, len: usize) -> *mut u8 {
    optional(hover(unsafe { std::slice::from_raw_parts(ptr, len) }))
}

/// # Safety
/// `ptr` points at `len` initialised bytes from `hx_alloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hx_signature_help(ptr: *const u8, len: usize) -> *mut u8 {
    optional(signature_help(unsafe {
        std::slice::from_raw_parts(ptr, len)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(
        files: &[(&str, &str)],
        path: &str,
        line: u32,
        character: u32,
        trigger: &str,
    ) -> Vec<String> {
        let request = serde_json::json!({
            "files": files.iter().cloned().collect::<HashMap<_, _>>(),
            "path": path,
            "line": line,
            "character": character,
            "trigger": trigger,
        });
        let mut labels: Vec<String> = complete(request.to_string().as_bytes())
            .expect("answer")
            .into_iter()
            .map(|item| item.label)
            .collect();
        labels.sort();

        labels
    }

    // The tests run under a WASI runtime (`cargo test --target wasm32-wasip1`), one after another on
    // one thread, so they share the engine: each request carries its whole file set, as the host's do.
    #[test]
    fn completes_members_of_own_classes_other_files_and_built_ins() {
        let account = "<?php\nclass Account {\n    public function deposit(int $amount): void {}\n    public function report(): string { return ''; }\n}\n";
        let main = "<?php\n$a = new Account();\n$a->\n";

        // The class is declared in another file
        assert_eq!(
            ask(
                &[("/app/Account.php", account), ("/app/main.php", main)],
                "/app/main.php",
                2,
                4,
                ">"
            ),
            vec!["deposit", "report"]
        );

        // A built-in class: its members come from phpstorm-stubs
        let date = "<?php\n$d = new DateTime();\n$d->\n";
        let labels = ask(&[("/app/date.php", date)], "/app/date.php", 2, 4, ">");
        assert!(labels.iter().any(|label| label == "format"), "{labels:?}");

        // A file that left the list is forgotten, so its class is gone with it
        assert!(ask(&[("/app/main.php", main)], "/app/main.php", 2, 4, ">").is_empty());
    }

    #[test]
    fn a_file_that_came_back_is_known_again() {
        let account =
            "<?php\nclass Account {\n    public function deposit(int $amount): void {}\n}\n";
        let main = "<?php\n$a = new Account();\n$a->\n";
        let both = [("/app/Account.php", account), ("/app/main.php", main)];

        assert_eq!(ask(&both, "/app/main.php", 2, 4, ">"), vec!["deposit"]);
        // Another file only: both are forgotten
        assert!(
            ask(
                &[("/app/other.php", "<?php\n$x = 1;\n")],
                "/app/other.php",
                1,
                0,
                ">"
            )
            .is_empty()
        );
        // And back
        assert_eq!(ask(&both, "/app/main.php", 2, 4, ">"), vec!["deposit"]);
    }

    #[test]
    fn static_members_of_a_class_in_another_file() {
        let account = "<?php\nclass Account {\n    const MAX = 1;\n    public static function open(): Account { return new Account(); }\n}\n";
        let main = "<?php\nAccount::\n";

        assert_eq!(
            ask(
                &[("/app/Account.php", account), ("/app/main.php", main)],
                "/app/main.php",
                1,
                9,
                ":"
            ),
            vec!["MAX", "open"]
        );
    }

    fn position_request(files: &[(&str, &str)], path: &str, line: u32, character: u32) -> Vec<u8> {
        serde_json::json!({
            "files": files.iter().cloned().collect::<HashMap<_, _>>(),
            "path": path,
            "line": line,
            "character": character,
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn hovers_a_method_of_a_class_in_another_file() {
        let account = "<?php\nclass Account {\n    /** Adds money to the account. */\n    public function deposit(int $amount): void {}\n}\n";
        let main = "<?php\n$a = new Account();\n$a->deposit(1);\n";
        let request = position_request(
            &[("/app/Account.php", account), ("/app/main.php", main)],
            "/app/main.php",
            2,
            6,
        );
        let hover =
            serde_json::to_string(&hover(&request).expect("answer").expect("hover")).expect("json");

        assert!(hover.contains("deposit(int $amount)"), "{hover}");
        assert!(hover.contains("Adds money"), "{hover}");
    }

    #[test]
    fn helps_with_the_arguments_of_a_built_in_function() {
        let main = "<?php\nstr_replace('a', \n";
        let request = position_request(&[("/app/main.php", main)], "/app/main.php", 1, 17);
        let help = signature_help(&request)
            .expect("answer")
            .expect("signature help");

        assert!(help.signatures[0].label.contains("str_replace"), "{help:?}");
        assert_eq!(
            help.active_parameter
                .or(help.signatures[0].active_parameter),
            Some(1)
        );
    }
}
