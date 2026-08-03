//! Middleware alias index powering go-to-definition, hover, document links
//! and completion for `->middleware('alias')` / `Route::middleware([...])`
//! call sites.
//!
//! Aliases are registered in one of two places depending on the Laravel
//! version targeted by the project:
//! - Laravel 11+: `bootstrap/app.php`, via
//!   `->withMiddleware(function (Middleware $middleware) { $middleware->alias([...]); })`.
//! - Laravel 10 and earlier: `app/Http/Kernel.php`, via the `$routeMiddleware`
//!   (or, from 9.x, `$middlewareAliases`) class property.
//!
//! Both are scanned unconditionally — a project only ever has one or the
//! other, so there's no ordering/precedence concern like `EnvIndex`'s
//! `.env`/`.env.example` pair.
//!
//! Laravel 11+'s built-in `web`/`api` middleware *groups* (as opposed to
//! aliases) are also indexed, from `$middleware->group('name', [...])` calls
//! in `bootstrap/app.php` — the group name resolves the same way an alias
//! does (`Route::middleware('web')`), pointing at the `group(...)`
//! registration site rather than a specific middleware class, since a group
//! has no single one.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::Path;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{ClassMember, ClassMemberKind, Expr, ExprKind, Span};
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, Location, Position, Range, Uri,
};

use crate::analysis::diagnostics::parse_document_no_diags;
use crate::document::ast::{ParsedDoc, SourceView};

use super::string_call::content_span;

#[derive(Debug, Default, Clone)]
pub struct MiddlewareIndex {
    aliases: HashMap<String, Location>,
}

impl MiddlewareIndex {
    pub fn get(&self, alias: &str) -> Option<&Location> {
        self.aliases.get(alias)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.aliases.keys().map(String::as_str)
    }

    /// The middleware alias whose registration contains `position`, if any —
    /// the reverse of `get`, used to recognize a find-references request
    /// starting from the definition site.
    pub fn key_at(&self, uri: &Uri, position: Position) -> Option<&str> {
        crate::laravel::location_lookup::key_at(&self.aliases, uri, position)
    }

    pub(super) fn load(root: &Path) -> Self {
        let mut aliases = HashMap::new();
        load_bootstrap_app(root, &mut aliases);
        load_http_kernel(root, &mut aliases);
        Self { aliases }
    }
}

fn load_bootstrap_app(root: &Path, out: &mut HashMap<String, Location>) {
    let path = root.join("bootstrap").join("app.php");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Some(uri) = Uri::from_file_path(&path) else {
        return;
    };
    let doc = parse_document_no_diags(&text);
    let sv = doc.view();
    let mut visitor = AliasCallVisitor { sv, uri: &uri, out };
    for stmt in doc.program().stmts.iter() {
        let _ = visitor.visit_stmt(stmt);
    }
}

/// Walks every expression looking for `<anything>->alias([...])` and
/// `<anything>->group('name', [...])` — the receiver isn't checked against a
/// specific variable name (unlike `request_fields`'s `$request` heuristic)
/// since both method names are distinctive enough on their own within
/// `bootstrap/app.php`.
struct AliasCallVisitor<'a> {
    sv: SourceView<'a>,
    uri: &'a Uri,
    out: &'a mut HashMap<String, Location>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for AliasCallVisitor<'_> {
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::MethodCall(mc) = &expr.kind {
            if is_ident(mc.method, "alias")
                && let Some(arg) = mc.args.first()
                && let Some(arg_value) = &arg.value
                && let ExprKind::Array(elements) = &arg_value.kind
            {
                collect_flat_string_keys(elements, self.sv, self.uri, self.out);
            }
            if is_ident(mc.method, "group")
                && mc.args.len() == 2
                && let Some(name_arg) = mc.args[0].value.as_ref()
                && let ExprKind::String(name) = &name_arg.kind
            {
                // `span.start`/`span.end` point at the surrounding quotes;
                // trim one byte off each side to land on the name text
                // itself (see `editing::document_link::link_from_path_expr`).
                let range = Range {
                    start: self.sv.position_of(name_arg.span.start + 1),
                    end: self.sv.position_of(name_arg.span.end - 1),
                };
                self.out
                    .entry(name.to_string())
                    .or_insert_with(|| Location {
                        uri: self.uri.clone(),
                        range,
                    });
            }
        }
        walk_expr(self, expr)
    }
}

fn load_http_kernel(root: &Path, out: &mut HashMap<String, Location>) {
    let path = root.join("app").join("Http").join("Kernel.php");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Some(uri) = Uri::from_file_path(&path) else {
        return;
    };
    let doc = parse_document_no_diags(&text);
    let sv = doc.view();
    let mut visitor = KernelPropertyVisitor { sv, uri: &uri, out };
    for stmt in doc.program().stmts.iter() {
        let _ = visitor.visit_stmt(stmt);
    }
}

