use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::*;

use std::collections::HashMap;

use crate::ast::{ParsedDoc, str_offset};
use crate::util::word_at;

/// Return a moniker for the symbol at `position`.
///
/// Scheme: `"php"`.
/// Identifier: the fully-qualified name in PHP convention. For class-like
/// declarations or references that resolve via `use` / namespace this is
/// `Ns\\ClassName`. For methods, properties, class constants, or enum cases
/// it is `Ns\\ClassName::member` (`::$prop` for properties), determined by
/// inspecting the AST node under the cursor. For unqualified words that
/// don't resolve to a local declaration or import, the bare word is
/// returned — the namespace prefix is *not* applied as a guess (PHP's
/// resolver falls back to global for unqualified function calls; for
/// classes the FQCN can't be inferred without explicit qualification).
/// Uniqueness: `project`.
pub fn moniker_at(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    file_imports: &HashMap<String, String>,
) -> Option<Moniker> {
    let word = word_at(source, position)?;
    if word.is_empty() {
        return None;
    }

    // Use the AST's own source for member detection. AST name slices
    // point into `doc.source()`, so `str_offset`'s pointer arithmetic
    // resolves to per-occurrence offsets only when the same allocation
    // is used; mixing in the caller-provided `source` falls back to
    // `source.find(name)`, which returns the first textual occurrence
    // and silently misattributes cursors when names collide (comments
    // mentioning the symbol, or the same method name in two classes).
    let ast_source = doc.source();

    // Member-name declaration sites are checked first so that property
    // declarations (whose `word` starts with `$`) still produce a moniker.
    let identifier = if let Some(id) = enclosing_member_identifier(ast_source, doc, position, &word)
    {
        id
    } else if word.starts_with('$') {
        // Plain variable — no project-stable identifier.
        return None;
    } else {
        resolve_fqn_for_moniker(doc, &word, file_imports)
    };

    Some(Moniker {
        scheme: "php".to_string(),
        identifier,
        unique: UniquenessLevel::Project,
        kind: Some(MonikerKind::Export),
    })
}

/// If the cursor sits on the *name* of a method, property, class constant, or
/// enum case declaration inside a class/interface/trait/enum, return
/// `Class::name` (or `Ns\\Class::name`, `Ns\\Class::$prop`, `Ns\\Enum::Case`).
/// Returns `None` for cursor positions outside a class-like declaration's
/// member-name span.
fn enclosing_member_identifier(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    word: &str,
) -> Option<String> {
    let cursor_byte = position_to_byte(source, position)?;
    // Property declarations carry the AST name without the `$`; strip it
    // from the cursor word before comparing.
    let bare = word.trim_start_matches('\\').trim_start_matches('$');
    walk_for_member(&doc.program().stmts, source, cursor_byte, bare, "")
}

