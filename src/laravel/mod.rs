//! Laravel framework support.
//!
//! Beyond generic PHP analysis, Laravel projects lean heavily on stringly-typed
//! lookups — `env('KEY')`, `config('a.b.c')`, `view('a.b')`, `route('name')`,
//! `trans('a.b')`, `asset('a/b')`, `->middleware('alias')` — that resolve
//! against files elsewhere in the project rather than through normal symbol
//! resolution. `LaravelIndex` builds a workspace-scan-time index for each of
//! these domains and wires them into go-to-definition
//! ([`resolve_string_key`]), completion ([`completions_for_string_key`]),
//! hover ([`hover_for_string_key`]) and document links ([`document_links`]).
//!
//! Gated behind [`LaravelIndex::load`]'s project-detection check, so
//! non-Laravel workspaces pay no cost beyond the one-time
//! `artisan`/`composer.json` probe: every dispatch function bails out on the
//! `is_laravel` flag before doing any string scanning.

mod asset_index;
mod config_index;
mod detect;
mod eloquent_guard;
mod env_index;
pub(crate) mod facades;
mod hover;
mod location_lookup;
mod middleware_index;
pub(crate) mod request_fields;
mod route_index;
pub(crate) mod route_scaffold;
mod string_call;
mod translation_index;
mod view_index;

pub use asset_index::AssetIndex;
pub use config_index::ConfigIndex;
pub use eloquent_guard::unguarded_model_diagnostics;
pub use env_index::EnvIndex;
pub use middleware_index::MiddlewareIndex;
pub use route_index::RouteIndex;
pub use translation_index::TranslationIndex;
pub use view_index::ViewIndex;

use asset_index::asset_completions;
use config_index::config_completions;
use env_index::{env_completions, missing_env_key_action};
use middleware_index::{
    collect_middleware_calls, middleware_alias_at, middleware_completions, middleware_string_prefix,
};
use route_index::route_completions;
use string_call::{call_string_arg, call_string_prefix};
use translation_index::{missing_translation_json_key_action, translation_completions};
use view_index::view_completions;

use std::path::Path;

use tower_lsp_server::ls_types::{
    CodeActionOrCommand, CompletionItem, DocumentLink, Hover, Location, Position, Range, Uri,
};

pub(crate) use string_call::find_call_sites;

/// Bare function names recognized as the `env()` string-key helper call.
const ENV_CALL_NAMES: &[&str] = &["env"];
/// Bare function names recognized as the `config()` string-key helper call.
const CONFIG_CALL_NAMES: &[&str] = &["config"];
/// Bare function names recognized as the `view()` string-key helper call.
const VIEW_CALL_NAMES: &[&str] = &["view"];
/// Bare function names recognized as translation string-key helper calls —
/// `__()` and its `trans()` alias.
const TRANS_CALL_NAMES: &[&str] = &["__", "trans"];
/// Bare function names recognized as the `route()` string-key helper call.
const ROUTE_CALL_NAMES: &[&str] = &["route"];
/// Bare function names recognized as the `asset()` string-key helper call.
const ASSET_CALL_NAMES: &[&str] = &["asset"];

#[derive(Debug, Default)]
pub struct LaravelIndex {
    pub is_laravel: bool,
    pub env: EnvIndex,
    pub config: ConfigIndex,
    pub views: ViewIndex,
    pub translations: TranslationIndex,
    pub routes: RouteIndex,
    pub assets: AssetIndex,
    pub middleware: MiddlewareIndex,
}

impl LaravelIndex {
    /// Build the index for a workspace root. Returns an empty, inert index
    /// (no filesystem access beyond the detection probe) for non-Laravel
    /// roots.
    pub fn load(root: &Path) -> Self {
        if !detect::is_laravel_project(root) {
            return Self::default();
        }
        Self {
            is_laravel: true,
            env: EnvIndex::load(root),
            config: ConfigIndex::load(root),
            views: ViewIndex::load(root),
            translations: TranslationIndex::load(root),
            routes: RouteIndex::load(root),
            assets: AssetIndex::load(root),
            middleware: MiddlewareIndex::load(root),
        }
    }
}