/// Names of the `Kernel` class properties that hold the alias map — renamed
/// from `$routeMiddleware` to `$middlewareAliases` in Laravel 9; both are
/// checked since either can appear depending on the project's history.
const KERNEL_ALIAS_PROPERTIES: &[&str] = &["routeMiddleware", "middlewareAliases"];

struct KernelPropertyVisitor<'a> {
    sv: SourceView<'a>,
    uri: &'a Uri,
    out: &'a mut HashMap<String, Location>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for KernelPropertyVisitor<'_> {
    fn visit_class_member(&mut self, member: &ClassMember<'arena, 'src>) -> ControlFlow<()> {
        if let ClassMemberKind::Property(prop) = &member.kind
            && KERNEL_ALIAS_PROPERTIES
                .iter()
                .any(|name| prop.name == *name)
            && let Some(default) = &prop.default
            && let ExprKind::Array(elements) = &default.kind
        {
            collect_flat_string_keys(elements, self.sv, self.uri, self.out);
        }
        ControlFlow::Continue(())
    }
}

fn is_ident(expr: &Expr<'_, '_>, name: &str) -> bool {
    matches!(&expr.kind, ExprKind::Identifier(n) if n.eq_ignore_ascii_case(name))
}

/// Collects every string-keyed entry of a flat (non-nested) array literal —
/// middleware alias maps are always one level deep, unlike `config`'s nested
/// arrays.
fn collect_flat_string_keys(
    elements: &[php_ast::ArrayElement<'_, '_>],
    sv: SourceView<'_>,
    uri: &Uri,
    out: &mut HashMap<String, Location>,
) {
    for el in elements {
        let Some(key_expr) = &el.key else { continue };
        let ExprKind::String(key) = &key_expr.kind else {
            continue;
        };
        // `span.start`/`span.end` point at the surrounding quotes; trim one
        // byte off each side to land on the key text itself (see
        // `editing::document_link::link_from_path_expr`).
        let range = Range {
            start: sv.position_of(key_expr.span.start + 1),
            end: sv.position_of(key_expr.span.end - 1),
        };
        out.entry(key.to_string()).or_insert_with(|| Location {
            uri: uri.clone(),
            range,
        });
    }
}

/// Completion items for middleware aliases starting with `prefix`.
pub(crate) fn middleware_completions(index: &MiddlewareIndex, prefix: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = index
        .names()
        .filter(|name| name.starts_with(prefix))
        .map(|name| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::CONSTANT),
            insert_text: Some(name.to_string()),
            ..Default::default()
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

/// One `->middleware(...)`/`Route::middleware(...)` string argument found in
/// application code — either the sole string argument or one element of an
/// array-of-strings argument. `alias` has any `:params` suffix
/// (`'throttle:60,1'`) already stripped; `token_span` is the *full* original
/// string literal's span (quotes included, params included) — used for
/// cursor-containment, matching `string_call::call_string_arg`'s own
/// semantics — while `range` covers only the alias portion for the location
/// actually reported to the client.
struct MiddlewareCall {
    token_span: Span,
    alias: String,
    range: Range,
}

fn collect_middleware_calls_raw(doc: &ParsedDoc) -> Vec<MiddlewareCall> {
    struct Collector<'a> {
        source: &'a str,
        sv: SourceView<'a>,
        out: Vec<MiddlewareCall>,
    }

    impl<'arena, 'src> Visitor<'arena, 'src> for Collector<'_> {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            let args = match &expr.kind {
                ExprKind::MethodCall(mc) if is_ident(mc.method, "middleware") => Some(&mc.args),
                ExprKind::StaticMethodCall(s) if is_ident(s.method, "middleware") => Some(&s.args),
                _ => None,
            };
            if let Some(args) = args
                && let Some(arg) = args.first()
                && let Some(arg_value) = &arg.value
            {
                match &arg_value.kind {
                    ExprKind::String(s) => self.push_string(s, arg_value.span),
                    ExprKind::Array(elements) => {
                        for el in elements.iter() {
                            if el.key.is_none()
                                && let ExprKind::String(s) = &el.value.kind
                            {
                                self.push_string(s, el.value.span);
                            }
                        }
                    }
                    _ => {}
                }
            }
            walk_expr(self, expr)
        }
    }

    impl Collector<'_> {
        fn push_string(&mut self, content: &str, token_span: Span) {
            let Some(cspan) = content_span(self.source, token_span) else {
                return;
            };
            let alias = content.split_once(':').map_or(content, |(a, _)| a);
            let range = Range {
                start: self.sv.position_of(cspan.start),
                end: self.sv.position_of(cspan.start + alias.len() as u32),
            };
            self.out.push(MiddlewareCall {
                token_span,
                alias: alias.to_string(),
                range,
            });
        }
    }

    let mut collector = Collector {
        source: doc.source(),
        sv: doc.view(),
        out: Vec::new(),
    };
    for stmt in doc.program().stmts.iter() {
        let _ = collector.visit_stmt(stmt);
    }
    collector.out
}

