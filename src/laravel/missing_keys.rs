//! Warnings for Laravel helper calls (`env`, `config`, `route`, `view`, `__`) whose key provably resolves to nothing.

// Reached only through the server, which is not built for wasm.
#![cfg_attr(target_family = "wasm", allow(dead_code))]

use std::ops::ControlFlow;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{Expr, ExprKind};
use tower_lsp_server::ls_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range, Uri};

use crate::analysis::diagnostics::PHP_LSP_SOURCE;
use crate::document::ast::ParsedDoc;

use super::LaravelIndex;

const UNDEFINED_ENV_KEY: &str = "UndefinedEnvKey";
const UNDEFINED_CONFIG_KEY: &str = "UndefinedConfigKey";
const UNDEFINED_ROUTE: &str = "UndefinedRoute";
const UNDEFINED_VIEW: &str = "UndefinedView";
const UNDEFINED_TRANSLATION_KEY: &str = "UndefinedTranslationKey";

struct Call {
    name: String,
    key: String,
    has_second_arg: bool,
    range: Range,
}

const CALL_NAMES: &[&str] = &["env", "config", "route", "view", "__", "trans"];

fn collect_calls(doc: &ParsedDoc) -> Vec<Call> {
    struct Collector<'a> {
        doc: &'a ParsedDoc,
        out: Vec<Call>,
    }
    impl<'arena, 'src> Visitor<'arena, 'src> for Collector<'_> {
        type Break = ();
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            if let ExprKind::FunctionCall(f) = &expr.kind
                && let ExprKind::Name(name) = &f.name.kind
                && let name = name.to_string_repr().to_ascii_lowercase()
                && CALL_NAMES.contains(&name.as_str())
                && let Some(arg) = f.args.first()
                && arg.name.is_none()
                && !arg.is_spread()
                && let Some(value) = arg.value()
                && let ExprKind::String(s) = &value.kind
                && value.span.end > value.span.start + 1
            {
                let sv = self.doc.view();
                self.out.push(Call {
                    name,
                    key: s.to_string_lossy().into_owned(),
                    has_second_arg: f.args.len() > 1,
                    range: Range {
                        start: sv.position_of(value.span.start + 1),
                        end: sv.position_of(value.span.end - 1),
                    },
                });
            }
            walk_expr(self, expr)
        }
    }
    let mut collector = Collector {
        doc,
        out: Vec::new(),
    };
    for stmt in doc.program().stmts.iter() {
        let _ = collector.visit_stmt(stmt);
    }
    collector.out
}

/// Warnings for every helper call in `doc` whose key is provably undefined.
pub fn missing_key_diagnostics(
    doc: &ParsedDoc,
    uri: &Uri,
    laravel: &LaravelIndex,
) -> Vec<Diagnostic> {
    if !laravel.is_laravel {
        return Vec::new();
    }
    // Config files read optional variables with a bare `env('KEY')` by design.
    let in_config_dir = laravel.root.as_ref().is_some_and(|root| {
        uri.to_file_path()
            .is_some_and(|path| path.starts_with(root.join("config")))
    });
    collect_calls(doc)
        .into_iter()
        .filter(|call| !(in_config_dir && call.name == "env"))
        .filter_map(|call| {
            let (code, message) = check(&call, laravel)?;
            Some(Diagnostic {
                range: call.range,
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String(code.to_string())),
                source: Some(PHP_LSP_SOURCE.to_string()),
                message,
                ..Default::default()
            })
        })
        .collect()
}

fn check(call: &Call, laravel: &LaravelIndex) -> Option<(&'static str, String)> {
    let key = call.key.as_str();
    if key.is_empty() || key.contains("::") || key.contains('$') {
        return None;
    }
    match call.name.as_str() {
        "env" => {
            (!call.has_second_arg && laravel.env.has_source() && laravel.env.get(key).is_none())
                .then(|| {
                    (
                        UNDEFINED_ENV_KEY,
                        format!("Environment variable '{key}' is not defined in .env"),
                    )
                })
        }
        "config" => laravel.config.is_missing(key).then(|| {
            (
                UNDEFINED_CONFIG_KEY,
                format!("Config key '{key}' is not defined"),
            )
        }),
        "route" => (laravel.routes.names().next().is_some() && laravel.routes.get(key).is_none())
            .then(|| (UNDEFINED_ROUTE, format!("Route '{key}' is not defined"))),
        "view" => (laravel.views.names().next().is_some()
            && !key.contains('/')
            && laravel.views.get(key).is_none())
        .then(|| (UNDEFINED_VIEW, format!("View '{key}' does not exist"))),
        "__" | "trans" => laravel.translations.is_missing(key).then(|| {
            (
                UNDEFINED_TRANSLATION_KEY,
                format!("Translation key '{key}' is not defined"),
            )
        }),
        _ => None,
    }
}
