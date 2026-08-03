//! `routes/*.php` index powering go-to-definition and completion for
//! `route('name')` calls.
//!
//! Explicit `->name('...')` registrations are indexed, as are `Route::
//! resource('posts', PostController::class)`/`apiResource(...)`'s seven (or
//! five) implicit CRUD route names — `posts.index`, `posts.create`,
//! `posts.store`, ... — synthesized from the resource's base name.
//! `->only()`/`->except()`/`->name()`/`->names()` fluent modifiers on a
//! resource registration aren't honored, so all seven/five names are always
//! synthesized regardless of such filtering — a known remaining gap. `Route::
//! group(['as' => 'prefix.'], function () { ... })` and the fluent
//! `Route::name('prefix.')->group(function () { ... })` equivalent both
//! prepend their `as` prefix to every `->name(...)`/resource registration
//! inside the closure, including nested groups.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::Path;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{Block, Expr, ExprKind};
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, Location, Position, Range, Uri,
};

use crate::analysis::diagnostics::parse_document_no_diags;
use crate::document::ast::SourceView;

#[derive(Debug, Default, Clone)]
pub struct RouteIndex {
    routes: HashMap<String, Location>,
}

impl RouteIndex {
    pub fn get(&self, name: &str) -> Option<&Location> {
        self.routes.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.routes.keys().map(String::as_str)
    }

    /// The route name whose `->name(...)` declaration contains `position`,
    /// if any — the reverse of `get`, used to recognize a find-references
    /// request starting from the definition site.
    pub fn key_at(&self, uri: &Uri, position: Position) -> Option<&str> {
        crate::laravel::location_lookup::key_at(&self.routes, uri, position)
    }

    /// Every `.php` file under `routes/`, including nested subdirectories —
    /// `require`d files that live outside that tree still aren't followed.
    pub(super) fn load(root: &Path) -> Self {
        let mut routes = HashMap::new();
        for path in super::fs_walk::php_files_recursive(&root.join("routes")) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(uri) = Uri::from_file_path(&path) else {
                continue;
            };
            let doc = parse_document_no_diags(&text);
            let sv = doc.view();
            let mut visitor = RouteVisitor {
                sv,
                uri: &uri,
                prefix_stack: Vec::new(),
                out: &mut routes,
            };
            for stmt in doc.program().stmts.iter() {
                let _ = visitor.visit_stmt(stmt);
            }
        }
        Self { routes }
    }
}

struct RouteVisitor<'a> {
    sv: SourceView<'a>,
    uri: &'a Uri,
    prefix_stack: Vec<String>,
    out: &'a mut HashMap<String, Location>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for RouteVisitor<'_> {
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::MethodCall(mc) = &expr.kind
            && is_ident(mc.method, "name")
            && let Some(arg) = mc.args.first()
            && let Some(arg_value) = &arg.value
            && let ExprKind::String(name) = &arg_value.kind
        {
            let full_name = format!("{}{name}", self.prefix_stack.concat());
            // `span.start`/`span.end` point at the surrounding quotes (see
            // `editing::document_link::link_from_path_expr`); trim one byte
            // off each side to land on the name text itself.
            let range = Range {
                start: self.sv.position_of(arg_value.span.start + 1),
                end: self.sv.position_of(arg_value.span.end - 1),
            };
            self.out.entry(full_name).or_insert_with(|| Location {
                uri: self.uri.clone(),
                range,
            });
        }

        if let ExprKind::StaticMethodCall(s) = &expr.kind
            && let Some(actions) = resource_actions(s.method)
            && let Some(arg) = s.args.first()
            && let Some(arg_value) = &arg.value
            && let ExprKind::String(base) = &arg_value.kind
        {
            let prefix = self.prefix_stack.concat();
            // Points at the resource's base-name literal — there's no
            // separate name string to point at like `->name(...)` has, since
            // these names are synthesized rather than written out.
            let range = Range {
                start: self.sv.position_of(arg_value.span.start + 1),
                end: self.sv.position_of(arg_value.span.end - 1),
            };
            for action in actions {
                let full_name = format!("{prefix}{base}.{action}");
                self.out.entry(full_name).or_insert_with(|| Location {
                    uri: self.uri.clone(),
                    range,
                });
            }
        }

        if let Some((as_prefix, block)) = group_closure(expr) {
            self.prefix_stack.push(as_prefix.unwrap_or_default());
            for stmt in block.stmts.iter() {
                let _ = self.visit_stmt(stmt);
            }
            self.prefix_stack.pop();
            // Already walked the closure body above; the object chain
            // leading into `->group(...)` (e.g. `Route::name('x.')`) is
            // deliberately not walked — it's a prefix declaration, not a
            // route registration.
            return ControlFlow::Continue(());
        }

        walk_expr(self, expr)
    }
}

fn is_ident(expr: &Expr<'_, '_>, name: &str) -> bool {
    matches!(&expr.kind, ExprKind::Identifier(n) if n.eq_ignore_ascii_case(name))
}