/// The middleware alias and its `Range`, when the cursor sits inside a
/// `middleware('...')` call's string argument (or one array element of it).
pub(crate) fn middleware_alias_at(doc: &ParsedDoc, position: Position) -> Option<(String, Range)> {
    let sv = doc.view();
    let offset = sv.byte_of_position(position);
    collect_middleware_calls_raw(doc)
        .into_iter()
        .find(|c| c.token_span.start <= offset && offset < c.token_span.end)
        .map(|c| (c.alias, c.range))
}

/// Every `middleware(...)` alias usage in `doc`, decoded and with its
/// `Range` — used to build document links for a whole file in one AST walk.
pub(crate) fn collect_middleware_calls(doc: &ParsedDoc) -> Vec<(String, Range)> {
    collect_middleware_calls_raw(doc)
        .into_iter()
        .map(|c| (c.alias, c.range))
        .collect()
}

/// Strips a run of already-typed, comma-separated array elements
/// (`'auth', 'verif` → back to just after the `[`), so
/// `middleware_string_prefix` can recognize the array form of a
/// `middleware(['a', 'b'])` call while the last element is still being
/// typed. Returns `None` if the text isn't shaped like a (possibly empty)
/// run of quoted elements immediately preceded by `[`.
fn strip_trailing_array_elements(mut s: &str) -> Option<&str> {
    loop {
        s = s.trim_end();
        if let Some(rest) = s.strip_suffix('[') {
            return Some(rest.trim_end());
        }
        let rest = s.strip_suffix(',')?.trim_end();
        let quote = *rest.as_bytes().last()?;
        if quote != b'\'' && quote != b'"' {
            return None;
        }
        let without_close = &rest[..rest.len() - 1];
        let open_idx = without_close.rfind(quote as char)?;
        s = &without_close[..open_idx];
    }
}

