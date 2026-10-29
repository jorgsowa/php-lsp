use std::ops::ControlFlow;
use std::sync::Arc;

use php_ast::visitor::{Visitor, walk_expr, walk_stmt};
use php_ast::{
    ClassMember, ClassMemberKind, EnumMemberKind, Expr, ExprKind, MethodDecl, Param, Span, Stmt,
    StmtKind,
};
use tower_lsp_server::ls_types::{Location, Position, Range, Uri};

use super::walk::collect_var_refs_in_scope;
use crate::document::ast::{ParsedDoc, SourceView};
use crate::text::word_at_position;
use crate::types::resolve::{Container, Declaration, resolve_declaration};

/// Find the definition of the symbol under `position`.
/// Searches the current document first, then `other_docs` for cross-file resolution.
pub fn goto_definition(
    uri: &Uri,
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[(Uri, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Location> {
    let word = word_at_position(source, position)?;

    // For $variable, find the first occurrence in scope (= the definition/assignment).
    let sv = doc.view();
    if word.starts_with('$') {
        let bare = word.trim_start_matches('$');
        let byte_off = sv.byte_of_position(position) as usize;
        let mut spans = Vec::new();
        collect_var_refs_in_scope(&doc.program().stmts, bare, byte_off, &mut spans);
        if let Some((span, _)) = spans.into_iter().min_by_key(|(s, _)| s.start) {
            // Promoted property parameters include a visibility keyword before the
            // type and name (`private Database $db`); keep the full span so the cursor
            // lands on the complete declaration. Regular typed params (`int $x`) have
            // span.start at the type — narrow to just $var_name instead.
            let src_at_start = source.get(span.start as usize..).unwrap_or("");
            let is_promoted = src_at_start.starts_with("private ")
                || src_at_start.starts_with("public ")
                || src_at_start.starts_with("protected ")
                || src_at_start.starts_with("readonly ");
            let range = if is_promoted {
                Range {
                    start: sv.position_of(span.start),
                    end: sv.position_of(span.end),
                }
            } else {
                let name_with_sigil = format!("${bare}");
                let precise_start =
                    crate::document::ast::str_offset_in_range(source, span, &name_with_sigil)
                        .unwrap_or(span.start);
                Range {
                    start: sv.position_of(precise_start),
                    end: sv.position_of(precise_start + name_with_sigil.len() as u32),
                }
            };
            return Some(Location {
                uri: uri.clone(),
                range,
            });
        }
    }

    if let Some(range) = resolve_definition_range(sv, &doc.program().stmts, &word) {
        return Some(Location {
            uri: uri.clone(),
            range,
        });
    }

    for (other_uri, other_doc) in other_docs {
        let other_sv = other_doc.view();
        if let Some(range) = resolve_definition_range(other_sv, &other_doc.program().stmts, &word) {
            return Some(Location {
                uri: other_uri.clone(),
                range,
            });
        }
    }

    None
}

/// Search an AST for a declaration named `name`, returning its selection range.
/// Used by the PSR-4 fallback in the backend after resolving a class to a file.
pub fn find_declaration_range(doc: &ParsedDoc, name: &str) -> Option<Range> {
    let sv = doc.view();
    resolve_definition_range(sv, &doc.program().stmts, name)
}

/// Resolve `word` to a declaration in `stmts` and return its precise name range.
fn resolve_definition_range(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    word: &str,
) -> Option<Range> {
    // Definition resolves every declaration kind *except* enum constants
    // (which the original walker never matched).
    let decl = resolve_declaration(stmts, word, &|d| {
        !matches!(
            d,
            Declaration::ClassConst {
                container: Container::Enum,
                ..
            }
        )
    })?;
    Some(definition_name_range(sv, &decl))
}

fn definition_name_range(sv: SourceView<'_>, decl: &Declaration<'_>) -> Range {
    sv.name_range_in_span(decl.name(), decl.span())
}

/// The callee name offset and label of the named argument whose label contains `offset`.
pub fn named_arg_at(doc: &ParsedDoc, offset: u32) -> Option<(u32, String)> {
    struct Finder {
        offset: u32,
        found: Option<(u32, String)>,
    }

    impl<'arena, 'src> Visitor<'arena, 'src> for Finder {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            let call = match &expr.kind {
                ExprKind::FunctionCall(c) => Some((c.name.span, &c.args)),
                ExprKind::MethodCall(m) | ExprKind::NullsafeMethodCall(m) => {
                    Some((m.method.span, &m.args))
                }
                ExprKind::StaticMethodCall(s) => Some((s.method.span, &s.args)),
                ExprKind::New(n) => Some((n.class.span, &n.args)),
                _ => None,
            };
            if let Some((callee, args)) = call
                && let Some(label) = args.iter().filter_map(|a| a.name.as_ref()).find(|n| {
                    let span = n.span();
                    span.start <= self.offset && self.offset < span.end
                })
            {
                self.found = Some((callee.start, label.to_string_repr().into_owned()));
                return ControlFlow::Break(());
            }
            walk_expr(self, expr)
        }
    }

    let mut finder = Finder {
        offset,
        found: None,
    };
    let _ = finder.visit_program(doc.program());
    finder.found
}

/// The `$label` parameter of the callable whose name sits at `decl_offset`; a class
/// name there means its constructor.
pub fn param_range_of_declaration(doc: &ParsedDoc, decl_offset: u32, label: &str) -> Option<Range> {
    struct Finder<'a> {
        offset: u32,
        label: &'a str,
        found: Option<Span>,
    }

    impl Finder<'_> {
        fn contains(&self, span: Span) -> bool {
            span.start <= self.offset && self.offset < span.end
        }

        fn take_param(&mut self, params: &[Param<'_, '_>]) -> ControlFlow<()> {
            self.found = params.iter().find(|p| p.name == self.label).map(|p| p.span);
            ControlFlow::Break(())
        }

        fn members(&mut self, methods: &[(&MethodDecl<'_, '_>, Span)]) -> ControlFlow<()> {
            let target = methods
                .iter()
                .find(|(_, span)| self.contains(*span))
                .or_else(|| {
                    methods
                        .iter()
                        .find(|(m, _)| m.name.as_str() == Some("__construct"))
                });
            match target {
                Some((m, _)) => self.take_param(&m.params),
                None => ControlFlow::Break(()),
            }
        }
    }

    fn class_methods<'m, 'arena, 'src>(
        members: &'m [ClassMember<'arena, 'src>],
    ) -> Vec<(&'m MethodDecl<'arena, 'src>, Span)> {
        members
            .iter()
            .filter_map(|m| match &m.kind {
                ClassMemberKind::Method(decl) => Some((decl, m.span)),
                _ => None,
            })
            .collect()
    }

    impl<'arena, 'src> Visitor<'arena, 'src> for Finder<'_> {
        fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
            if !self.contains(stmt.span) {
                return ControlFlow::Continue(());
            }
            match &stmt.kind {
                StmtKind::Function(f) if !f.body.stmts.iter().any(|s| self.contains(s.span)) => {
                    self.take_param(&f.params)
                }
                StmtKind::Class(c) => self.members(&class_methods(&c.body.members)),
                StmtKind::Interface(i) => self.members(&class_methods(&i.body.members)),
                StmtKind::Trait(t) => self.members(&class_methods(&t.body.members)),
                StmtKind::Enum(e) => {
                    let methods: Vec<_> = e
                        .body
                        .members
                        .iter()
                        .filter_map(|m| match &m.kind {
                            EnumMemberKind::Method(decl) => Some((decl, m.span)),
                            _ => None,
                        })
                        .collect();
                    self.members(&methods)
                }
                _ => walk_stmt(self, stmt),
            }
        }
    }

    let mut finder = Finder {
        offset: decl_offset,
        label,
        found: None,
    };
    let _ = finder.visit_program(doc.program());
    let span = finder.found?;
    let name = format!("${label}");
    let start =
        crate::document::ast::str_offset_in_range(doc.source(), span, &name).unwrap_or(span.start);
    Some(
        doc.view()
            .range_of(Span::new(start, start + name.len() as u32)),
    )
}