/// The route name and its `Range`, when the cursor sits inside a
/// `route('...')` call's string argument. Thin wrapper exposing
/// `string_call::call_string_arg` scoped to [`ROUTE_CALL_NAMES`] for the
/// "Create route" quickfix (`src/actions/route_scaffold_action.rs`), which
/// lives outside this module.
pub(crate) fn route_call_at(
    doc: &crate::document::ast::ParsedDoc,
    position: Position,
) -> Option<(String, Range)> {
    call_string_arg(doc, position, ROUTE_CALL_NAMES)
}

/// Resolve the cursor position to a Laravel string-key definition — checked
/// in order: `env('KEY')`, `config('a.b.c')`, `view('a.b.c')`,
/// `__('a.b')`/`trans('a.b')`, then `route('name')`. Returns `None`
/// immediately for non-Laravel workspaces, or when the cursor isn't inside a
/// recognized call's string argument.
pub(crate) fn resolve_string_key(
    doc: &crate::document::ast::ParsedDoc,
    position: Position,
    laravel: &LaravelIndex,
) -> Option<Location> {
    if !laravel.is_laravel {
        return None;
    }
    if let Some((key, _)) = call_string_arg(doc, position, ENV_CALL_NAMES) {
        return laravel.env.get(&key).cloned();
    }
    if let Some((key, _)) = call_string_arg(doc, position, CONFIG_CALL_NAMES) {
        return laravel.config.get(&key).cloned();
    }
    if let Some((key, _)) = call_string_arg(doc, position, VIEW_CALL_NAMES) {
        return laravel.views.get(&key).cloned();
    }
    if let Some((key, _)) = call_string_arg(doc, position, TRANS_CALL_NAMES) {
        return laravel.translations.get(&key).cloned();
    }
    if let Some((key, _)) = call_string_arg(doc, position, ROUTE_CALL_NAMES) {
        return laravel.routes.get(&key).cloned();
    }
    if let Some((path, _)) = call_string_arg(doc, position, ASSET_CALL_NAMES) {
        return laravel.assets.get(&path).cloned();
    }
    if let Some((alias, _)) = middleware_alias_at(doc, position) {
        return laravel.middleware.get(&alias).cloned();
    }
    None
}

/// Completions for the cursor position inside a recognized Laravel string-key
/// call. `Some(items)` (possibly empty) means the cursor is inside such a
/// call and normal completion should be skipped in favor of these items;
/// `None` means the cursor isn't in a recognized context at all — the caller
/// should fall through to its normal completion logic.
pub(crate) fn completions_for_string_key(
    source: &str,
    position: Position,
    laravel: Option<&LaravelIndex>,
) -> Option<Vec<CompletionItem>> {
    let laravel = laravel.filter(|l| l.is_laravel)?;
    if let Some(prefix) = call_string_prefix(source, position, ENV_CALL_NAMES) {
        return Some(env_completions(&laravel.env, &prefix));
    }
    if let Some(prefix) = call_string_prefix(source, position, CONFIG_CALL_NAMES) {
        return Some(config_completions(&laravel.config, &prefix));
    }
    if let Some(prefix) = call_string_prefix(source, position, VIEW_CALL_NAMES) {
        return Some(view_completions(&laravel.views, &prefix));
    }
    if let Some(prefix) = call_string_prefix(source, position, TRANS_CALL_NAMES) {
        return Some(translation_completions(&laravel.translations, &prefix));
    }
    if let Some(prefix) = call_string_prefix(source, position, ROUTE_CALL_NAMES) {
        return Some(route_completions(&laravel.routes, &prefix));
    }
    if let Some(prefix) = call_string_prefix(source, position, ASSET_CALL_NAMES) {
        return Some(asset_completions(&laravel.assets, &prefix));
    }
    if let Some(prefix) = middleware_string_prefix(source, position) {
        return Some(middleware_completions(&laravel.middleware, &prefix));
    }
    None
}

