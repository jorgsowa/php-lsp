/// `textDocument/declaration` — jump to the abstract or interface declaration of a symbol.
///
/// In PHP the distinction between declaration and definition matters for:
///   - Interface methods (declared but never given a body)
///   - Abstract class methods
///
/// For concrete symbols with no abstract counterpart this falls back to the same
/// result as go-to-definition so the request is never empty-handed.
use std::sync::Arc;

use tower_lsp::lsp_types::{Location, Position, Url};

use crate::ast::ParsedDoc;
use crate::resolve::{Container, Declaration, resolve_declaration};
use crate::util::{strip_variable_sigil, utf16_code_units, word_at_position};

/// Find the abstract or interface declaration of `word`.
/// Prefers abstract/interface declarations; falls back to any declaration.
pub fn goto_declaration(
    source: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
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
                range: sv.name_range(decl.name()),
            });
        }
    }

    // Second pass: any declaration (same as goto_definition)
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(decl) = resolve_declaration(&doc.program().stmts, &word, &is_any_declaration) {
            return Some(Location {
                uri: uri.clone(),
                range: sv.name_range(decl.name()),
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

/// Find abstract or interface declaration using `FileIndex` entries.
/// Returns line-only positions (character 0) for unopened files.
/// This is a limitation of the compact FileIndex — for opened files,
/// goto_declaration() provides precise name ranges.
pub fn goto_declaration_from_index(
    source: &str,
    indexes: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::file_index::FileIndex>,
    )],
    position: tower_lsp::lsp_types::Position,
) -> Option<Location> {
    use crate::file_index::ClassKind;
    use crate::util::word_at_position;
    let word = word_at_position(source, position)?;
    let bare = strip_variable_sigil(&word);

    let precise_range = |line: u32, name_char: u32, name: &str| -> tower_lsp::lsp_types::Range {
        let end_char = name_char + utf16_code_units(name);
        tower_lsp::lsp_types::Range {
            start: tower_lsp::lsp_types::Position {
                line,
                character: name_char,
            },
            end: tower_lsp::lsp_types::Position {
                line,
                character: end_char,
            },
        }
    };

    // First pass: abstract/interface declarations.
    for (uri, idx) in indexes {
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

    // Second pass: any declaration.
    for (uri, idx) in indexes {
        // Top-level functions.
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
    }
    None
}
