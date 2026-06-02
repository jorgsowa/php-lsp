use std::cell::OnceCell;
use std::sync::Arc;

use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Url};

use crate::ast::ParsedDoc;
use crate::docblock::find_docblock;
use crate::resolve::{Declaration, resolve_declaration};
use crate::symbol_map::{SymbolMap, is_hoverable_kind};
use crate::type_map::TypeMap;
use crate::util::{fqn_short_name, is_php_builtin, php_doc_url, word_at_position, word_range_at};

use super::closures::closure_hover;
use super::formatting::{declaration_signature, wrap_php};
use super::members::{
    find_property_info, resolve_method_docblock, scan_class_const_of_class,
    scan_enum_case_of_class, scan_method_of_class,
};
use super::named_args::{extract_named_arg_callee, is_named_arg_at, named_arg_hover_value};
use super::parsing::{extract_static_class_before_cursor, resolve_use_alias};

/// Hover handles every declaration kind except properties (covered by the
/// dedicated mir-primary path) and promoted parameters.
fn is_hoverable(decl: &Declaration<'_>) -> bool {
    !matches!(
        decl,
        Declaration::Property { .. } | Declaration::PromotedParam { .. }
    )
}

pub fn hover_info(
    source: &str,
    doc: &ParsedDoc,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    position: Position,
    other_docs: &[(Url, Arc<ParsedDoc>)],
) -> Option<Hover> {
    hover_at(source, doc, analysis, other_docs, position)
}

/// Indexed variant: uses pre-computed [`SymbolMap`]s for the cross-file
/// declaration lookup (path 4/5), eliminating repeated AST walks on stable
/// files. All other paths (named-arg hover, mir-member hover, static-prop
/// hover) still use `other_docs` since they require full AST traversal.
pub fn hover_info_with_maps(
    source: &str,
    doc: &ParsedDoc,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    position: Position,
    other_docs: &[(Url, Arc<ParsedDoc>)],
    other_maps: &[(Url, Arc<SymbolMap>)],
) -> Option<Hover> {
    hover_at_with_maps(source, doc, analysis, other_docs, other_maps, position)
}

