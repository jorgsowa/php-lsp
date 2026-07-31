use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::*;

use std::collections::HashMap;

use crate::document::ast::ParsedDoc;
use crate::text::word_at_position;

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
    let word = word_at_position(source, position)?;
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
    let cursor_byte = doc.view().byte_of_position(position);
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
                        if let Some(id) =
                            walk_for_member(&inner.stmts, source, cursor_byte, word, &prefix)
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
                let class_name_str = class_name.to_string();
                for member in c.body.members.iter() {
                    if let Some(id) = match_class_member(
                        &member.kind,
                        source,
                        cursor_byte,
                        word,
                        &current_ns,
                        &class_name_str,
                        member.span,
                    ) {
                        return Some(id);
                    }
                }
            }
            StmtKind::Interface(i) => {
                if !span_contains(stmt.span.start, stmt.span.end, cursor_byte) {
                    continue;
                }
                let interface_name = i.name.to_string();
                for member in i.body.members.iter() {
                    if let Some(id) = match_class_member(
                        &member.kind,
                        source,
                        cursor_byte,
                        word,
                        &current_ns,
                        &interface_name,
                        member.span,
                    ) {
                        return Some(id);
                    }
                }
            }
            StmtKind::Trait(t) => {
                if !span_contains(stmt.span.start, stmt.span.end, cursor_byte) {
                    continue;
                }
                let trait_name = t.name.to_string();
                for member in t.body.members.iter() {
                    if let Some(id) = match_class_member(
                        &member.kind,
                        source,
                        cursor_byte,
                        word,
                        &current_ns,
                        &trait_name,
                        member.span,
                    ) {
                        return Some(id);
                    }
                }
            }
            StmtKind::Enum(e) => {
                if !span_contains(stmt.span.start, stmt.span.end, cursor_byte) {
                    continue;
                }
                for member in e.body.members.iter() {
                    let id = match &member.kind {
                        EnumMemberKind::Method(m) if m.name == word => cursor_on_name_in_span(
                            source,
                            cursor_byte,
                            &m.name.to_string(),
                            member.span,
                        )
                        .then(|| format!("{current_ns}{}::{}", e.name, m.name)),
                        EnumMemberKind::Case(c) if c.name == word => cursor_on_name_in_span(
                            source,
                            cursor_byte,
                            &c.name.to_string(),
                            member.span,
                        )
                        .then(|| format!("{current_ns}{}::{}", e.name, c.name)),
                        EnumMemberKind::ClassConst(cc) if cc.name == word => {
                            cursor_on_name_in_span(
                                source,
                                cursor_byte,
                                &cc.name.to_string(),
                                member.span,
                            )
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
    member_span: php_ast::Span,
) -> Option<String> {
    match kind {
        ClassMemberKind::Method(m) if m.name == word => {
            cursor_on_name_in_span(source, cursor_byte, &m.name.to_string(), member_span)
                .then(|| format!("{ns_prefix}{class_name}::{}", m.name))
        }
        ClassMemberKind::Property(p) if p.name == word => {
            cursor_on_name_in_span(source, cursor_byte, &p.name.to_string(), member_span)
                .then(|| format!("{ns_prefix}{class_name}::${}", p.name))
        }
        ClassMemberKind::ClassConst(c) if c.name == word => {
            cursor_on_name_in_span(source, cursor_byte, &c.name.to_string(), member_span)
                .then(|| format!("{ns_prefix}{class_name}::{}", c.name))
        }
        _ => None,
    }
}

/// Variant of [`cursor_on_name`] that searches for the name within
/// `member_span` rather than the whole file. Avoids the global-`str_offset`
/// bug where two classes with same-named members both map to the first one.
#[inline]
fn cursor_on_name_in_span(
    source: &str,
    cursor_byte: u32,
    name: &str,
    member_span: php_ast::Span,
) -> bool {
    let s = member_span.start as usize;
    let e = (member_span.end as usize).min(source.len());
    let Some(slice) = source.get(s..e) else {
        return false;
    };
    let Some(off) = slice.find(name) else {
        return false;
    };
    let start = member_span.start + off as u32;
    let end = start + name.len() as u32;
    // Inclusive on the right boundary so that a cursor positioned right
    // after the name (e.g. between `bar` and `(`) — a common "just typed
    // the name" position — still resolves.
    cursor_byte >= start && cursor_byte <= end
}

#[inline]
fn span_contains(start: u32, end: u32, off: u32) -> bool {
    off >= start && off < end
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
            StmtKind::Class(c) => c.name.as_ref().map(|n| n.to_string()) == Some(name.to_string()),
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
                        for s in inner.stmts.iter() {
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
    // A leading `\` marks the name as already fully-qualified: skip both
    // `use`-import lookups and namespace fallback. Strip the slash for the
    // local-declaration walk below.
    let is_fqn = name.starts_with('\\');
    let bare = name.trim_start_matches('\\');

    // Fully-qualified names (`\Foo\Bar`) bypass everything below — they're
    // already absolute.
    if is_fqn {
        return bare.to_string();
    }

    // PHP semantics: `use` imports take precedence over local class names.
    // Check imports first before scanning for local declarations.
    if let Some(fqn) = file_imports.get(bare) {
        return fqn.clone();
    }

    // Qualified-via-aliased-use: `Sub\Inner` where `use App\Sub;` is in
    // scope. The first segment of the qualified name is the alias; replace
    // it with the alias's FQN and append the remainder.
    if let Some((first, rest)) = bare.split_once('\\')
        && let Some(prefix) = file_imports.get(first)
    {
        return format!("{prefix}\\{rest}");
    }

    // Track the current namespace prefix across top-level statements so that
    // the declaration-form `namespace App;` (NamespaceBody::Simple) applies
    // to every subsequent class/function until the next namespace statement.
    let mut current_ns: Option<String> = None;
    // Namespace from the braced form — used as fallback when the name is not a
    // local declaration but the whole file lives inside `namespace Foo { }`.
    let mut braced_ns: Option<String> = None;

    fn matches_top(kind: &StmtKind<'_, '_>, name: &str) -> bool {
        match kind {
            StmtKind::Class(c) => c.name.as_ref().map(|n| n.to_string()) == Some(name.to_string()),
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
                        for s in inner.stmts.iter() {
                            if matches_top(&s.kind, bare) {
                                return format!("{ns_prefix}{bare}");
                            }
                        }
                        // No local declaration matched — record the braced namespace so
                        // unqualified names that resolve via imports or fallback still
                        // get the correct namespace prefix applied.
                        braced_ns = ns_name;
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

    // No local declaration and no `use` import. When the file declares a
    // namespace (Simple or Braced form), unqualified references still resolve
    // to that namespace (PHP falls back to global only for *functions*; for
    // classes the namespace-prefixed FQCN is authoritative).
    let effective_ns = current_ns.or(braced_ns);
    if let Some(ns) = effective_ns {
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

    fn empty() -> HashMap<String, String> {
        HashMap::new()
    }

    #[test]
    fn resolve_fqn_qualified_via_aliased_use() {
        // `use App\Sub;` then `new Sub\Foo()` must resolve to `App\Sub\Foo`.
        // The first segment of the qualified name (`Sub`) is the use-imported
        // alias; the remainder is appended.
        let src = "<?php\nuse App\\Sub;\n";
        let d = doc(src);
        let imports = HashMap::from([("Sub".to_string(), "App\\Sub".to_string())]);
        assert_eq!(resolve_fqn(&d, "Sub\\Foo", &imports), "App\\Sub\\Foo");
    }

    #[test]
    fn resolve_fqn_qualified_via_aliased_use_with_alias() {
        // `use App\Submodule as Sub;` then `Sub\Foo`.
        let src = "<?php\nuse App\\Submodule as Sub;\n";
        let d = doc(src);
        let imports = HashMap::from([("Sub".to_string(), "App\\Submodule".to_string())]);
        assert_eq!(resolve_fqn(&d, "Sub\\Foo", &imports), "App\\Submodule\\Foo");
    }

    #[test]
    fn resolve_fqn_qualified_without_matching_use_falls_back_to_namespace() {
        // No `use` import for `Sub` — qualified name resolves relative to the
        // current namespace.
        let src = "<?php\nnamespace Acme;\n";
        let d = doc(src);
        let m = resolve_fqn(&d, "Sub\\Foo", &empty());
        assert_eq!(m, "Acme\\Sub\\Foo");
    }

    #[test]
    fn resolve_fqn_fully_qualified_bypasses_use_imports() {
        // A leading `\` means "use the literal FQN" — must not consult `use`
        // imports, even if the first segment happens to match an alias.
        let src = "<?php\nuse App\\Sub;\n";
        let d = doc(src);
        let imports = HashMap::from([("Sub".to_string(), "App\\Sub".to_string())]);
        assert_eq!(resolve_fqn(&d, "\\Sub\\Foo", &imports), "Sub\\Foo");
    }
}
