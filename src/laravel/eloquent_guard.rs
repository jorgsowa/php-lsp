//! Flags `Model::create([...])` call sites against Eloquent models that
//! explicitly disable Laravel's mass-assignment protection
//! (`protected $guarded = [];`).
//!
//! Deliberately does **not** flag models that simply omit `$fillable`/
//! `$guarded` — Laravel's own default there (`Model::$guarded = ['*']`) is
//! fully guarded, so treating "missing" as risky would be a false positive.
//! Only an explicit empty `$guarded` array is the real footgun.

use std::collections::HashSet;
use std::ops::ControlFlow;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{ClassMemberKind, Expr, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp_server::ls_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range, Uri};

use crate::analysis::diagnostics::PHP_LSP_SOURCE;
use crate::db::workspace_index::WorkspaceIndexData;
use crate::document::ast::SourceView;
use crate::document::document_store::DocumentStore;

const ELOQUENT_MODEL_FQN: &str = "Illuminate\\Database\\Eloquent\\Model";
const DIAGNOSTIC_CODE: &str = "UnguardedMassAssignment";
/// Guards against pathological/circular `extends` chains.
const MAX_HIERARCHY_DEPTH: usize = 64;

/// Diagnostics for every `ClassName::create([...])` call in `uri`'s file
/// where `ClassName` transitively extends `Illuminate\Database\Eloquent\Model`
/// and has `protected $guarded = [];`. Empty for non-Laravel workspaces or
/// files with no such call sites.
pub fn unguarded_model_diagnostics(
    docs: &DocumentStore,
    uri: &Uri,
    is_laravel: bool,
) -> Vec<Diagnostic> {
    if !is_laravel {
        return Vec::new();
    }
    let Some(doc) = docs.get_doc_salsa(uri) else {
        return Vec::new();
    };
    let wi = docs.get_workspace_index_salsa();
    let mut visitor = CreateCallVisitor {
        sv: doc.view(),
        uri,
        docs,
        wi: &wi,
        out: Vec::new(),
    };
    for stmt in doc.program().stmts.iter() {
        let _ = visitor.visit_stmt(stmt);
    }
    visitor.out
}

struct CreateCallVisitor<'a> {
    sv: SourceView<'a>,
    uri: &'a Uri,
    docs: &'a DocumentStore,
    wi: &'a WorkspaceIndexData,
    out: Vec<Diagnostic>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for CreateCallVisitor<'_> {
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::StaticMethodCall(s) = &expr.kind
            && is_ident(s.method, "create")
            && let ExprKind::Identifier(class_ident) = &s.class.kind
            && matches!(
                s.args
                    .first()
                    .and_then(|a| a.value.as_ref())
                    .map(|v| &v.kind),
                Some(ExprKind::Array(_))
            )
        {
            let class_name = class_ident.as_str().to_string();
            if let Some(GuardVerdict::Unguarded) =
                classify_mass_assignment(&class_name, self.uri, self.docs, self.wi)
            {
                self.out.push(diagnostic_for_call(
                    self.sv,
                    s.class.span.start,
                    s.method.span.end,
                    &class_name,
                ));
            }
        }
        walk_expr(self, expr)
    }
}

fn is_ident(expr: &Expr<'_, '_>, name: &str) -> bool {
    matches!(&expr.kind, ExprKind::Identifier(n) if n.eq_ignore_ascii_case(name))
}

fn diagnostic_for_call(
    sv: SourceView<'_>,
    class_span_start: u32,
    method_span_end: u32,
    class_name: &str,
) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: sv.position_of(class_span_start),
            end: sv.position_of(method_span_end),
        },
        severity: Some(DiagnosticSeverity::WARNING),
        source: Some(PHP_LSP_SOURCE.to_string()),
        code: Some(NumberOrString::String(DIAGNOSTIC_CODE.to_string())),
        message: format!(
            "Mass assignment via {class_name}::create() is unguarded \u{2014} {class_name} sets `$guarded = []`, disabling Laravel's mass-assignment protection"
        ),
        ..Default::default()
    }
}

