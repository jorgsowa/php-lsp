use php_ast::{ClassMemberKind, NamespaceBody, Param, Stmt, StmtKind};
use tower_lsp::lsp_types::Position;

use crate::ast::{MethodReturnsMap, ParsedDoc, format_type_hint};
use crate::util::{fqn_short_name, utf16_offset_to_byte};

/// Resolve the class(es) of a named-argument call's receiver variable, for
/// looking up the method's parameter signature. mir-primary: locate the
/// receiver occurrence (`$recv->method(`) before the cursor and read mir's
/// recorded type there; fall back to TypeMap for binding sites mir resolves to
/// `mixed` (e.g. `$x = Enum::Case`). Returns short class names, `|`-joined for
/// unions.
fn resolve_method_receiver_class(
    source: &str,
    doc: &ParsedDoc,
    doc_returns: &MethodReturnsMap,
    other_docs: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<ParsedDoc>,
        std::sync::Arc<MethodReturnsMap>,
    )],
    position: Position,
    receiver_var: &str,
    analysis: Option<&mir_analyzer::FileAnalysis>,
) -> Option<String> {
    if let Some(a) = analysis
        && let Some(offset) = receiver_var_offset(source, doc, position, receiver_var)
        && let Some(ty) = crate::type_query::type_at_offset(a, offset)
    {
        let names: Vec<String> = crate::type_query::class_names(ty)
            .iter()
            .map(|fqcn| fqn_short_name(fqcn).to_string())
            .collect();
        if !names.is_empty() {
            return Some(names.join("|"));
        }
    }
    // TypeMap fallback.
    let type_map = crate::type_map::TypeMap::from_docs_at_position(
        doc,
        doc_returns,
        other_docs.iter().map(|(_, d, r)| (d.as_ref(), r.as_ref())),
        None,
        position,
    );
    if receiver_var == "$this" {
        crate::type_map::enclosing_class_at(source, doc, position)
            .or_else(|| type_map.get(receiver_var).map(str::to_owned))
    } else {
        type_map.get(receiver_var).map(str::to_owned)
    }
}

/// Byte offset of the last char of the `receiver_var` token in the nearest
/// `receiver_var->` / `receiver_var?->` occurrence before the cursor — a
/// position inside mir's end-exclusive variable span.
fn receiver_var_offset(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    receiver_var: &str,
) -> Option<u32> {
    let line = source.lines().nth(position.line as usize)?;
    let cursor_byte = utf16_offset_to_byte(line, position.character as usize).min(line.len());
    let before = &line[..cursor_byte];
    let p = before
        .rfind(&format!("{receiver_var}?->"))
        .or_else(|| before.rfind(&format!("{receiver_var}->")))?;
    let line_start = doc.view().byte_of_position(Position {
        line: position.line,
        character: 0,
    });
    Some(line_start + (p + receiver_var.len()) as u32 - 1)
}

use super::formatting::{format_default_value, wrap_php};
use super::members::find_parent_class_name;
use super::parsing::extract_name_from_chars_end;

pub(crate) enum NamedArgCallee {
    Function(String),
    Method(
        String, /* receiver var */
        String, /* method name */
    ),
    StaticMethod(
        String, /* class or pseudo */
        String, /* method name */
    ),
}

/// Return true when the cursor word is a named-argument label: `foo(label: $x)`.
///
/// Guards: `::` after the word is a static access, not a named arg; lines that
/// start with `case` are switch-case labels.
pub(crate) fn is_named_arg_at(line: &str, cursor_col_utf16: usize, _word: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("case ") || trimmed.starts_with("case\t") {
        return false;
    }

    let chars: Vec<char> = line.chars().collect();
    let mut utf16 = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16 >= cursor_col_utf16 {
            break;
        }
        utf16 += ch.len_utf16();
        char_idx += 1;
    }
    // Advance past the word.
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    while char_idx < chars.len() && is_word_char(chars[char_idx]) {
        char_idx += 1;
    }
    // Must be followed by `:` but not `::`.
    char_idx < chars.len()
        && chars[char_idx] == ':'
        && !(char_idx + 1 < chars.len() && chars[char_idx + 1] == ':')
}