const RESOURCE_ACTIONS: &[&str] = &[
    "index", "create", "store", "show", "edit", "update", "destroy",
];
const API_RESOURCE_ACTIONS: &[&str] = &["index", "store", "show", "update", "destroy"];

/// The implicit action names `Route::resource()`/`apiResource()` synthesizes,
/// keyed by the static method identifier — `None` for anything else.
fn resource_actions(method: &Expr<'_, '_>) -> Option<&'static [&'static str]> {
    if is_ident(method, "resource") {
        Some(RESOURCE_ACTIONS)
    } else if is_ident(method, "apiResource") {
        Some(API_RESOURCE_ACTIONS)
    } else {
        None
    }
}

/// Recognizes `Route::group(['as' => 'x.'], function () {...})` and
/// `<chain>->group(function () {...})`, returning the group's `as` prefix
/// (if any) and the closure body to descend into.
fn group_closure<'arena, 'src>(
    expr: &Expr<'arena, 'src>,
) -> Option<(Option<String>, &'arena Block<'arena, 'src>)> {
    match &expr.kind {
        ExprKind::StaticMethodCall(s) if is_ident(s.method, "group") && s.args.len() == 2 => {
            let as_prefix = s.args[0].value.as_ref().and_then(array_as_prefix);
            let block = closure_block(s.args[1].value.as_ref()?)?;
            Some((as_prefix, block))
        }
        ExprKind::MethodCall(mc) if is_ident(mc.method, "group") && mc.args.len() == 1 => {
            let as_prefix = find_as_prefix_in_chain(mc.object);
            let block = closure_block(mc.args[0].value.as_ref()?)?;
            Some((as_prefix, block))
        }
        _ => None,
    }
}

fn closure_block<'arena, 'src>(expr: &Expr<'arena, 'src>) -> Option<&'arena Block<'arena, 'src>> {
    match &expr.kind {
        ExprKind::Closure(c) => Some(c.body),
        _ => None,
    }
}

/// Extracts the `'as' => '...'` entry from a route-group attributes array.
fn array_as_prefix(expr: &Expr<'_, '_>) -> Option<String> {
    let ExprKind::Array(elements) = &expr.kind else {
        return None;
    };
    elements.iter().find_map(|el| {
        let key_expr = el.key.as_ref()?;
        let ExprKind::String(key) = &key_expr.kind else {
            return None;
        };
        if *key != "as" {
            return None;
        }
        let ExprKind::String(val) = &el.value.kind else {
            return None;
        };
        Some(val.to_string())
    })
}

/// Walks a fluent method-call chain (`Route::name('x.')->middleware(...)` or
/// `Route::prefix('admin')->name('x.')`) looking for a `->name('...')` /
/// `Route::name('...')` call anywhere in it — the equivalent of the
/// `'as' => '...'` array entry for the fluent group-attribute form. The
/// chain bottoms out at a `StaticMethodCall` (`Route::...`), which is where
/// `Route::name(...)` itself is found when it's the first call in the chain.
fn find_as_prefix_in_chain(expr: &Expr<'_, '_>) -> Option<String> {
    match &expr.kind {
        ExprKind::MethodCall(mc) => {
            if is_ident(mc.method, "name")
                && let Some(arg) = mc.args.first()
                && let Some(arg_value) = &arg.value
                && let ExprKind::String(s) = &arg_value.kind
            {
                return Some(s.to_string());
            }
            find_as_prefix_in_chain(mc.object)
        }
        ExprKind::StaticMethodCall(s) if is_ident(s.method, "name") => {
            let arg = s.args.first()?;
            let arg_value = arg.value.as_ref()?;
            let ExprKind::String(name) = &arg_value.kind else {
                return None;
            };
            Some(name.to_string())
        }
        _ => None,
    }
}