/// Full hover implementation.
pub fn hover_at(
    source: &str,
    doc: &ParsedDoc,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    other_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Hover> {
    let hover_range = word_range_at(source, position);

    // Hover on a `use` line shows the full FQN — check before word_at since the
    // cursor may be past the last word boundary.
    if let Some(line_text) = source.lines().nth(position.line as usize) {
        let trimmed = line_text.trim();
        if trimmed.starts_with("use ") {
            let (prefix, content) = if trimmed.starts_with("use function ") {
                (
                    "use function ",
                    trimmed.strip_prefix("use function ").unwrap_or(""),
                )
            } else if trimmed.starts_with("use const ") {
                (
                    "use const ",
                    trimmed.strip_prefix("use const ").unwrap_or(""),
                )
            } else {
                ("use ", trimmed.strip_prefix("use ").unwrap_or(""))
            };
            let fqn = content.trim_end_matches(';').trim();
            if !fqn.is_empty() {
                let maybe_word = word_at_position(source, position);
                let alias = fqn_short_name(fqn);
                let matches = match &maybe_word {
                    Some(w) => w == alias || fqn.contains(w.as_str()),
                    None => true,
                };
                if matches {
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("`{}{};`", prefix, fqn),
                        }),
                        range: hover_range,
                    });
                }
            }
        }
    }

    let word = word_at_position(source, position)?;

    // Keyword hover — must be checked before the static-access path so that
    // `static::foo()` still falls through.  The `::` guard prevents this branch
    // from firing for `Class::static` or `self::method`.
    if let Some(line_text) = source.lines().nth(position.line as usize)
        && extract_static_class_before_cursor(line_text, position.character as usize).is_none()
    {
        let keyword_doc: Option<&str> = match word.as_str() {
            "match" => Some("`match` — evaluates an expression against a set of arms (PHP 8.0)"),
            "null" => Some("`null` — the null value; a variable has no value"),
            "true" => Some("`true` — boolean true"),
            "false" => Some("`false` — boolean false"),
            "abstract" => Some(
                "`abstract` — declares an abstract class or method that must be implemented by a subclass",
            ),
            "readonly" => {
                Some("`readonly` — property or class that can only be initialised once (PHP 8.1)")
            }
            "yield" => Some("`yield` — produces a value from a generator function"),
            "never" => Some(
                "`never` — return type indicating the function always throws or exits (PHP 8.1)",
            ),
            "throw" => {
                Some("`throw` — throws an exception; can be used as an expression (PHP 8.0)")
            }
            _ => None,
        };
        if let Some(doc_str) = keyword_doc {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: doc_str.to_string(),
                }),
                range: hover_range,
            });
        }
    }

    // TypeMap is expensive; build lazily and reuse across branches (including
    // named-arg resolution below, which also needs a TypeMap as a mir fallback).
    let type_map_cell: OnceCell<TypeMap> = OnceCell::new();
    let type_map =
        || type_map_cell.get_or_init(|| TypeMap::from_doc_at_position(doc, None, position));

    // Named argument hover: `foo(label: $x)` — hovering the label shows the
    // parameter type and description.
    if let Some(line_text) = source.lines().nth(position.line as usize)
        && !word.starts_with('$')
        && is_named_arg_at(line_text, position.character as usize, &word)
        && let Some(callee) = extract_named_arg_callee(line_text, position.character as usize)
        && let Some(value) = named_arg_hover_value(
            source,
            doc,
            other_docs,
            position,
            &callee,
            &word,
            analysis,
            &type_map_cell,
        )
    {
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: hover_range,
        });
    }

    // Hover on $variable shows its inferred type. mir-primary: when mir
    // recorded a class-typed symbol, show its full rendering (generics, unions,
    // narrowing — e.g. `Collection<User>`, `Foo|Bar`). Scalar/`mixed` types are
    // skipped so we fall through to the TypeMap short-name fallback (covers
    // bindings where mir records no class-typed symbol).
    if word.starts_with('$') {
        if let Some(ty) = analysis.and_then(|a| {
            let off =
                word_range_at(source, position).map(|r| doc.view().byte_of_position(r.start))?;
            crate::type_query::type_at_offset(a, off)
        }) && !crate::type_query::class_names(ty).is_empty()
        {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("`{word}` `{ty}`"),
                }),
                range: hover_range,
            });
        }
        if let Some(class_name) = type_map().get(&word) {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("`{}` `{}`", word, class_name),
                }),
                range: hover_range,
            });
        }
    }

    // Hover on ClassName::$staticProp — word begins with '$' but is not a local var.
    if word.starts_with('$')
        && let Some(line_text) = source.lines().nth(position.line as usize)
        && let Some(class_name) =
            extract_static_class_before_cursor(line_text, position.character as usize)
    {
        let prop_name = word.trim_start_matches('$');
        let effective_class = if class_name == "self" || class_name == "static" {
            crate::type_map::enclosing_class_at(source, doc, position).unwrap_or(class_name.clone())
        } else {
            class_name.clone()
        };
        for d in std::iter::once(doc).chain(other_docs.iter().map(|(_, d)| d.as_ref())) {
            if let Some((modifiers, type_str, db)) =
                find_property_info(d, &effective_class, prop_name)
            {
                let sig = format!(
                    "(property) {}{}::${}{}",
                    modifiers,
                    effective_class,
                    prop_name,
                    if type_str.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", type_str)
                    }
                );
                let mut value = wrap_php(&sig);
                if let Some(doc) = db {
                    let md = doc.to_markdown();
                    if !md.is_empty() {
                        value.push_str("\n\n---\n\n");
                        value.push_str(&md);
                    }
                }
                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value,
                    }),
                    range: hover_range,
                });
            }
        }
    }

    // mir-primary member hover: when mir recorded a resolved symbol at the
    // cursor position, use its class name directly instead of extracting the
    // receiver via TypeMap. This handles chains like `$a->b()->c()` where
    // TypeMap has no variable to look up.
    if !word.starts_with('$')
        && let Some(sym) = analysis.and_then(|a| {
            let off =
                word_range_at(source, position).map(|r| doc.view().byte_of_position(r.start))?;
            a.symbol_at(off)
        })
    {
        let mir_hover = mir_member_hover(sym, &word, doc, other_docs);
        if mir_hover.is_some() {
            return mir_hover.map(|value| Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: hover_range,
            });
        }
    }

    // Closure / arrow function hover: `function($x) {}` or `fn($x) => $x`.
    // Must run before `scan_statements` so the keyword doesn't fall through to
    // the named-function path (which won't find anything for an anonymous fn).
    if (word == "function" || word == "fn")
        && let Some(sig) = closure_hover(source, doc, position, &word)
    {
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: wrap_php(&sig),
            }),
            range: hover_range,
        });
    }

    // Resolve use-import aliases: `use Foo\Bar as Baz` — hovering on `Baz`
    // should show what `Bar` is.
    let all_stmts = &*doc.program().stmts as &[_];
    let resolved_word = resolve_use_alias(all_stmts, &word).unwrap_or_else(|| word.clone());

    // Search current document first, then cross-file (using resolved name).
    let found = resolve_declaration(&doc.program().stmts, &resolved_word, &is_hoverable)
        .and_then(|d| declaration_signature(&d, &resolved_word))
        .map(|sig| (sig, source, doc));
    let found = found.or_else(|| {
        for (_, other) in other_docs {
            if let Some(sig) =
                resolve_declaration(&other.program().stmts, &resolved_word, &is_hoverable)
                    .and_then(|d| declaration_signature(&d, &resolved_word))
            {
                return Some((sig, other.source(), other.as_ref()));
            }
        }
        None
    });

    if let Some((sig, _sig_source, sig_doc)) = found {
        let mut value = wrap_php(&sig);
        if let Some(db) = find_docblock(&sig_doc.program().stmts, &resolved_word) {
            let md = db.to_markdown();
            if !md.is_empty() {
                value.push_str("\n\n---\n\n");
                value.push_str(&md);
            }
        }
        if is_php_builtin(&resolved_word) {
            value.push_str(&format!(
                "\n\n[php.net documentation]({})",
                php_doc_url(&resolved_word)
            ));
        }
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: hover_range,
        });
    }

    // Fallback: built-in function with no user-defined counterpart.
    if is_php_builtin(&resolved_word) {
        let value = format!(
            "```php\nfunction {}()\n```\n\n[php.net documentation]({})",
            resolved_word,
            php_doc_url(&resolved_word)
        );
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: hover_range,
        });
    }

    // Hover on a built-in class name shows stub info.
    if let Some(stub) = crate::stubs::builtin_class_members(&resolved_word) {
        let method_names: Vec<&str> = stub
            .methods
            .iter()
            .filter(|(_, is_static)| !is_static)
            .map(|(n, _)| n.as_str())
            .take(8)
            .collect();
        let static_names: Vec<&str> = stub
            .methods
            .iter()
            .filter(|(_, is_static)| *is_static)
            .map(|(n, _)| n.as_str())
            .take(4)
            .collect();
        let mut lines = vec![format!("**{}** — built-in class", resolved_word)];
        if !method_names.is_empty() {
            lines.push(format!(
                "Methods: {}",
                method_names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !static_names.is_empty() {
            lines.push(format!(
                "Static: {}",
                static_names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(parent) = &stub.parent {
            lines.push(format!("Extends: `{parent}`"));
        }
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: lines.join("\n\n"),
            }),
            range: hover_range,
        });
    }

    None
}

/// Indexed variant of [`hover_at`]: replaces the cross-file `resolve_declaration`
/// walk with O(1) symbol map lookup. All other paths are identical.
fn hover_at_with_maps(
    source: &str,
    doc: &ParsedDoc,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    other_docs: &[(Url, Arc<ParsedDoc>)],
    other_maps: &[(Url, Arc<SymbolMap>)],
    position: Position,
) -> Option<Hover> {
    let hover_range = word_range_at(source, position);

    if let Some(line_text) = source.lines().nth(position.line as usize) {
        let trimmed = line_text.trim();
        if trimmed.starts_with("use ") {
            let (prefix, content) = if trimmed.starts_with("use function ") {
                (
                    "use function ",
                    trimmed.strip_prefix("use function ").unwrap_or(""),
                )
            } else if trimmed.starts_with("use const ") {
                (
                    "use const ",
                    trimmed.strip_prefix("use const ").unwrap_or(""),
                )
            } else {
                ("use ", trimmed.strip_prefix("use ").unwrap_or(""))
            };
            let fqn = content.trim_end_matches(';').trim();
            if !fqn.is_empty() {
                let maybe_word = word_at_position(source, position);
                let alias = fqn_short_name(fqn);
                let matches = match &maybe_word {
                    Some(w) => w == alias || fqn.contains(w.as_str()),
                    None => true,
                };
                if matches {
                    return Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("`{}{};`", prefix, fqn),
                        }),
                        range: hover_range,
                    });
                }
            }
        }
    }

    let word = word_at_position(source, position)?;

    if let Some(line_text) = source.lines().nth(position.line as usize)
        && extract_static_class_before_cursor(line_text, position.character as usize).is_none()
    {
        let keyword_doc: Option<&str> = match word.as_str() {
            "match" => Some("`match` — evaluates an expression against a set of arms (PHP 8.0)"),
            "null" => Some("`null` — the null value; a variable has no value"),
            "true" => Some("`true` — boolean true"),
            "false" => Some("`false` — boolean false"),
            "abstract" => Some(
                "`abstract` — declares an abstract class or method that must be implemented by a subclass",
            ),
            "readonly" => {
                Some("`readonly` — property or class that can only be initialised once (PHP 8.1)")
            }
            "yield" => Some("`yield` — produces a value from a generator function"),
            "never" => Some(
                "`never` — return type indicating the function always throws or exits (PHP 8.1)",
            ),
            "throw" => {
                Some("`throw` — throws an exception; can be used as an expression (PHP 8.0)")
            }
            _ => None,
        };
        if let Some(doc_str) = keyword_doc {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: doc_str.to_string(),
                }),
                range: hover_range,
            });
        }
    }

    let type_map_cell: OnceCell<TypeMap> = OnceCell::new();
    let type_map =
        || type_map_cell.get_or_init(|| TypeMap::from_doc_at_position(doc, None, position));

    if let Some(line_text) = source.lines().nth(position.line as usize)
        && !word.starts_with('$')
        && is_named_arg_at(line_text, position.character as usize, &word)
        && let Some(callee) = extract_named_arg_callee(line_text, position.character as usize)
        && let Some(value) = named_arg_hover_value(
            source,
            doc,
            other_docs,
            position,
            &callee,
            &word,
            analysis,
            &type_map_cell,
        )
    {
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: hover_range,
        });
    }

    if word.starts_with('$') {
        if let Some(ty) = analysis.and_then(|a| {
            let off =
                word_range_at(source, position).map(|r| doc.view().byte_of_position(r.start))?;
            crate::type_query::type_at_offset(a, off)
        }) && !crate::type_query::class_names(ty).is_empty()
        {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("`{word}` `{ty}`"),
                }),
                range: hover_range,
            });
        }
        if let Some(class_name) = type_map().get(&word) {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("`{}` `{}`", word, class_name),
                }),
                range: hover_range,
            });
        }
    }

    if word.starts_with('$')
        && let Some(line_text) = source.lines().nth(position.line as usize)
        && let Some(class_name) =
            extract_static_class_before_cursor(line_text, position.character as usize)
    {
        let prop_name = word.trim_start_matches('$');
        let effective_class = if class_name == "self" || class_name == "static" {
            crate::type_map::enclosing_class_at(source, doc, position).unwrap_or(class_name.clone())
        } else {
            class_name.clone()
        };
        for d in std::iter::once(doc).chain(other_docs.iter().map(|(_, d)| d.as_ref())) {
            if let Some((modifiers, type_str, db)) =
                find_property_info(d, &effective_class, prop_name)
            {
                let sig = format!(
                    "(property) {}{}::${}{}",
                    modifiers,
                    effective_class,
                    prop_name,
                    if type_str.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", type_str)
                    }
                );
                let mut value = wrap_php(&sig);
                if let Some(doc) = db {
                    let md = doc.to_markdown();
                    if !md.is_empty() {
                        value.push_str("\n\n---\n\n");
                        value.push_str(&md);
                    }
                }
                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value,
                    }),
                    range: hover_range,
                });
            }
        }
    }

    if !word.starts_with('$')
        && let Some(sym) = analysis.and_then(|a| {
            let off =
                word_range_at(source, position).map(|r| doc.view().byte_of_position(r.start))?;
            a.symbol_at(off)
        })
    {
        let mir_hover = mir_member_hover(sym, &word, doc, other_docs);
        if mir_hover.is_some() {
            return mir_hover.map(|value| Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: hover_range,
            });
        }
    }

    if (word == "function" || word == "fn")
        && let Some(sig) = closure_hover(source, doc, position, &word)
    {
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: wrap_php(&sig),
            }),
            range: hover_range,
        });
    }

    let all_stmts = &*doc.program().stmts as &[_];
    let resolved_word = resolve_use_alias(all_stmts, &word).unwrap_or_else(|| word.clone());

    // Current-doc: still uses the AST walker (doc is already in memory, fast).
    let current_doc_found =
        resolve_declaration(&doc.program().stmts, &resolved_word, &is_hoverable)
            .and_then(|d| declaration_signature(&d, &resolved_word))
            .map(|sig| {
                let doc_md = find_docblock(&doc.program().stmts, &resolved_word)
                    .map(|db| db.to_markdown())
                    .filter(|md| !md.is_empty());
                (sig, doc_md)
            });

    // Cross-file: O(1) symbol map lookup — no AST walk.
    let found = current_doc_found.or_else(|| {
        for (_, sym_map) in other_maps {
            if let Some(entry) = sym_map.lookup(&resolved_word, |e| is_hoverable_kind(e.kind))
                && let Some(sig) = &entry.signature
            {
                return Some((sig.clone(), entry.doc_markdown.clone()));
            }
        }
        None
    });

    if let Some((sig, doc_md)) = found {
        let mut value = wrap_php(&sig);
        if let Some(md) = doc_md
            && !md.is_empty()
        {
            value.push_str("\n\n---\n\n");
            value.push_str(&md);
        }
        if is_php_builtin(&resolved_word) {
            value.push_str(&format!(
                "\n\n[php.net documentation]({})",
                php_doc_url(&resolved_word)
            ));
        }
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: hover_range,
        });
    }

    if is_php_builtin(&resolved_word) {
        let value = format!(
            "```php\nfunction {}()\n```\n\n[php.net documentation]({})",
            resolved_word,
            php_doc_url(&resolved_word)
        );
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: hover_range,
        });
    }

    if let Some(stub) = crate::stubs::builtin_class_members(&resolved_word) {
        let method_names: Vec<&str> = stub
            .methods
            .iter()
            .filter(|(_, is_static)| !is_static)
            .map(|(n, _)| n.as_str())
            .take(8)
            .collect();
        let static_names: Vec<&str> = stub
            .methods
            .iter()
            .filter(|(_, is_static)| *is_static)
            .map(|(n, _)| n.as_str())
            .take(4)
            .collect();
        let mut lines = vec![format!("**{}** — built-in class", resolved_word)];
        if !method_names.is_empty() {
            lines.push(format!(
                "Methods: {}",
                method_names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !static_names.is_empty() {
            lines.push(format!(
                "Static: {}",
                static_names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(parent) = &stub.parent {
            lines.push(format!("Extends: `{parent}`"));
        }
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: lines.join("\n\n"),
            }),
            range: hover_range,
        });
    }

    None
}

/// Produce hover markdown for a member access resolved by mir. Returns the
/// hover value string (not the full `Hover` struct — the caller wraps it).
/// `None` means mir has no symbol here; the caller falls through to `resolve_declaration`.
fn mir_member_hover(
    sym: &mir_analyzer::ResolvedSymbol,
    word: &str,
    doc: &ParsedDoc,
    other_docs: &[(tower_lsp::lsp_types::Url, std::sync::Arc<ParsedDoc>)],
) -> Option<String> {
    let docs = || std::iter::once(doc).chain(other_docs.iter().map(|(_, d)| d.as_ref()));
    match &sym.kind {
        mir_analyzer::ReferenceKind::MethodCall { class, .. }
        | mir_analyzer::ReferenceKind::StaticCall { class, .. } => {
            let class_short = fqn_short_name(class);
            for d in docs() {
                if let Some(sig) = scan_method_of_class(&d.program().stmts, class_short, word) {
                    // Augment declared return type with mir's inferred type when richer.
                    let sig = augment_return_type(sig, &sym.resolved_type);
                    let mut value = wrap_php(&sig);
                    let all =
                        std::iter::once(doc).chain(other_docs.iter().map(|(_, d)| d.as_ref()));
                    if let Some(db) = resolve_method_docblock(all, class_short, word) {
                        let md = db.to_markdown();
                        if !md.is_empty() {
                            value.push_str("\n\n---\n\n");
                            value.push_str(&md);
                        }
                    }
                    return Some(value);
                }
            }
            None
        }
        mir_analyzer::ReferenceKind::PropertyAccess { class, property } => {
            let class_short = fqn_short_name(class);
            for d in docs() {
                if let Some((modifiers, declared_type, db)) =
                    find_property_info(d, class_short, property)
                {
                    // Use mir's resolved type when it's more specific than declared.
                    let type_str = augment_property_type(declared_type, &sym.resolved_type);
                    let sig = format!(
                        "(property) {}{}::${}{}",
                        modifiers,
                        class_short,
                        property,
                        if type_str.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", type_str)
                        }
                    );
                    let mut value = wrap_php(&sig);
                    if let Some(doc_block) = db {
                        let md = doc_block.to_markdown();
                        if !md.is_empty() {
                            value.push_str("\n\n---\n\n");
                            value.push_str(&md);
                        }
                    }
                    return Some(value);
                }
            }
            None
        }
        mir_analyzer::ReferenceKind::ConstantAccess { class, constant } => {
            let class_short = fqn_short_name(class);
            for d in docs() {
                if let Some(sig) =
                    scan_enum_case_of_class(&d.program().stmts, class_short, constant)
                {
                    return Some(wrap_php(&sig));
                }
                if let Some(sig) =
                    scan_class_const_of_class(&d.program().stmts, class_short, constant)
                {
                    return Some(wrap_php(&sig));
                }
            }
            None
        }
        _ => None,
    }
}

