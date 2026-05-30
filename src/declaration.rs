/// `textDocument/declaration` — jump to the abstract or interface declaration of a symbol.
///
/// In PHP the distinction between declaration and definition matters for:
///   - Interface methods (declared but never given a body)
///   - Abstract class methods
///
/// For concrete symbols with no abstract counterpart this falls back to the same
/// result as go-to-definition so the request is never empty-handed.
use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Url};

use crate::ast::{ParsedDoc, SourceView};
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
        if let Some(range) = find_abstract_declaration(sv, &doc.program().stmts, &word) {
            return Some(Location {
                uri: uri.clone(),
                range,
            });
        }
    }

    // Second pass: any declaration (same as goto_definition)
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(range) = find_any_declaration(sv, &doc.program().stmts, &word) {
            return Some(Location {
                uri: uri.clone(),
                range,
            });
        }
    }

    None
}

fn find_abstract_declaration(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    word: &str,
) -> Option<tower_lsp::lsp_types::Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Interface(i) => {
                // Interface methods are declarations without bodies
                for member in i.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == word
                    {
                        return Some(sv.name_range(m.name.or_error()));
                    }
                }
                if i.name == word {
                    return Some(sv.name_range(i.name.or_error()));
                }
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.is_abstract
                        && m.name == word
                    {
                        return Some(sv.name_range(m.name.or_error()));
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.is_abstract
                        && m.name == word
                    {
                        return Some(sv.name_range(m.name.or_error()));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = find_abstract_declaration(sv, &inner.stmts, word)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_any_declaration(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    word: &str,
) -> Option<tower_lsp::lsp_types::Range> {
    let bare = strip_variable_sigil(word);
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                return Some(sv.name_range(f.name.or_error()));
            }
            StmtKind::Class(c) if c.name.map(|n| n.or_error()) == Some(word) => {
                let name = c.name.expect("match guard ensures Some");
                return Some(sv.name_range(name.or_error()));
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(m.name.or_error()));
                        }
                        ClassMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(sv.name_range(cc.name.or_error()));
                        }
                        ClassMemberKind::Property(p) if p.name == bare => {
                            return Some(sv.name_range(p.name.or_error()));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Interface(i) => {
                if i.name == word {
                    return Some(sv.name_range(i.name.or_error()));
                }
                for member in i.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(m.name.or_error()));
                        }
                        ClassMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(sv.name_range(cc.name.or_error()));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Trait(t) => {
                if t.name == word {
                    return Some(sv.name_range(t.name.or_error()));
                }
                for member in t.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(m.name.or_error()));
                        }
                        ClassMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(sv.name_range(cc.name.or_error()));
                        }
                        ClassMemberKind::Property(p) if p.name == bare => {
                            return Some(sv.name_range(p.name.or_error()));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Enum(e) if e.name == word => {
                return Some(sv.name_range(e.name.or_error()));
            }
            StmtKind::Enum(e) => {
                for member in e.body.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Case(c) if c.name == word => {
                            return Some(sv.name_range(c.name.or_error()));
                        }
                        EnumMemberKind::Method(m) if m.name == word => {
                            return Some(sv.name_range(m.name.or_error()));
                        }
                        EnumMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(sv.name_range(cc.name.or_error()));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = find_any_declaration(sv, &inner.stmts, word)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
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
