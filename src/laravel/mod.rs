//! Laravel framework support.
//!
//! Beyond generic PHP analysis, Laravel projects lean heavily on stringly-typed
//! lookups — `env('KEY')`, `config('a.b.c')`, `view('a.b')`, `route('name')`,
//! `trans('a.b')` — that resolve against files elsewhere in the project rather
//! than through normal symbol resolution. `LaravelIndex` builds a workspace-
//! scan-time index for each of these domains (`env` and `config` so far;
//! further domains are added incrementally) and wires them into
//! go-to-definition ([`resolve_string_key`]) and completion
//! ([`completions_for_string_key`]).
//!
//! Gated behind [`LaravelIndex::load`]'s project-detection check, so
//! non-Laravel workspaces pay no cost beyond the one-time
//! `artisan`/`composer.json` probe: both dispatch functions bail out on the
//! `is_laravel` flag before doing any string scanning.

mod config_index;
mod detect;
mod env_index;
mod location_lookup;
mod route_index;
mod string_call;
mod translation_index;
mod view_index;

pub use config_index::ConfigIndex;
pub use env_index::EnvIndex;
pub use route_index::RouteIndex;
pub use translation_index::TranslationIndex;
pub use view_index::ViewIndex;

use config_index::config_completions;
use env_index::env_completions;
use route_index::route_completions;
use string_call::{call_string_arg, call_string_prefix};
use translation_index::translation_completions;
use view_index::view_completions;

use std::path::Path;

use tower_lsp_server::ls_types::{CompletionItem, Location, Position, Uri};

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

#[derive(Debug, Default)]
pub struct LaravelIndex {
    pub is_laravel: bool,
    pub env: EnvIndex,
    pub config: ConfigIndex,
    pub views: ViewIndex,
    pub translations: TranslationIndex,
    pub routes: RouteIndex,
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
        }
    }
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
    None
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
    None
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
        let idx = LaravelIndex::load(tmp.path());
        assert!(idx.is_laravel);
        assert!(idx.env.get("APP_NAME").is_some());
        assert!(idx.config.get("app.name").is_some());
        assert!(idx.views.get("welcome").is_some());
        assert!(idx.translations.get("auth.failed").is_some());
        assert!(idx.routes.get("home").is_some());
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
}
