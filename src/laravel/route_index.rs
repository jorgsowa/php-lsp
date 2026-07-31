//! `routes/*.php` index powering go-to-definition and completion for
//! `route('name')` calls.
//!
//! Only explicit `->name('...')` registrations are indexed — `Route::
//! resource()`/`apiResource()` implicit CRUD route names (replicating
//! Laravel's verb/action naming convention) are a known gap. `Route::
//! group(['as' => 'prefix.'], function () { ... })` and the fluent
//! `Route::name('prefix.')->group(function () { ... })` equivalent both
//! prepend their `as` prefix to every `->name(...)` registered inside the
//! closure, including nested groups.

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

    /// Direct `.php` children of `routes/` (matches `ConfigIndex`'s and
    /// `TranslationIndex`'s same simplification — nested route files
    /// `require`d from these aren't followed).
    pub(super) fn load(root: &Path) -> Self {
        let mut routes = HashMap::new();
        let Ok(entries) = std::fs::read_dir(root.join("routes")) else {
            return Self { routes };
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "php") {
                continue;
            }
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
        assert!(idx.get("home").is_some());
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
        assert!(idx.get("admin.dashboard").is_some());
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
        assert!(idx.get("api.users").is_some());
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
        assert!(idx.get("admin.users.index").is_some());
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
        assert!(idx.get("x").is_some());
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