/// Scan backward from `cursor_col_utf16` (which is within the named-arg label
/// word) to find the opening `(` of the enclosing function call, then extract
/// the callee information from the text before that `(`.
pub(crate) fn extract_named_arg_callee(
    line: &str,
    cursor_col_utf16: usize,
) -> Option<NamedArgCallee> {
    let chars: Vec<char> = line.chars().collect();

    // Convert cursor position to char index.
    let mut utf16 = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16 >= cursor_col_utf16 {
            break;
        }
        utf16 += ch.len_utf16();
        char_idx += 1;
    }
    // Back up to the start of the word.
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    while char_idx > 0 && is_word_char(chars[char_idx - 1]) {
        char_idx -= 1;
    }

    // Scan backward through balanced parens to find the enclosing `(`.
    let mut depth = 0i32;
    let mut i = char_idx;
    while i > 0 {
        i -= 1;
        match chars[i] {
            ')' | ']' => depth += 1,
            '(' => {
                if depth == 0 {
                    return callee_from_chars_before(&chars[..i]);
                }
                depth -= 1;
            }
            '[' => {
                if depth == 0 {
                    return None; // Inside array, not a call.
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Extract the callee from the characters immediately before the opening `(`.
fn callee_from_chars_before(chars: &[char]) -> Option<NamedArgCallee> {
    let is_name_char = |c: char| c.is_alphanumeric() || c == '_';
    let end = chars.len()
        - chars
            .iter()
            .rev()
            .take_while(|&&c| c == ' ' || c == '\t')
            .count();
    if end == 0 {
        return None;
    }
    let mut start = end;
    while start > 0 && is_name_char(chars[start - 1]) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let name: String = chars[start..end].iter().collect();

    if start >= 2 && chars[start - 2] == '-' && chars[start - 1] == '>' {
        // Instance method: `$obj->method(`
        let receiver = extract_name_from_chars_end(&chars[..start - 2])?;
        Some(NamedArgCallee::Method(receiver, name))
    } else if start >= 3
        && chars[start - 3] == '?'
        && chars[start - 2] == '-'
        && chars[start - 1] == '>'
    {
        // Nullsafe: `$obj?->method(`
        let receiver = extract_name_from_chars_end(&chars[..start - 3])?;
        Some(NamedArgCallee::Method(receiver, name))
    } else if start >= 2 && chars[start - 2] == ':' && chars[start - 1] == ':' {
        // Static: `ClassName::method(`
        let is_class_char = |c: char| c.is_alphanumeric() || c == '_' || c == '\\';
        let cls_end = start - 2;
        let cls_end_trimmed = cls_end
            - chars[..cls_end]
                .iter()
                .rev()
                .take_while(|&&c| c == ' ' || c == '\t')
                .count();
        let mut cls_start = cls_end_trimmed;
        while cls_start > 0 && is_class_char(chars[cls_start - 1]) {
            cls_start -= 1;
        }
        if cls_start == cls_end_trimmed {
            return None;
        }
        let full_class: String = chars[cls_start..cls_end_trimmed].iter().collect();
        let short = full_class
            .rsplit('\\')
            .next()
            .unwrap_or(&full_class)
            .to_owned();
        Some(NamedArgCallee::StaticMethod(short, name))
    } else {
        Some(NamedArgCallee::Function(name))
    }
}

/// Build the hover string for a named argument label.
///
/// Returns `None` when the callee or matching parameter cannot be found.
#[allow(clippy::too_many_arguments)]
pub(crate) fn named_arg_hover_value(
    source: &str,
    doc: &ParsedDoc,
    doc_returns: &MethodReturnsMap,
    other_docs: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::ast::ParsedDoc>,
        std::sync::Arc<crate::ast::MethodReturnsMap>,
    )],
    position: Position,
    callee: &NamedArgCallee,
    label: &str,
    analysis: Option<&mir_analyzer::FileAnalysis>,
) -> Option<String> {
    let all_docs = || std::iter::once(doc).chain(other_docs.iter().map(|(_, d, _)| d.as_ref()));

    match callee {
        NamedArgCallee::Function(name) => {
            for d in all_docs() {
                if let Some((sig, db)) =
                    find_param_sig_in_stmts(d.source(), &d.program().stmts, name, None, label)
                {
                    return Some(format_named_param_hover(&sig, db.as_ref(), label));
                }
            }
            None
        }
        NamedArgCallee::Method(receiver_var, method_name) => {
            let class_name = resolve_method_receiver_class(
                source,
                doc,
                doc_returns,
                other_docs,
                position,
                receiver_var,
                analysis,
            )?;
            let first_class = class_name
                .split('|')
                .next()
                .unwrap_or(&class_name)
                .to_owned();
            for d in all_docs() {
                if let Some((sig, db)) = find_param_sig_in_stmts(
                    d.source(),
                    &d.program().stmts,
                    method_name,
                    Some(&first_class),
                    label,
                ) {
                    return Some(format_named_param_hover(&sig, db.as_ref(), label));
                }
            }
            None
        }
        NamedArgCallee::StaticMethod(class_name, method_name) => {
            let effective_class = if class_name == "self" || class_name == "static" {
                crate::type_map::enclosing_class_at(source, doc, position)
                    .unwrap_or_else(|| class_name.clone())
            } else if class_name == "parent" {
                crate::type_map::enclosing_class_at(source, doc, position)
                    .and_then(|enc| find_parent_class_name(&doc.program().stmts, &enc))
                    .unwrap_or_else(|| class_name.clone())
            } else {
                class_name.clone()
            };
            for d in all_docs() {
                if let Some((sig, db)) = find_param_sig_in_stmts(
                    d.source(),
                    &d.program().stmts,
                    method_name,
                    Some(&effective_class),
                    label,
                ) {
                    return Some(format_named_param_hover(&sig, db.as_ref(), label));
                }
            }
            None
        }
    }
}
fn find_param_sig_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    callee_name: &str,
    class_name: Option<&str>,
    label: &str,
) -> Option<(String, Option<crate::docblock::Docblock>)> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if class_name.is_none() && f.name == callee_name => {
                let param = f.params.iter().find(|p| p.name == label)?;
                let sig = format_single_param(param);
                let db = crate::docblock::docblock_before(source, stmt.span.start)
                    .map(|raw| crate::docblock::parse_docblock(&raw));
                return Some((sig, db));
            }
            StmtKind::Class(c)
                if class_name
                    .as_ref()
                    .map(|cn| cn == &c.name.as_ref().map(|n| n.to_string()).unwrap_or_default())
                    .unwrap_or(false) =>
            {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == callee_name
                    {
                        let param = m.params.iter().find(|p| p.name == label)?;
                        let sig = format_single_param(param);
                        let db = crate::docblock::docblock_before(source, member.span.start)
                            .map(|raw| crate::docblock::parse_docblock(&raw));
                        return Some((sig, db));
                    }
                }
            }
            StmtKind::Trait(t)
                if class_name
                    .as_ref()
                    .map(|cn| cn == &t.name.to_string())
                    .unwrap_or(false) =>
            {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == callee_name
                    {
                        let param = m.params.iter().find(|p| p.name == label)?;
                        let sig = format_single_param(param);
                        let db = crate::docblock::docblock_before(source, member.span.start)
                            .map(|raw| crate::docblock::parse_docblock(&raw));
                        return Some((sig, db));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = find_param_sig_in_stmts(
                        source,
                        &inner.stmts,
                        callee_name,
                        class_name,
                        label,
                    )
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn format_single_param(p: &Param<'_, '_>) -> String {
    let mut s = String::new();
    if let Some(t) = &p.type_hint {
        s.push_str(&format_type_hint(t));
        s.push(' ');
    }
    if p.variadic {
        s.push_str("...");
    }
    s.push('$');
    s.push_str(&p.name.to_string());
    if let Some(default) = &p.default {
        s.push_str(&format!(" = {}", format_default_value(default)));
    }
    s
}

fn format_named_param_hover(
    sig: &str,
    db: Option<&crate::docblock::Docblock>,
    label: &str,
) -> String {
    let mut value = wrap_php(&format!("(parameter) {}", sig));
    // Include the @param description for this parameter from the docblock.
    if let Some(db) = db {
        let matching_param = db.params.iter().find(|p| {
            p.name == label
                || p.name == format!("${}", label)
                || p.name.trim_start_matches('$') == label
        });
        if let Some(param) = matching_param
            && !param.description.is_empty()
        {
            value.push_str(&format!("\n\n---\n\n{}", param.description));
        }
    }
    value
}