/// Typed prefix (from the opening quote up to the cursor), when the cursor
/// sits inside a string literal — closed or not — that's either the sole
/// argument or an array element of a `middleware(...)` call. Used for
/// completion, where the closing quote may not exist yet.
pub(crate) fn middleware_string_prefix(source: &str, position: Position) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let line = *lines.get(position.line as usize)?;
    let byte_col = crate::text::utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..byte_col];
    let quote_pos = before.rfind(['\'', '"'])?;
    let before_quote = before[..quote_pos].trim_end();
    let head = strip_trailing_array_elements(before_quote).unwrap_or(before_quote);
    let head = head.strip_suffix('(')?.trim_end();
    let name = "middleware";
    if head.len() < name.len() || !head[head.len() - name.len()..].eq_ignore_ascii_case(name) {
        return None;
    }
    let before_name = &head[..head.len() - name.len()];
    if !before_name.is_empty()
        && matches!(
            before_name.as_bytes().last(),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        )
    {
        return None;
    }
    Some(before[quote_pos + 1..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::Position;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn indexes_alias_from_bootstrap_app() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "bootstrap/app.php",
            "<?php\nreturn Application::configure()\n    ->withMiddleware(function (Middleware $middleware) {\n        $middleware->alias([\n            'auth' => \\App\\Http\\Middleware\\Authenticate::class,\n            'admin' => \\App\\Http\\Middleware\\EnsureIsAdmin::class,\n        ]);\n    })->create();\n",
        );
        let idx = MiddlewareIndex::load(tmp.path());
        let auth = idx.get("auth").unwrap();
        assert_eq!((auth.range.start.line, auth.range.start.character), (4, 13));
        assert_eq!(auth.range.end.character, 17);
        let admin = idx.get("admin").unwrap();
        assert_eq!(
            (admin.range.start.line, admin.range.start.character),
            (5, 13)
        );
        assert_eq!(admin.range.end.character, 18);
    }

    #[test]
    fn indexes_alias_from_legacy_kernel_route_middleware() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "app/Http/Kernel.php",
            "<?php\nclass Kernel extends HttpKernel {\n    protected $routeMiddleware = [\n        'auth' => \\App\\Http\\Middleware\\Authenticate::class,\n    ];\n}\n",
        );
        let idx = MiddlewareIndex::load(tmp.path());
        let loc = idx.get("auth").unwrap();
        assert_eq!((loc.range.start.line, loc.range.start.character), (3, 9));
        assert_eq!(loc.range.end.character, 13);
    }

    #[test]
    fn indexes_alias_from_renamed_kernel_middleware_aliases() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "app/Http/Kernel.php",
            "<?php\nclass Kernel extends HttpKernel {\n    protected $middlewareAliases = [\n        'verified' => EnsureEmailIsVerified::class,\n    ];\n}\n",
        );
        let idx = MiddlewareIndex::load(tmp.path());
        let loc = idx.get("verified").unwrap();
        assert_eq!((loc.range.start.line, loc.range.start.character), (3, 9));
        assert_eq!(loc.range.end.character, 17);
    }

    #[test]
    fn indexes_group_from_bootstrap_app() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "bootstrap/app.php",
            "<?php\nreturn Application::configure()\n    ->withMiddleware(function (Middleware $middleware) {\n        $middleware->group('web', [\n            \\App\\Http\\Middleware\\EncryptCookies::class,\n        ]);\n        $middleware->alias([\n            'auth' => \\App\\Http\\Middleware\\Authenticate::class,\n        ]);\n    })->create();\n",
        );
        let idx = MiddlewareIndex::load(tmp.path());
        let web = idx.get("web").unwrap();
        assert_eq!((web.range.start.line, web.range.start.character), (3, 28));
        assert_eq!(web.range.end.character, 31);
        assert!(idx.get("auth").is_some());
    }

    #[test]
    fn missing_files_yield_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = MiddlewareIndex::load(tmp.path());
        assert_eq!(idx.names().count(), 0);
    }

    #[test]
    fn middleware_completions_filters_by_prefix_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "app/Http/Kernel.php",
            "<?php\nclass Kernel extends HttpKernel {\n    protected $routeMiddleware = [\n        'auth' => A::class,\n        'auth.basic' => B::class,\n        'guest' => C::class,\n    ];\n}\n",
        );
        let idx = MiddlewareIndex::load(tmp.path());
        let items = middleware_completions(&idx, "auth");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["auth", "auth.basic"]);
    }

    fn parse(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    #[test]
    fn middleware_alias_at_matches_single_string_call() {
        let doc = parse("<?php\nRoute::get('/x', Foo::class)->middleware('auth');\n");
        let pos = Position {
            line: 1,
            character: 42,
        };
        let (alias, _) = middleware_alias_at(&doc, pos).unwrap();
        assert_eq!(alias, "auth");
    }

    #[test]
    fn middleware_alias_at_strips_parameters() {
        let doc = parse("<?php\nRoute::get('/x', Foo::class)->middleware('throttle:60,1');\n");
        let pos = Position {
            line: 1,
            character: 42,
        };
        let (alias, _) = middleware_alias_at(&doc, pos).unwrap();
        assert_eq!(alias, "throttle");
    }

    #[test]
    fn middleware_alias_at_matches_array_element() {
        let doc = parse("<?php\nRoute::middleware(['auth', 'verified'])->group(function () {});\n");
        let pos = Position {
            line: 1,
            character: 33,
        };
        let (alias, _) = middleware_alias_at(&doc, pos).unwrap();
        assert_eq!(alias, "verified");
    }

    #[test]
    fn collect_middleware_calls_finds_every_site() {
        let doc = parse(
            "<?php\n$a = fn() => Route::get('/x', Foo::class)->middleware('auth');\n$b = fn() => Route::get('/y', Bar::class)->middleware('auth');\n",
        );
        let calls = collect_middleware_calls(&doc);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "auth");
        assert_eq!(
            calls[0].1.start,
            Position {
                line: 1,
                character: 55
            }
        );
        assert_eq!(calls[1].0, "auth");
        assert_eq!(
            calls[1].1.start,
            Position {
                line: 2,
                character: 55
            }
        );
    }

    #[test]
    fn middleware_string_prefix_matches_single_string_form() {
        let src = "<?php\nRoute::get('/x', Foo::class)->middleware('au";
        let pos = Position {
            line: 1,
            character: 44,
        };
        assert_eq!(middleware_string_prefix(src, pos).as_deref(), Some("au"));
    }

    #[test]
    fn middleware_string_prefix_matches_empty_array_form() {
        let src = "<?php\nRoute::middleware(['";
        let pos = Position {
            line: 1,
            character: 21,
        };
        assert_eq!(middleware_string_prefix(src, pos).as_deref(), Some(""));
    }

    #[test]
    fn middleware_string_prefix_matches_second_array_element() {
        let src = "<?php\nRoute::middleware(['auth', 've";
        let pos = Position {
            line: 1,
            character: 32,
        };
        assert_eq!(middleware_string_prefix(src, pos).as_deref(), Some("ve"));
    }

    #[test]
    fn middleware_string_prefix_rejects_unrelated_call() {
        let src = "<?php\n$request->input('em";
        let pos = Position {
            line: 1,
            character: 19,
        };
        assert!(middleware_string_prefix(src, pos).is_none());
    }
}
