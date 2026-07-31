/// `textDocument/declaration` — jump to the abstract or interface declaration of a symbol.
///
/// In PHP the distinction between declaration and definition matters for:
///   - Interface methods (declared but never given a body)
///   - Abstract class methods
///
/// For concrete symbols with no abstract counterpart this falls back to the same
/// result as go-to-definition so the request is never empty-handed.
use std::sync::Arc;

use tower_lsp_server::ls_types::{Location, Position, Uri};

use crate::document::ast::ParsedDoc;
use crate::lang::is_bare_keyword_at;
use crate::text::{strip_variable_sigil, utf16_code_units, word_at_position};
use crate::types::resolve::{Container, Declaration, resolve_declaration};

/// Find the abstract or interface declaration of `word`.
/// Prefers abstract/interface declarations; falls back to any declaration.
pub fn goto_declaration(
    source: &str,
    all_docs: &[(Uri, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Location> {
    let word = word_at_position(source, position)?;

    // First pass: look for an abstract or interface declaration
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(decl) =
            resolve_declaration(&doc.program().stmts, &word, &is_abstract_declaration)
        {
            return Some(Location {
                uri: uri.clone(),
                range: sv.name_range_in_span(decl.name(), decl.span()),
            });
        }
    }

    // Second pass: any declaration (same as goto_definition)
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(decl) = resolve_declaration(&doc.program().stmts, &word, &is_any_declaration) {
            return Some(Location {
                uri: uri.clone(),
                range: sv.name_range_in_span(decl.name(), decl.span()),
            });
        }
    }

    None
}

/// Pass 1: abstract/interface declarations only — interface members and names,
/// plus abstract methods on classes and traits.
///
/// `resolve_declaration` checks a type's name before its members, whereas the original
/// walker checked interface members first. This only differs for an interface
/// named the same as a method it contains — syntactically legal but absurd, and
/// never seen in real PHP, so the order is harmless here.
fn is_abstract_declaration(decl: &Declaration<'_>) -> bool {
    match decl {
        Declaration::Interface { .. } => true,
        Declaration::Method {
            container: Container::Interface,
            ..
        } => true,
        Declaration::Method {
            method,
            container: Container::Class | Container::Trait,
            ..
        } => method.is_abstract,
        _ => false,
    }
}

/// Pass 2: any declaration. Constructor-promoted parameters are not surfaced as
/// declarations here (the original `find_any_declaration` never matched them).
fn is_any_declaration(decl: &Declaration<'_>) -> bool {
    !matches!(decl, Declaration::PromotedParam { .. })
}

fn precise_range(line: u32, name_char: u32, name: &str) -> tower_lsp_server::ls_types::Range {
    let end_char = name_char + utf16_code_units(name);
    tower_lsp_server::ls_types::Range {
        start: tower_lsp_server::ls_types::Position {
            line,
            character: name_char,
        },
        end: tower_lsp_server::ls_types::Position {
            line,
            character: end_char,
        },
    }
}

/// Pass-2 body scoped to a single file: any declaration named `word`
/// (`bare` for properties, which are indexed without their `$` sigil).
/// Shared by the `decls_by_name` fast path and the exhaustive fallback scan
/// below, so both paths apply identical matching rules.
fn any_declaration_in_file(
    uri: &tower_lsp_server::ls_types::Uri,
    idx: &crate::index::file_index::FileIndex,
    word: &str,
    bare: &str,
) -> Option<Location> {
    use crate::index::file_index::ClassKind;

    for f in &idx.functions {
        if f.name.as_ref() == word {
            return Some(Location {
                uri: uri.clone(),
                range: precise_range(f.start_line, f.name_char, &f.name),
            });
        }
    }

    for cls in &idx.classes {
        // Class/Interface/Trait/Enum declarations.
        if cls.name.as_ref() == word {
            return Some(Location {
                uri: uri.clone(),
                range: precise_range(cls.start_line, cls.name_char, &cls.name),
            });
        }

        // Methods.
        for m in &cls.methods {
            if m.name.as_ref() == word {
                return Some(Location {
                    uri: uri.clone(),
                    range: precise_range(m.start_line, m.name_char, &m.name),
                });
            }
        }

        // `@method` docblock methods — zero-width location at the tag line.
        for dm in &cls.doc_methods {
            if dm.name.as_ref() == word {
                return Some(Location {
                    uri: uri.clone(),
                    range: crate::text::zero_width_range(dm.start_line),
                });
            }
        }

        // Properties.
        for p in &cls.properties {
            if p.name.as_ref() == bare {
                return Some(Location {
                    uri: uri.clone(),
                    range: precise_range(p.start_line, p.name_char, &p.name),
                });
            }
        }

        // Class/Interface/Trait/Enum constants.
        for c in &cls.constants {
            if c.as_ref() == word {
                return Some(Location {
                    uri: uri.clone(),
                    range: precise_range(cls.start_line, cls.name_char, &cls.name),
                });
            }
        }

        // Enum cases (stored in separate `cases` field).
        if cls.kind == ClassKind::Enum {
            for case_name in &cls.cases {
                if case_name.as_ref() == word {
                    return Some(Location {
                        uri: uri.clone(),
                        range: precise_range(cls.start_line, cls.name_char, &cls.name),
                    });
                }
            }
        }
    }
    None
}

/// Find abstract or interface declaration using the aggregated workspace
/// index. Returns line-only positions (character 0) for unopened files.
/// This is a limitation of the compact FileIndex — for opened files,
/// goto_declaration() provides precise name ranges.
pub fn goto_declaration_from_index(
    source: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    position: tower_lsp_server::ls_types::Position,
) -> Option<Location> {
    use crate::index::file_index::ClassKind;
    use crate::text::word_at_position;
    let word = word_at_position(source, position)?;
    // A bare keyword (`abstract`, `final`, `class`, ...) can never be a
    // declaration name — bail out before either full-workspace scan below.
    // `decls_by_name` would always miss for these anyway, so without this
    // check every keyword click pays for the exhaustive fallback loop.
    if is_bare_keyword_at(source, position, &word) {
        return None;
    }
    let bare = strip_variable_sigil(&word);

    // First pass: abstract/interface declarations. Already bounded to a
    // small subset of classes (interfaces, traits, abstract classes), so
    // left as a scan rather than adding another index for it.
    for (uri, idx) in &wi.files {
        for cls in &idx.classes {
            match cls.kind {
                ClassKind::Interface => {
                    // Interface itself.
                    if cls.name.as_ref() == word {
                        return Some(Location {
                            uri: uri.clone(),
                            range: precise_range(cls.start_line, cls.name_char, &cls.name),
                        });
                    }
                    // Abstract method in interface.
                    for m in &cls.methods {
                        if m.name.as_ref() == word {
                            return Some(Location {
                                uri: uri.clone(),
                                range: precise_range(m.start_line, m.name_char, &m.name),
                            });
                        }
                    }
                }
                ClassKind::Trait => {
                    // Trait abstract methods.
                    for m in &cls.methods {
                        if m.is_abstract && m.name.as_ref() == word {
                            return Some(Location {
                                uri: uri.clone(),
                                range: precise_range(m.start_line, m.name_char, &m.name),
                            });
                        }
                    }
                }
                _ if cls.is_abstract => {
                    // Abstract methods in abstract classes.
                    for m in &cls.methods {
                        if m.is_abstract && m.name.as_ref() == word {
                            return Some(Location {
                                uri: uri.clone(),
                                range: precise_range(m.start_line, m.name_char, &m.name),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Second pass: any declaration. `decls_by_name` gives an O(1) candidate
    // file list for the common kinds (function/class/method/constant/
    // enum-case, all keyed by `word` — see `build_maps`), so the usual case
    // scans one file's classes instead of every class in every workspace
    // file. Properties keyed under `bare`, `@method` doc-methods (not
    // indexed at all), and any `decls_by_name` miss fall back to the
    // exhaustive scan below, identical to the original behavior.
    if let Some(refs) = wi.decls_by_name.get(&word) {
        for r in refs {
            if let Some((uri, idx)) = wi.files.get(r.file as usize)
                && let Some(loc) = any_declaration_in_file(uri, idx, &word, bare)
            {
                return Some(loc);
            }
        }
    }

    for (uri, idx) in &wi.files {
        if let Some(loc) = any_declaration_in_file(uri, idx, &word, bare) {
            return Some(loc);
        }
    }
    None
}