/// Hover for the cursor position inside a recognized Laravel string-key
/// call — checked at the same call sites as [`resolve_string_key`], covering
/// every domain that resolves to a real location. `root` (the workspace
/// root) is used only to shorten the file path shown in the hover; a
/// missing root just falls back to the full path.
pub(crate) fn hover_for_string_key(
    doc: &crate::document::ast::ParsedDoc,
    position: Position,
    laravel: &LaravelIndex,
    root: Option<&Path>,
) -> Option<Hover> {
    if !laravel.is_laravel {
        return None;
    }
    if let Some((key, _)) = call_string_arg(doc, position, ENV_CALL_NAMES) {
        let loc = laravel.env.get(&key)?;
        return Some(hover::key_hover(
            root,
            loc,
            &format!("env('{key}')"),
            "properties",
            true,
        ));
    }
    if let Some((key, _)) = call_string_arg(doc, position, CONFIG_CALL_NAMES) {
        let loc = laravel.config.get(&key)?;
        return Some(hover::key_hover(
            root,
            loc,
            &format!("config('{key}')"),
            "php",
            true,
        ));
    }
    if let Some((key, _)) = call_string_arg(doc, position, VIEW_CALL_NAMES) {
        let loc = laravel.views.get(&key)?;
        return Some(hover::key_hover(
            root,
            loc,
            &format!("view('{key}')"),
            "php",
            false,
        ));
    }
    if let Some((key, _)) = call_string_arg(doc, position, TRANS_CALL_NAMES) {
        let loc = laravel.translations.get(&key)?;
        let lang = if loc.uri.as_str().ends_with(".json") {
            "json"
        } else {
            "php"
        };
        return Some(hover::key_hover(
            root,
            loc,
            &format!("trans('{key}')"),
            lang,
            true,
        ));
    }
    if let Some((key, _)) = call_string_arg(doc, position, ROUTE_CALL_NAMES) {
        let loc = laravel.routes.get(&key)?;
        return Some(hover::key_hover(
            root,
            loc,
            &format!("route('{key}')"),
            "php",
            true,
        ));
    }
    if let Some((path, _)) = call_string_arg(doc, position, ASSET_CALL_NAMES) {
        let loc = laravel.assets.get(&path)?;
        return Some(hover::key_hover(
            root,
            loc,
            &format!("asset('{path}')"),
            "php",
            false,
        ));
    }
    if let Some((alias, _)) = middleware_alias_at(doc, position) {
        let loc = laravel.middleware.get(&alias)?;
        return Some(hover::key_hover(
            root,
            loc,
            &format!("middleware('{alias}')"),
            "php",
            true,
        ));
    }
    None
}

/// Document links for every recognized Laravel string-key call site in
/// `doc` — one AST walk per domain, each entry resolved against the
/// workspace-wide index built at `LaravelIndex::load` time. Complements
/// go-to-definition (`resolve_string_key`) with the same targets surfaced as
/// clickable underlines, matching how editors normally source
/// `textDocument/documentLink`.
pub(crate) fn document_links(
    doc: &crate::document::ast::ParsedDoc,
    laravel: &LaravelIndex,
) -> Vec<DocumentLink> {
    if !laravel.is_laravel {
        return Vec::new();
    }
    let mut out = Vec::new();
    push_links(&mut out, doc, ENV_CALL_NAMES, |k| laravel.env.get(k));
    push_links(&mut out, doc, CONFIG_CALL_NAMES, |k| laravel.config.get(k));
    push_links(&mut out, doc, VIEW_CALL_NAMES, |k| laravel.views.get(k));
    push_links(&mut out, doc, TRANS_CALL_NAMES, |k| {
        laravel.translations.get(k)
    });
    push_links(&mut out, doc, ROUTE_CALL_NAMES, |k| laravel.routes.get(k));
    push_links(&mut out, doc, ASSET_CALL_NAMES, |k| laravel.assets.get(k));
    for (alias, range) in collect_middleware_calls(doc) {
        if let Some(loc) = laravel.middleware.get(&alias) {
            out.push(DocumentLink {
                range,
                target: Some(loc.uri.clone()),
                tooltip: Some(format!("middleware: {alias}")),
                data: None,
            });
        }
    }
    out
}