fn walk_for_member(
    stmts: &[Stmt<'_, '_>],
    source: &str,
    cursor_byte: u32,
    word: &str,
    ns_prefix: &str,
) -> Option<String> {
    let mut current_ns: String = ns_prefix.to_owned();
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Namespace(ns) => {
                let ns_name = ns
                    .name
                    .as_ref()
                    .map(|n| n.to_string_repr().to_string())
                    .unwrap_or_default();
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        let prefix = if ns_name.is_empty() {
                            String::new()
                        } else {
                            format!("{ns_name}\\")
                        };
                        if let Some(id) = walk_for_member(inner, source, cursor_byte, word, &prefix)
                        {
                            return Some(id);
                        }
                    }
                    NamespaceBody::Simple => {
                        current_ns = if ns_name.is_empty() {
                            String::new()
                        } else {
                            format!("{ns_name}\\")
                        };
                    }
                }
            }
            StmtKind::Class(c) => {
                if !span_contains(stmt.span.start, stmt.span.end, cursor_byte) {
                    continue;
                }
                let Some(class_name) = c.name else { continue };
                for member in c.members.iter() {
                    if let Some(id) = match_class_member(
                        &member.kind,
                        source,
                        cursor_byte,
                        word,
                        &current_ns,
                        class_name,
                    ) {
                        return Some(id);
                    }
                }
            }
            StmtKind::Interface(i) => {
                if !span_contains(stmt.span.start, stmt.span.end, cursor_byte) {
                    continue;
                }
                for member in i.members.iter() {
                    if let Some(id) = match_class_member(
                        &member.kind,
                        source,
                        cursor_byte,
                        word,
                        &current_ns,
                        i.name,
                    ) {
                        return Some(id);
                    }
                }
            }
            StmtKind::Trait(t) => {
                if !span_contains(stmt.span.start, stmt.span.end, cursor_byte) {
                    continue;
                }
                for member in t.members.iter() {
                    if let Some(id) = match_class_member(
                        &member.kind,
                        source,
                        cursor_byte,
                        word,
                        &current_ns,
                        t.name,
                    ) {
                        return Some(id);
                    }
                }
            }
            StmtKind::Enum(e) => {
                if !span_contains(stmt.span.start, stmt.span.end, cursor_byte) {
                    continue;
                }
                for member in e.members.iter() {
                    let id = match &member.kind {
                        EnumMemberKind::Method(m) if m.name == word => {
                            cursor_on_name(source, cursor_byte, m.name)
                                .then(|| format!("{current_ns}{}::{}", e.name, m.name))
                        }
                        EnumMemberKind::Case(c) if c.name == word => {
                            cursor_on_name(source, cursor_byte, c.name)
                                .then(|| format!("{current_ns}{}::{}", e.name, c.name))
                        }
                        EnumMemberKind::ClassConst(cc) if cc.name == word => {
                            cursor_on_name(source, cursor_byte, cc.name)
                                .then(|| format!("{current_ns}{}::{}", e.name, cc.name))
                        }
                        _ => None,
                    };
                    if id.is_some() {
                        return id;
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn match_class_member(
    kind: &ClassMemberKind<'_, '_>,
    source: &str,
    cursor_byte: u32,
    word: &str,
    ns_prefix: &str,
    class_name: &str,
) -> Option<String> {
    match kind {
        ClassMemberKind::Method(m) if m.name == word => cursor_on_name(source, cursor_byte, m.name)
            .then(|| format!("{ns_prefix}{class_name}::{}", m.name)),
        ClassMemberKind::Property(p) if p.name == word => {
            cursor_on_name(source, cursor_byte, p.name)
                .then(|| format!("{ns_prefix}{class_name}::${}", p.name))
        }
        ClassMemberKind::ClassConst(c) if c.name == word => {
            cursor_on_name(source, cursor_byte, c.name)
                .then(|| format!("{ns_prefix}{class_name}::{}", c.name))
        }
        _ => None,
    }
}

#[inline]
fn cursor_on_name(source: &str, cursor_byte: u32, name: &str) -> bool {
    let start = str_offset(source, name);
    let end = start + name.len() as u32;
    // Inclusive on the right boundary so that a cursor positioned right
    // after the name (e.g. between `bar` and `(`) — a common "just typed
    // the name" position — still counts as on the name.
    cursor_byte >= start && cursor_byte <= end
}

#[inline]
fn span_contains(start: u32, end: u32, off: u32) -> bool {
    off >= start && off < end
}

/// UTF-16 `Position` → byte offset. Returns `None` if `position` is past the
/// end of the file. Mirrors the line-walking helper in `selection_range.rs`
/// without needing a `SourceView`.
fn position_to_byte(source: &str, position: Position) -> Option<u32> {
    let mut byte: u32 = 0;
    for (line, ln) in (0_u32..).zip(source.split_inclusive('\n')) {
        if line == position.line {
            let raw = ln.strip_suffix('\n').unwrap_or(ln);
            let raw = raw.strip_suffix('\r').unwrap_or(raw);
            let mut col_utf16: u32 = 0;
            let mut byte_in_line: u32 = 0;
            for ch in raw.chars() {
                if col_utf16 >= position.character {
                    break;
                }
                col_utf16 += ch.len_utf16() as u32;
                byte_in_line += ch.len_utf8() as u32;
            }
            return Some(byte + byte_in_line);
        }
        byte += ln.len() as u32;
    }
    None
}

/// Moniker-flavored FQN resolution. Like `resolve_fqn` but does NOT attach
/// the file's namespace prefix to unresolved unqualified words: PHP's
/// resolver falls back to global for unqualified function calls, and for
/// classes the FQCN cannot be inferred without explicit qualification or a
/// `use` import. Returning the bare word is therefore safer than guessing.
fn resolve_fqn_for_moniker(
    doc: &ParsedDoc,
    name: &str,
    file_imports: &HashMap<String, String>,
) -> String {
    let bare = name.trim_start_matches('\\');

    fn matches_top(kind: &StmtKind<'_, '_>, name: &str) -> bool {
        match kind {
            StmtKind::Class(c) => c.name == Some(name),
            StmtKind::Interface(i) => i.name == name,
            StmtKind::Trait(t) => t.name == name,
            StmtKind::Enum(e) => e.name == name,
            StmtKind::Function(f) => f.name == name,
            _ => false,
        }
    }

    let mut current_ns: Option<String> = None;
    for stmt in doc.program().stmts.iter() {
        match &stmt.kind {
            StmtKind::Namespace(ns) => {
                let ns_name = ns.name.as_ref().map(|n| n.to_string_repr().to_string());
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        let ns_prefix = ns_name
                            .as_ref()
                            .map(|n| format!("{n}\\"))
                            .unwrap_or_default();
                        for s in inner.iter() {
                            if matches_top(&s.kind, bare) {
                                return format!("{ns_prefix}{bare}");
                            }
                        }
                    }
                    NamespaceBody::Simple => {
                        current_ns = ns_name;
                    }
                }
            }
            k if matches_top(k, bare) => {
                return match &current_ns {
                    Some(ns) => format!("{ns}\\{bare}"),
                    None => bare.to_string(),
                };
            }
            _ => {}
        }
    }

    if let Some(fqn) = file_imports.get(bare) {
        return fqn.clone();
    }

    bare.to_string()
}

/// Walk the top-level statements of `doc` looking for a declaration of `name`
/// and return its fully-qualified name including the namespace prefix.
/// When the name is not declared in this file, checks `use` statements so that
/// imported names resolve to their FQN (e.g. `Mailer` → `App\\Services\\Mailer`).
/// Falls back to returning `name` as-is.
pub(crate) fn resolve_fqn(
    doc: &ParsedDoc,
    name: &str,
    file_imports: &HashMap<String, String>,
) -> String {
    // Strip a leading `\` from a fully-qualified reference.
    let bare = name.trim_start_matches('\\');

    // Track the current namespace prefix across top-level statements so that
    // the declaration-form `namespace App;` (NamespaceBody::Simple) applies
    // to every subsequent class/function until the next namespace statement.
    let mut current_ns: Option<String> = None;

    fn matches_top(kind: &StmtKind<'_, '_>, name: &str) -> bool {
        match kind {
            StmtKind::Class(c) => c.name == Some(name),
            StmtKind::Interface(i) => i.name == name,
            StmtKind::Trait(t) => t.name == name,
            StmtKind::Enum(e) => e.name == name,
            StmtKind::Function(f) => f.name == name,
            _ => false,
        }
    }

    for stmt in doc.program().stmts.iter() {
        match &stmt.kind {
            StmtKind::Namespace(ns) => {
                let ns_name = ns.name.as_ref().map(|n| n.to_string_repr().to_string());
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        let ns_prefix = ns_name
                            .as_ref()
                            .map(|n| format!("{n}\\"))
                            .unwrap_or_default();
                        for s in inner.iter() {
                            if matches_top(&s.kind, bare) {
                                return format!("{ns_prefix}{bare}");
                            }
                        }
                    }
                    NamespaceBody::Simple => {
                        // Set the "active namespace" for all following top-level stmts.
                        current_ns = ns_name;
                    }
                }
            }
            k if matches_top(k, bare) => {
                return match &current_ns {
                    Some(ns) => format!("{ns}\\{bare}"),
                    None => bare.to_string(),
                };
            }
            _ => {}
        }
    }

    // Not a local declaration — resolve via `use` statements.
    if let Some(fqn) = file_imports.get(bare) {
        return fqn.clone();
    }

    // No local declaration and no `use` import. When the file declares a
    // namespace (Simple form), unqualified references still resolve to that
    // namespace (PHP falls back to global only for *functions*; for classes
    // the namespace-prefixed FQCN is authoritative).
    if let Some(ns) = current_ns {
        return format!("{ns}\\{bare}");
    }

    bare.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn empty() -> HashMap<String, String> {
        HashMap::new()
    }

    #[test]
    fn bare_class_name() {
        let src = "<?php\nclass Foo {}";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(1, 7), &empty()).unwrap();
        assert_eq!(m.scheme, "php");
        assert_eq!(m.identifier, "Foo");
        assert_eq!(m.unique, UniquenessLevel::Project);
        assert_eq!(m.kind, Some(MonikerKind::Export));
    }

    #[test]
    fn namespaced_class() {
        let src = "<?php\nnamespace App\\Services {\n    class FooService {}\n}";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(2, 10), &empty()).unwrap();
        assert_eq!(m.identifier, "App\\Services\\FooService");
    }

    #[test]
    fn unknown_word_returns_bare_name() {
        let src = "<?php\n$x = doSomething();";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(1, 6), &empty()).unwrap();
        assert_eq!(m.identifier, "doSomething");
    }

    #[test]
    fn empty_position_returns_none() {
        let src = "<?php\n   ";
        let d = doc(src);
        assert!(moniker_at(src, &d, pos(1, 1), &empty()).is_none());
    }

    #[test]
    fn variable_returns_none() {
        let src = "<?php\n$foo = 1;";
        let d = doc(src);
        assert!(moniker_at(src, &d, pos(1, 1), &empty()).is_none());
    }

    #[test]
    fn imported_name_resolves_via_use_statement() {
        let src = "<?php\nuse App\\Services\\Mailer;\n$m = new Mailer();";
        let d = doc(src);
        let imports = HashMap::from([("Mailer".to_string(), "App\\Services\\Mailer".to_string())]);
        // Cursor on `Mailer` in `new Mailer()`
        let m = moniker_at(src, &d, pos(2, 10), &imports).unwrap();
        assert_eq!(m.identifier, "App\\Services\\Mailer");
    }

    #[test]
    fn use_alias_resolves_to_fqn() {
        let src = "<?php\nuse App\\Http\\Request as Req;\n$r = new Req();";
        let d = doc(src);
        let imports = HashMap::from([("Req".to_string(), "App\\Http\\Request".to_string())]);
        let m = moniker_at(src, &d, pos(2, 10), &imports).unwrap();
        assert_eq!(m.identifier, "App\\Http\\Request");
    }

    #[test]
    fn uniqueness_is_workspace() {
        let src = "<?php\nclass Foo {}";
        let d = doc(src);
        let m = moniker_at(src, &d, pos(1, 7), &empty()).unwrap();
        assert_eq!(m.unique, UniquenessLevel::Project);
    }
}