/// Completion items for route names starting with `prefix`.
pub(crate) fn route_completions(index: &RouteIndex, prefix: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = index
        .names()
        .filter(|name| name.starts_with(prefix))
        .map(|name| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::REFERENCE),
            insert_text: Some(name.to_string()),
            ..Default::default()
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_routes(root: &Path, name: &str, contents: &str) {
        std::fs::create_dir_all(root.join("routes")).unwrap();
        std::fs::write(root.join("routes").join(name), contents).unwrap();
    }

    #[test]
    fn indexes_top_level_named_route() {
        let tmp = tempfile::tempdir().unwrap();
        write_routes(
            tmp.path(),
            "web.php",
            "<?php\nRoute::get('/home', HomeController::class)->name('home');\n",
        );
        let idx = RouteIndex::load(tmp.path());
        let loc = idx.get("home").unwrap();
        assert_eq!((loc.range.start.line, loc.range.start.character), (1, 50));
        assert_eq!(loc.range.end.character, 54);
    }

    #[test]
    fn unnamed_routes_are_not_indexed() {
        let tmp = tempfile::tempdir().unwrap();
        write_routes(
            tmp.path(),
            "web.php",
            "<?php\nRoute::get('/home', HomeController::class);\n",
        );
        let idx = RouteIndex::load(tmp.path());
        assert_eq!(idx.names().count(), 0);
    }

    #[test]
    fn array_group_prefix_applies_to_nested_route_name() {
        let tmp = tempfile::tempdir().unwrap();
        write_routes(
            tmp.path(),
            "web.php",
            "<?php\nRoute::group(['as' => 'admin.'], function () {\n    Route::get('/dashboard', Foo::class)->name('dashboard');\n});\n",
        );
        let idx = RouteIndex::load(tmp.path());
        let loc = idx.get("admin.dashboard").unwrap();
        assert_eq!((loc.range.start.line, loc.range.start.character), (2, 48));
        assert_eq!(loc.range.end.character, 57);
        assert!(idx.get("dashboard").is_none());
    }

    #[test]
    fn fluent_group_prefix_applies_to_nested_route_name() {
        let tmp = tempfile::tempdir().unwrap();
        write_routes(
            tmp.path(),
            "web.php",
            "<?php\nRoute::name('api.')->group(function () {\n    Route::get('/users', Foo::class)->name('users');\n});\n",
        );
        let idx = RouteIndex::load(tmp.path());
        let loc = idx.get("api.users").unwrap();
        assert_eq!((loc.range.start.line, loc.range.start.character), (2, 44));
        assert_eq!(loc.range.end.character, 49);
        // The group-prefix declaration itself must not be indexed as a route.
        assert!(idx.get("api.").is_none());
    }

    #[test]
    fn nested_groups_concatenate_prefixes() {
        let tmp = tempfile::tempdir().unwrap();
        write_routes(
            tmp.path(),
            "web.php",
            "<?php\nRoute::group(['as' => 'admin.'], function () {\n    Route::group(['as' => 'users.'], function () {\n        Route::get('/', Foo::class)->name('index');\n    });\n});\n",
        );
        let idx = RouteIndex::load(tmp.path());
        let loc = idx.get("admin.users.index").unwrap();
        assert_eq!((loc.range.start.line, loc.range.start.character), (3, 43));
        assert_eq!(loc.range.end.character, 48);
    }

    #[test]
    fn group_without_as_prefix_has_no_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        write_routes(
            tmp.path(),
            "web.php",
            "<?php\nRoute::group(['middleware' => 'auth'], function () {\n    Route::get('/x', Foo::class)->name('x');\n});\n",
        );
        let idx = RouteIndex::load(tmp.path());
        let loc = idx.get("x").unwrap();
        assert_eq!((loc.range.start.line, loc.range.start.character), (2, 40));
        assert_eq!(loc.range.end.character, 41);
    }

    #[test]
    fn resource_synthesizes_seven_implicit_names() {
        let tmp = tempfile::tempdir().unwrap();
        write_routes(
            tmp.path(),
            "web.php",
            "<?php\nRoute::resource('posts', PostController::class);\n",
        );
        let idx = RouteIndex::load(tmp.path());
        for action in [
            "index", "create", "store", "show", "edit", "update", "destroy",
        ] {
            assert!(
                idx.get(&format!("posts.{action}")).is_some(),
                "missing posts.{action}"
            );
        }
        assert_eq!(idx.names().count(), 7);
    }

    #[test]
    fn api_resource_synthesizes_five_implicit_names() {
        let tmp = tempfile::tempdir().unwrap();
        write_routes(
            tmp.path(),
            "web.php",
            "<?php\nRoute::apiResource('posts', PostController::class);\n",
        );
        let idx = RouteIndex::load(tmp.path());
        for action in ["index", "store", "show", "update", "destroy"] {
            assert!(idx.get(&format!("posts.{action}")).is_some());
        }
        assert!(idx.get("posts.create").is_none());
        assert!(idx.get("posts.edit").is_none());
        assert_eq!(idx.names().count(), 5);
    }

    #[test]
    fn resource_respects_as_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        write_routes(
            tmp.path(),
            "web.php",
            "<?php\nRoute::group(['as' => 'admin.'], function () {\n    Route::resource('posts', PostController::class);\n});\n",
        );
        let idx = RouteIndex::load(tmp.path());
        assert!(idx.get("admin.posts.index").is_some());
        assert!(idx.get("posts.index").is_none());
    }

    #[test]
    fn indexes_nested_route_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("routes").join("api")).unwrap();
        std::fs::write(
            tmp.path().join("routes").join("api").join("v1.php"),
            "<?php\nRoute::get('/users', Foo::class)->name('api.users');\n",
        )
        .unwrap();
        let idx = RouteIndex::load(tmp.path());
        assert!(idx.get("api.users").is_some());
    }

    #[test]
    fn missing_routes_dir_yields_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = RouteIndex::load(tmp.path());
        assert_eq!(idx.names().count(), 0);
    }

    #[test]
    fn route_completions_filters_by_prefix_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write_routes(
            tmp.path(),
            "web.php",
            "<?php\nRoute::get('/a', Foo::class)->name('admin.index');\nRoute::get('/b', Foo::class)->name('admin.show');\nRoute::get('/c', Foo::class)->name('home');\n",
        );
        let idx = RouteIndex::load(tmp.path());
        let items = route_completions(&idx, "admin.");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["admin.index", "admin.show"]);
    }
}