/// Sweeps `doc` for every bare call in `names`, resolving each string-literal
/// argument through `lookup` and pushing a matching [`DocumentLink`] —
/// shared body for every bare-call domain in [`document_links`].
fn push_links<'a>(
    out: &mut Vec<DocumentLink>,
    doc: &crate::document::ast::ParsedDoc,
    names: &[&str],
    lookup: impl Fn(&str) -> Option<&'a Location>,
) {
    for (content, range) in string_call::find_all_calls(doc, names) {
        if let Some(loc) = lookup(&content) {
            out.push(DocumentLink {
                range,
                target: Some(loc.uri.clone()),
                tooltip: Some(content),
                data: None,
            });
        }
    }
}

/// If the cursor sits on a Laravel string-key *definition* site — a `.env`
/// entry, a `config/*.php` array key, a view template's start, a
/// translation key, or a route's `->name(...)` string — returns the call
/// names to sweep the rest of the workspace for, the resolved key, and this
/// definition's own `Location` (for `include_declaration`). `None` for
/// non-Laravel workspaces or when the cursor isn't on any definition site.
///
/// Used by find-references: go-to-definition/completion start from a call
/// site and look up a key; this is the reverse direction, starting from the
/// definition and needing to know *which* key it is before a workspace
/// sweep is possible.
pub(crate) fn resolve_definition_key(
    uri: &Uri,
    position: Position,
    laravel: &LaravelIndex,
) -> Option<(&'static [&'static str], String, Location)> {
    if !laravel.is_laravel {
        return None;
    }
    if let Some(key) = laravel.env.key_at(uri, position) {
        let loc = laravel.env.get(key)?.clone();
        return Some((ENV_CALL_NAMES, key.to_string(), loc));
    }
    if let Some(key) = laravel.config.key_at(uri, position) {
        let loc = laravel.config.get(key)?.clone();
        return Some((CONFIG_CALL_NAMES, key.to_string(), loc));
    }
    if let Some(key) = laravel.views.key_at(uri, position) {
        let loc = laravel.views.get(key)?.clone();
        return Some((VIEW_CALL_NAMES, key.to_string(), loc));
    }
    if let Some(key) = laravel.translations.key_at(uri, position) {
        let loc = laravel.translations.get(key)?.clone();
        return Some((TRANS_CALL_NAMES, key.to_string(), loc));
    }
    if let Some(key) = laravel.routes.key_at(uri, position) {
        let loc = laravel.routes.get(key)?.clone();
        return Some((ROUTE_CALL_NAMES, key.to_string(), loc));
    }
    if let Some(key) = laravel.assets.key_at(uri, position) {
        let loc = laravel.assets.get(key)?.clone();
        return Some((ASSET_CALL_NAMES, key.to_string(), loc));
    }
    // Middleware aliases are deliberately not wired into find-references
    // here: usages are method/static calls (`->middleware(...)`), not bare
    // calls, so they don't fit `find_call_sites`'s `names`-based sweep.
    // `document_links` below covers the same alias usages via
    // `middleware_index::collect_middleware_calls` instead.
    None
}

