/// Code action: "Create route" — offered on a `route('name')` call whose
/// name doesn't resolve against the workspace's `RouteIndex`.
///
/// Applies Laravel's `{resource}.{action}` naming convention
/// (`posts.show` → `PostsController::show`) to scaffold a route registration
/// in `routes/web.php`, referencing an existing controller method (appending
/// a stub method if the controller exists but the method doesn't) or falling
/// back to an inline closure when no controller can be conventionally
/// derived or found. Deliberately never creates a new *file* — this
/// codebase's `WorkspaceEdit` handling only exercises the plain `changes`
/// map (see `tests/common/render.rs::canonicalize_workspace_edit`), not
/// `documentChanges` resource operations, so a missing controller gets a
/// closure stub instead of a generated controller file.
use std::collections::HashMap;
use std::path::Path;

use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::ParsedDoc;
use crate::document::document_store::DocumentStore;
use crate::laravel::LaravelIndex;
use crate::laravel::route_scaffold::resource_and_action;

pub fn unknown_route_actions(
    doc: &ParsedDoc,
    position: Position,
    laravel: &LaravelIndex,
    laravel_root: Option<&Path>,
    docs: &DocumentStore,
) -> Vec<CodeActionOrCommand> {
    if !laravel.is_laravel {
        return Vec::new();
    }
    let Some(root) = laravel_root else {
        return Vec::new();
    };
    let Some((route_name, _)) = crate::laravel::route_call_at(doc, position) else {
        return Vec::new();
    };
    if laravel.routes.get(&route_name).is_some() {
        return Vec::new();
    }

    let routes_path = root.join("routes").join("web.php");
    let Ok(routes_text) = std::fs::read_to_string(&routes_path) else {
        return Vec::new();
    };
    let Some(routes_uri) = Uri::from_file_path(&routes_path) else {
        return Vec::new();
    };

    let uri_path = format!("/{}", route_name.replace('.', "/"));
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();

    let scaffolded_controller = resource_and_action(&route_name).and_then(|(resource, action)| {
        let controller = format!("{resource}Controller");
        find_controller_method_site(docs, &controller, &action).map(|site| (site, action))
    });

    match scaffolded_controller {
        Some((site, action)) => {
            let route_line = format!(
                "Route::get('{uri_path}', [\\{fqn}::class, '{action}'])->name('{route_name}');",
                fqn = site.fqn
            );
            append_route_line(&mut changes, &routes_uri, &routes_text, &route_line);

            if !site.has_method {
                let insert_pos = Position {
                    line: site.closing_brace_line,
                    character: 0,
                };
                changes.entry(site.uri).or_default().push(TextEdit {
                    range: Range {
                        start: insert_pos,
                        end: insert_pos,
                    },
                    new_text: format!("\n    public function {action}()\n    {{\n    }}\n"),
                });
            }
        }
        None => {
            let route_line = format!(
                "Route::get('{uri_path}', function () {{\n    // TODO: implement '{route_name}'\n}})->name('{route_name}');"
            );
            append_route_line(&mut changes, &routes_uri, &routes_text, &route_line);
        }
    }

    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Create route '{route_name}'"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })]
}

/// Appends `line` to the end of `routes_text`, adding a leading newline only
/// when the file doesn't already end with one.
fn append_route_line(
    changes: &mut HashMap<Uri, Vec<TextEdit>>,
    routes_uri: &Uri,
    routes_text: &str,
    line: &str,
) {
    let doc = crate::analysis::diagnostics::parse_document_no_diags(routes_text);
    let end_pos = doc.view().position_of(routes_text.len() as u32);
    let new_text = if routes_text.ends_with('\n') {
        format!("{line}\n")
    } else {
        format!("\n{line}\n")
    };
    changes
        .entry(routes_uri.clone())
        .or_default()
        .push(TextEdit {
            range: Range {
                start: end_pos,
                end: end_pos,
            },
            new_text,
        });
}

struct ControllerSite {
    uri: Uri,
    fqn: String,
    has_method: bool,
    closing_brace_line: u32,
}

/// Finds `controller_name` anywhere in the workspace (via a text-prefilter
/// scan, same mechanism `implement_action`'s deferred generator uses) and
/// reports its FQN, whether it already declares `method_name`, and the line
/// to insert a new method before if not.
fn find_controller_method_site(
    docs: &DocumentStore,
    controller_name: &str,
    method_name: &str,
) -> Option<ControllerSite> {
    for (uri, doc) in docs.docs_for_scan_mentioning(&[controller_name.to_string()]) {
        if let Some((fqn, body_end, has_method)) =
            find_class(&doc.program().stmts, controller_name, method_name, None)
        {
            let closing_brace_line = doc.view().position_of(body_end.saturating_sub(1)).line;
            return Some(ControllerSite {
                uri,
                fqn,
                has_method,
                closing_brace_line,
            });
        }
    }
    None
}

/// Returns `(fqn, body_span_end, has_method)` for the first class named
/// `class_name` found in `stmts`, recursing into braced namespaces and
/// tracking unbraced `namespace Foo;` context across sibling statements —
/// same shape as `FileIndex::extract`'s own namespace handling.
fn find_class<'a>(
    stmts: &[Stmt<'a, 'a>],
    class_name: &str,
    method_name: &str,
    namespace: Option<&str>,
) -> Option<(String, u32, bool)> {
    let mut cur_ns: Option<String> = namespace.map(|s| s.to_owned());
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Namespace(ns) => {
                let ns_name = ns.name.as_ref().map(|n| n.to_string_repr().into_owned());
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        if let Some(found) =
                            find_class(&inner.stmts, class_name, method_name, ns_name.as_deref())
                        {
                            return Some(found);
                        }
                    }
                    NamespaceBody::Simple => cur_ns = ns_name,
                }
            }
            StmtKind::Class(c) => {
                let Some(name) = c.name else { continue };
                if name.or_error() != class_name {
                    continue;
                }
                let fqn = match &cur_ns {
                    Some(ns) => format!("{ns}\\{class_name}"),
                    None => class_name.to_string(),
                };
                let has_method = c.body.members.iter().any(|m| {
                    matches!(&m.kind, ClassMemberKind::Method(method) if method.name == method_name)
                });
                return Some((fqn, c.body.span.end, has_method));
            }
            _ => {}
        }
    }
    None
}