enum GuardVerdict {
    /// A class in the hierarchy declares `$guarded = [];` before any
    /// `$fillable`/non-empty `$guarded` override closer to the model.
    Unguarded,
    /// Either `$fillable` is declared, `$guarded` is declared non-empty, or
    /// neither is declared anywhere (Laravel's own safe default applies).
    Guarded,
}

/// Walks `class_name_written`'s `extends` chain (resolving each hop's
/// as-written name through *that hop's own declaring file*, since a
/// grandparent class's `use` imports are unrelated to the child's) looking
/// for the first `$fillable`/`$guarded` declaration, then keeps walking to
/// confirm the chain actually reaches `Illuminate\Database\Eloquent\Model`
/// before trusting that verdict. Returns `None` when the class isn't
/// resolvable in the workspace or the chain doesn't reach `Model` at all —
/// callers treat `None` as "don't flag" to avoid false positives on
/// unrelated classes that happen to declare a `$guarded` property.
fn classify_mass_assignment(
    class_name_written: &str,
    from_uri: &Uri,
    docs: &DocumentStore,
    wi: &WorkspaceIndexData,
) -> Option<GuardVerdict> {
    let mut current_name = class_name_written.to_string();
    let mut current_uri = from_uri.clone();
    let mut verdict: Option<GuardVerdict> = None;
    let mut visited = HashSet::new();

    for _ in 0..MAX_HIERARCHY_DEPTH {
        let fqn = docs
            .get_index_salsa(&current_uri)
            .map(|fi| fi.resolve_name_to_fqn(&current_name))
            .unwrap_or_else(|| current_name.trim_start_matches('\\').to_string());

        if fqn.eq_ignore_ascii_case(ELOQUENT_MODEL_FQN) {
            return Some(verdict.unwrap_or(GuardVerdict::Guarded));
        }
        if !visited.insert(fqn.to_ascii_lowercase()) {
            return None;
        }

        let cr = docs.resolve_class_ref(wi, &fqn)?;
        let (decl_uri, cls) = wi.at(cr)?;

        if verdict.is_none() {
            if cls.properties.iter().any(|p| p.name.as_ref() == "fillable") {
                verdict = Some(GuardVerdict::Guarded);
            } else if cls.properties.iter().any(|p| p.name.as_ref() == "guarded") {
                let empty = property_default_is_empty_array(decl_uri, &cls.name, docs);
                verdict = Some(if empty {
                    GuardVerdict::Unguarded
                } else {
                    GuardVerdict::Guarded
                });
            }
        }

        let parent = cls.parent.as_ref()?;
        current_name = parent.to_string();
        current_uri = decl_uri.clone();
    }
    None
}

/// `FileIndex`/`ClassDef` doesn't carry a property's default-value
/// expression, so this re-parses the declaring file's AST to find
/// `class_name`'s `prop_name` property and inspect its default.
fn property_default_is_empty_array(uri: &Uri, class_name: &str, docs: &DocumentStore) -> bool {
    let Some(doc) = docs.get_doc_salsa(uri) else {
        return false;
    };
    find_property_default_is_empty(&doc.program().stmts, class_name, "guarded").unwrap_or(false)
}

fn find_property_default_is_empty(
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    prop_name: &str,
) -> Option<bool> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let Some(name) = c.name else { continue };
                if name.or_error() != class_name {
                    continue;
                }
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Property(p) = &member.kind
                        && p.name.or_error() == prop_name
                    {
                        return Some(matches!(
                            &p.default,
                            Some(Expr { kind: ExprKind::Array(elems), .. }) if elems.is_empty()
                        ));
                    }
                }
                return Some(false);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) =
                        find_property_default_is_empty(&inner.stmts, class_name, prop_name)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}