/// Quickfixes for a Laravel string-key call whose argument doesn't resolve
/// to anything in the workspace — checked at the same call sites as
/// [`resolve_string_key`], but only for domains where a safe, mechanical fix
/// exists (`env`, JSON-literal `trans`/`__`). `root` is the workspace root
/// used to locate the file to edit; `None` (no workspace root) or a
/// non-Laravel workspace yields no actions.
pub(crate) fn missing_key_actions(
    doc: &crate::document::ast::ParsedDoc,
    position: Position,
    laravel: &LaravelIndex,
    root: Option<&Path>,
) -> Vec<CodeActionOrCommand> {
    if !laravel.is_laravel {
        return Vec::new();
    }
    let Some(root) = root else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    if let Some((key, _)) = call_string_arg(doc, position, ENV_CALL_NAMES)
        && laravel.env.get(&key).is_none()
        && let Some(action) = missing_env_key_action(root, &key)
    {
        actions.push(action);
    }
    if let Some((key, _)) = call_string_arg(doc, position, TRANS_CALL_NAMES)
        && laravel.translations.get(&key).is_none()
        && let Some(action) = missing_translation_json_key_action(root, &key)
    {
        actions.push(action);
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_non_laravel_root_is_empty_and_inert() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = LaravelIndex::load(tmp.path());
        assert!(!idx.is_laravel);
        assert_eq!(idx.env.names().count(), 0);
        assert_eq!(idx.config.keys().count(), 0);
        assert_eq!(idx.views.names().count(), 0);
        assert_eq!(idx.translations.keys().count(), 0);
        assert_eq!(idx.routes.names().count(), 0);
        assert_eq!(idx.assets.names().count(), 0);
        assert_eq!(idx.middleware.names().count(), 0);
    }

    #[test]
    fn load_laravel_root_builds_every_domain_index() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(tmp.path().join(".env"), "APP_NAME=Test\n").unwrap();
        std::fs::create_dir_all(tmp.path().join("config")).unwrap();
        std::fs::write(
            tmp.path().join("config").join("app.php"),
            "<?php\nreturn ['name' => 'Test'];\n",
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join("resources").join("views")).unwrap();
        std::fs::write(
            tmp.path()
                .join("resources")
                .join("views")
                .join("welcome.blade.php"),
            "<h1>Hi</h1>",
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join("lang").join("en")).unwrap();
        std::fs::write(
            tmp.path().join("lang").join("en").join("auth.php"),
            "<?php\nreturn ['failed' => 'Nope'];\n",
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join("routes")).unwrap();
        std::fs::write(
            tmp.path().join("routes").join("web.php"),
            "<?php\nRoute::get('/', Foo::class)->name('home');\n",
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join("public").join("css")).unwrap();
        std::fs::write(
            tmp.path().join("public").join("css").join("app.css"),
            "body{}",
        )
        .unwrap();
        std::fs::create_dir_all(tmp.path().join("bootstrap")).unwrap();
        std::fs::write(
            tmp.path().join("bootstrap").join("app.php"),
            "<?php\n$middleware->alias(['auth' => Authenticate::class]);\n",
        )
        .unwrap();
        let idx = LaravelIndex::load(tmp.path());
        assert!(idx.is_laravel);
        assert!(idx.env.get("APP_NAME").is_some());
        assert!(idx.config.get("app.name").is_some());
        assert!(idx.views.get("welcome").is_some());
        assert!(idx.translations.get("auth.failed").is_some());
        assert!(idx.routes.get("home").is_some());
        assert!(idx.assets.get("css/app.css").is_some());
        assert!(idx.middleware.get("auth").is_some());
    }

    #[test]
    fn resolve_string_key_none_for_non_laravel() {
        let laravel = LaravelIndex::default();
        let pos = Position {
            line: 0,
            character: 12,
        };
        let doc = crate::document::ast::ParsedDoc::parse("<?php\nenv('APP_NAME');\n".to_string());
        assert!(resolve_string_key(&doc, pos, &laravel).is_none());
    }

    #[test]
    fn completions_for_string_key_none_when_laravel_is_none() {
        let pos = Position {
            line: 1,
            character: 5,
        };
        assert!(completions_for_string_key("<?php\nenv('", pos, None).is_none());
    }

    #[test]
    fn resolve_definition_key_finds_env_var_declaration() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(tmp.path().join(".env"), "APP_NAME=Test\n").unwrap();
        let laravel = LaravelIndex::load(tmp.path());
        let uri = Uri::from_file_path(tmp.path().join(".env")).unwrap();
        let (names, key, _loc) = resolve_definition_key(
            &uri,
            Position {
                line: 0,
                character: 2,
            },
            &laravel,
        )
        .unwrap();
        assert_eq!(names, ENV_CALL_NAMES);
        assert_eq!(key, "APP_NAME");
    }

    #[test]
    fn resolve_definition_key_none_when_not_on_a_definition() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(tmp.path().join(".env"), "APP_NAME=Test\n").unwrap();
        let laravel = LaravelIndex::load(tmp.path());
        let uri = Uri::from_file_path(tmp.path().join("app.php")).unwrap();
        assert!(
            resolve_definition_key(
                &uri,
                Position {
                    line: 0,
                    character: 0
                },
                &laravel
            )
            .is_none()
        );
    }

    #[test]
    fn missing_key_actions_offers_env_quickfix_for_unresolved_key() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(tmp.path().join(".env"), "APP_NAME=Test\n").unwrap();
        let laravel = LaravelIndex::load(tmp.path());
        let doc = crate::document::ast::ParsedDoc::parse("<?php\nenv('DB_HOST');\n".to_string());
        let pos = Position {
            line: 1,
            character: 6,
        };
        let actions = missing_key_actions(&doc, pos, &laravel, Some(tmp.path()));
        assert_eq!(actions.len(), 1);
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected a CodeAction");
        };
        assert_eq!(action.title, "Add 'DB_HOST' to .env");
    }

    #[test]
    fn missing_key_actions_empty_when_key_already_resolves() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(tmp.path().join(".env"), "APP_NAME=Test\n").unwrap();
        let laravel = LaravelIndex::load(tmp.path());
        let doc = crate::document::ast::ParsedDoc::parse("<?php\nenv('APP_NAME');\n".to_string());
        let pos = Position {
            line: 1,
            character: 6,
        };
        assert!(missing_key_actions(&doc, pos, &laravel, Some(tmp.path())).is_empty());
    }

    #[test]
    fn missing_key_actions_empty_for_non_laravel_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let laravel = LaravelIndex::load(tmp.path());
        let doc = crate::document::ast::ParsedDoc::parse("<?php\nenv('DB_HOST');\n".to_string());
        let pos = Position {
            line: 1,
            character: 6,
        };
        assert!(missing_key_actions(&doc, pos, &laravel, Some(tmp.path())).is_empty());
    }

    #[test]
    fn missing_key_actions_empty_without_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(tmp.path().join(".env"), "APP_NAME=Test\n").unwrap();
        let laravel = LaravelIndex::load(tmp.path());
        let doc = crate::document::ast::ParsedDoc::parse("<?php\nenv('DB_HOST');\n".to_string());
        let pos = Position {
            line: 1,
            character: 6,
        };
        assert!(missing_key_actions(&doc, pos, &laravel, None).is_empty());
    }

    fn laravel_root(tmp: &std::path::Path) {
        std::fs::write(tmp.join("artisan"), "#!/usr/bin/env php").unwrap();
    }

    #[test]
    fn resolve_string_key_resolves_asset_and_middleware() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        std::fs::create_dir_all(tmp.path().join("public").join("css")).unwrap();
        std::fs::write(tmp.path().join("public").join("css").join("app.css"), "x").unwrap();
        std::fs::create_dir_all(tmp.path().join("bootstrap")).unwrap();
        std::fs::write(
            tmp.path().join("bootstrap").join("app.php"),
            "<?php\n$middleware->alias(['auth' => Authenticate::class]);\n",
        )
        .unwrap();
        let laravel = LaravelIndex::load(tmp.path());

        let doc =
            crate::document::ast::ParsedDoc::parse("<?php\nasset('css/app.css');\n".to_string());
        let pos = Position {
            line: 1,
            character: 10,
        };
        assert!(resolve_string_key(&doc, pos, &laravel).is_some());

        let doc = crate::document::ast::ParsedDoc::parse(
            "<?php\nRoute::get('/x', Foo::class)->middleware('auth');\n".to_string(),
        );
        let pos = Position {
            line: 1,
            character: 44,
        };
        assert!(resolve_string_key(&doc, pos, &laravel).is_some());
    }

    #[test]
    fn hover_for_string_key_resolves_env_and_none_for_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        std::fs::write(tmp.path().join(".env"), "APP_NAME=Test\n").unwrap();
        let laravel = LaravelIndex::load(tmp.path());

        let doc = crate::document::ast::ParsedDoc::parse("<?php\nenv('APP_NAME');\n".to_string());
        let pos = Position {
            line: 1,
            character: 8,
        };
        let hover = hover_for_string_key(&doc, pos, &laravel, Some(tmp.path())).unwrap();
        let tower_lsp_server::ls_types::HoverContents::Markup(content) = hover.contents else {
            panic!("expected markup contents");
        };
        assert!(content.value.contains("APP_NAME=Test"));

        let doc = crate::document::ast::ParsedDoc::parse("<?php\nenv('MISSING');\n".to_string());
        assert!(hover_for_string_key(&doc, pos, &laravel, Some(tmp.path())).is_none());
    }

    #[test]
    fn hover_for_string_key_none_for_non_laravel() {
        let laravel = LaravelIndex::default();
        let doc = crate::document::ast::ParsedDoc::parse("<?php\nenv('APP_NAME');\n".to_string());
        let pos = Position {
            line: 1,
            character: 8,
        };
        assert!(hover_for_string_key(&doc, pos, &laravel, None).is_none());
    }

    #[test]
    fn document_links_covers_every_domain() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        std::fs::write(tmp.path().join(".env"), "APP_NAME=Test\n").unwrap();
        std::fs::create_dir_all(tmp.path().join("public")).unwrap();
        std::fs::write(tmp.path().join("public").join("app.js"), "x").unwrap();
        std::fs::create_dir_all(tmp.path().join("bootstrap")).unwrap();
        std::fs::write(
            tmp.path().join("bootstrap").join("app.php"),
            "<?php\n$middleware->alias(['auth' => Authenticate::class]);\n",
        )
        .unwrap();
        let laravel = LaravelIndex::load(tmp.path());

        let doc = crate::document::ast::ParsedDoc::parse(
            "<?php\nenv('APP_NAME');\nasset('app.js');\nRoute::get('/x', Foo::class)->middleware('auth');\nenv('MISSING');\n"
                .to_string(),
        );
        let links = document_links(&doc, &laravel);
        assert_eq!(links.len(), 3);
    }

    #[test]
    fn document_links_empty_for_non_laravel() {
        let laravel = LaravelIndex::default();
        let doc = crate::document::ast::ParsedDoc::parse("<?php\nenv('APP_NAME');\n".to_string());
        assert!(document_links(&doc, &laravel).is_empty());
    }

    #[test]
    fn completions_for_string_key_covers_asset_and_middleware() {
        let tmp = tempfile::tempdir().unwrap();
        laravel_root(tmp.path());
        std::fs::create_dir_all(tmp.path().join("public").join("css")).unwrap();
        std::fs::write(tmp.path().join("public").join("css").join("app.css"), "x").unwrap();
        std::fs::create_dir_all(tmp.path().join("bootstrap")).unwrap();
        std::fs::write(
            tmp.path().join("bootstrap").join("app.php"),
            "<?php\n$middleware->alias(['auth' => Authenticate::class]);\n",
        )
        .unwrap();
        let laravel = LaravelIndex::load(tmp.path());

        let pos = Position {
            line: 1,
            character: 10,
        };
        let items = completions_for_string_key("<?php\nasset('css/", pos, Some(&laravel)).unwrap();
        assert!(items.iter().any(|i| i.label == "css/app.css"));

        let src = "<?php\nRoute::get('/x', Foo::class)->middleware('au";
        let pos = Position {
            line: 1,
            character: 44,
        };
        let items = completions_for_string_key(src, pos, Some(&laravel)).unwrap();
        assert!(items.iter().any(|i| i.label == "auth"));
    }
}