/// Override the return-type portion of a method signature string with mir's
/// inferred type, when mir provides concrete (non-mixed, non-void) info.
/// `sig` has the form `"Class::method(params): OldType"` or no return type.
fn augment_return_type(sig: String, resolved: &mir_analyzer::Type) -> String {
    let ty_str = format!("{resolved}");
    if matches!(ty_str.as_str(), "mixed" | "void" | "never" | "null") {
        return sig;
    }
    let Some(paren) = sig.rfind(')') else {
        return sig;
    };
    let rest = &sig[paren + 1..];
    if let Some(colon_pos) = rest.find(": ") {
        let declared = rest[colon_pos + 2..].trim();
        // static/self/parent are late-static-binding — mir resolves them to a
        // concrete class, losing the polymorphic semantics. Keep declared.
        if matches!(declared, "static" | "self" | "parent") {
            return sig;
        }
        format!("{}: {}", &sig[..paren + 1 + colon_pos], ty_str)
    } else {
        format!("{}: {}", sig, ty_str)
    }
}

/// Choose between the declared property type and mir's resolved type. Uses
/// mir when it adds information (non-mixed, non-void).
fn augment_property_type(declared: String, resolved: &mir_analyzer::Type) -> String {
    let ty_str = format!("{resolved}");
    if matches!(ty_str.as_str(), "mixed" | "void" | "never") {
        return declared;
    }
    ty_str
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::cursor;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn hover_on_function_name_returns_signature() {
        let (src, p) = cursor("<?php\nfunction g$0reet(string $name): string {}");
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, None, p, &[]);
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("function greet("),
                "expected function signature, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_class_name_returns_class_sig() {
        let (src, p) = cursor("<?php\nclass My$0Service {}");
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, None, p, &[]);
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("class MyService"),
                "expected class sig, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_unknown_word_returns_none() {
        let src = "<?php\n$unknown = 42;";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 2), &[]);
        assert!(result.is_none(), "expected None for unknown word");
    }

    #[test]
    fn hover_at_column_beyond_line_length_returns_none() {
        let src = "<?php\nfunction hi() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 999), &[]);
        assert!(result.is_none());
    }

    #[test]
    fn word_at_extracts_from_middle_of_identifier() {
        let (src, p) = cursor("<?php\nfunction greet$0User() {}");
        let word = word_at_position(&src, p);
        assert_eq!(word.as_deref(), Some("greetUser"));
    }

    #[test]
    fn hover_on_class_with_extends_shows_parent() {
        let src = "<?php\nclass Dog extends Animal {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 8), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("extends Animal"),
                "expected 'extends Animal', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_class_with_implements_shows_interfaces() {
        let src = "<?php\nclass Repo implements Countable, Serializable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 8), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("implements Countable, Serializable"),
                "expected implements list, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_trait_returns_trait_sig() {
        let src = "<?php\ntrait Loggable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 8), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("trait Loggable"),
                "expected 'trait Loggable', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_interface_returns_interface_sig() {
        let src = "<?php\ninterface Serializable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 12), &[]);
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("interface Serializable"),
                "expected interface sig, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn function_with_no_params_no_return_shows_no_colon() {
        let src = "<?php\nfunction init() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 10), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("function init()"),
                "expected 'function init()', got: {}",
                mc.value
            );
            assert!(
                !mc.value.contains(':'),
                "should not contain ':' when no return type, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_enum_returns_enum_sig() {
        let src = "<?php\nenum Suit {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 6), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("enum Suit"),
                "expected 'enum Suit', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_enum_with_implements_shows_interface() {
        let src = "<?php\nenum Status: string implements Stringable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 6), &[]);
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("implements Stringable"),
                "expected implements clause, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_enum_case_shows_case_sig() {
        let src = "<?php\nenum Status { case Active; case Inactive; }";
        let doc = ParsedDoc::parse(src.to_string());
        // "Active" starts at col 19: "enum Status { case Active;"
        let result = hover_info(src, &doc, None, pos(1, 21), &[]);
        assert!(result.is_some(), "expected hover on enum case");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("Status::Active"),
                "expected 'Status::Active', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn snapshot_hover_backed_enum_case_shows_value() {
        check_hover(
            "<?php\nenum Color: string { case Red = 'red'; }",
            pos(1, 27),
            expect![[r#"
                ```php
                case Color::Red = 'red'
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_enum_class_const() {
        check_hover(
            "<?php\nenum Suit { const int MAX = 4; }",
            pos(1, 22),
            expect![[r#"
                ```php
                const int MAX = 4
                ```"#]],
        );
    }

    #[test]
    fn hover_on_trait_method_returns_signature() {
        let src = "<?php\ntrait Loggable { public function log(string $msg): void {} }";
        let doc = ParsedDoc::parse(src.to_string());
        // "log" at "trait Loggable { public function log(" — col 33
        let result = hover_info(src, &doc, None, pos(1, 34), &[]);
        assert!(result.is_some(), "expected hover on trait method");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("function log("),
                "expected function sig, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn cross_file_hover_finds_class_in_other_doc() {
        use std::sync::Arc;
        let src = "<?php\n$x = new PaymentService();";
        let other_src = "<?php\nclass PaymentService { public function charge() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let other_doc = Arc::new(ParsedDoc::parse(other_src.to_string()));
        let uri = tower_lsp::lsp_types::Url::parse("file:///other.php").unwrap();
        let other_docs = vec![(uri, other_doc)];
        // Hover on "PaymentService" in line 1
        let result = hover_info(src, &doc, None, pos(1, 12), &other_docs);
        assert!(result.is_some(), "expected cross-file hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("PaymentService"),
                "expected 'PaymentService', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_variable_shows_type() {
        let src = "<?php\n$obj = new Mailer();\n$obj";
        let doc = ParsedDoc::parse(src.to_string());
        let h = hover_at(src, &doc, None, &[], pos(2, 2));
        assert!(h.is_some());
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("Mailer"), "hover on $obj should show Mailer");
    }

    #[test]
    fn hover_on_builtin_class_shows_stub_info() {
        let src = "<?php\n$pdo = new PDO('sqlite::memory:');\n$pdo->query('SELECT 1');";
        let doc = ParsedDoc::parse(src.to_string());
        let h = hover_at(src, &doc, None, &[], pos(1, 12));
        assert!(h.is_some(), "should hover on PDO");
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("PDO"), "hover should mention PDO");
    }

    #[test]
    fn hover_on_use_alias_shows_fqn() {
        let src = "<?php\nuse App\\Mail\\Mailer;\n$m = new Mailer();";
        let doc = ParsedDoc::parse(src.to_string());
        let h = hover_at(
            src,
            &doc,
            None,
            &[],
            Position {
                line: 1,
                character: 20,
            },
        );
        assert!(h.is_some());
        let text = match h.unwrap().contents {
            HoverContents::Markup(m) => m.value,
            _ => String::new(),
        };
        assert!(text.contains("App\\Mail\\Mailer"), "should show full FQN");
    }

    #[test]
    fn hover_unknown_symbol_returns_none() {
        // `unknownFunc` is not defined anywhere — hover should return None.
        let src = "<?php\nunknownFunc();";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 3), &[]);
        assert!(
            result.is_none(),
            "hover on undefined symbol should return None"
        );
    }

    #[test]
    fn hover_on_builtin_function_returns_signature() {
        // `strlen` is a built-in function; hovering should return a non-empty
        // string that contains "strlen".
        let src = "<?php\nstrlen('hello');";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, pos(1, 3), &[]);
        let h = result.expect("expected hover result for built-in 'strlen'");
        let text = match h.contents {
            HoverContents::Markup(mc) => mc.value,
            _ => String::new(),
        };
        assert!(
            !text.is_empty(),
            "hover on strlen should return non-empty content"
        );
        assert!(
            text.contains("strlen"),
            "hover content should contain 'strlen', got: {text}"
        );
    }

    // ── Snapshot tests ───────────────────────────────────────────────────────

    use expect_test::{Expect, expect};

    fn check_hover(src: &str, position: Position, expect: Expect) {
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, None, position, &[]);
        let actual = match result {
            Some(Hover {
                contents: HoverContents::Markup(mc),
                ..
            }) => mc.value,
            Some(_) => "(non-markup hover)".to_string(),
            None => "(no hover)".to_string(),
        };
        expect.assert_eq(&actual);
    }

    #[test]
    fn snapshot_hover_simple_function() {
        check_hover(
            "<?php\nfunction init() {}",
            pos(1, 10),
            expect![[r#"
                ```php
                function init()
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_function_with_return_type() {
        check_hover(
            "<?php\nfunction greet(string $name): string {}",
            pos(1, 10),
            expect![[r#"
                ```php
                function greet(string $name): string
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_class() {
        check_hover(
            "<?php\nclass MyService {}",
            pos(1, 8),
            expect![[r#"
                ```php
                class MyService
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_class_with_extends() {
        check_hover(
            "<?php\nclass Dog extends Animal {}",
            pos(1, 8),
            expect![[r#"
                ```php
                class Dog extends Animal
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_method() {
        check_hover(
            "<?php\nclass Calc { public function add(int $a, int $b): int {} }",
            pos(1, 32),
            expect![[r#"
                ```php
                public function add(int $a, int $b): int
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_trait() {
        check_hover(
            "<?php\ntrait Loggable {}",
            pos(1, 8),
            expect![[r#"
                ```php
                trait Loggable
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_interface() {
        check_hover(
            "<?php\ninterface Serializable {}",
            pos(1, 12),
            expect![[r#"
                ```php
                interface Serializable
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_class_const_with_type_hint() {
        check_hover(
            "<?php\nclass Config { const string VERSION = '1.0.0'; }",
            pos(1, 28),
            expect![[r#"
                ```php
                const string VERSION = '1.0.0'
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_class_const_float_value() {
        check_hover(
            "<?php\nclass Math { const float PI = 3.14; }",
            pos(1, 27),
            expect![[r#"
                ```php
                const float PI = 3.14
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_class_const_infers_type_from_value() {
        let (src, p) = cursor("<?php\nclass Config { const VERSION$0 = '1.0.0'; }");
        check_hover(
            &src,
            p,
            expect![[r#"
                ```php
                const string VERSION = '1.0.0'
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_interface_const_shows_type_and_value() {
        let (src, p) = cursor("<?php\ninterface Limits { const int MA$0X = 100; }");
        check_hover(
            &src,
            p,
            expect![[r#"
                ```php
                const int MAX = 100
                ```"#]],
        );
    }

    #[test]
    fn snapshot_hover_trait_const_shows_type_and_value() {
        let (src, p) = cursor("<?php\ntrait HasVersion { const string TAG$0 = 'v1'; }");
        check_hover(
            &src,
            p,
            expect![[r#"
                ```php
                const string TAG = 'v1'
                ```"#]],
        );
    }

    #[test]
    fn hover_on_static_var_with_array_default_shows_array() {
        let (src, p) = cursor("<?php\nfunction counter() { static $cach$0e = []; }");
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, None, p, &[]);
        assert!(
            result.is_some(),
            "expected hover result for static variable"
        );
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("array"),
                "expected array type in hover, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_static_var_with_new_shows_class() {
        let (src, p) = cursor("<?php\nfunction make() { static $inst$0ance = new MyService(); }");
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, None, p, &[]);
        assert!(
            result.is_some(),
            "expected hover result for static variable"
        );
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                mc.value.contains("MyService"),
                "expected MyService in hover, got: {}",
                mc.value
            );
        }
    }

    // Gap 1: variables defined in one method must not pollute hover in another method.
    #[test]
    fn hover_variable_in_method_does_not_leak_across_methods() {
        // $result is defined as Widget in methodA but the cursor is in methodB.
        // Before the fix, $result from methodA would appear in methodB's hover.
        let (src, p) = cursor(concat!(
            "<?php\n",
            "class Service {\n",
            "    public function methodA(): void { $result = new Widget(); }\n",
            "    public function methodB(): void { $res$0ult = new Invoice(); }\n",
            "}\n",
        ));
        let doc = ParsedDoc::parse(src.clone());
        let result = hover_info(&src, &doc, None, p, &[]);
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
            assert!(
                !mc.value.contains("Widget"),
                "Widget from methodA must not appear in methodB hover, got: {}",
                mc.value
            );
            assert!(
                mc.value.contains("Invoice"),
                "Invoice from methodB should appear in hover, got: {}",
                mc.value
            );
        }
    }
}
