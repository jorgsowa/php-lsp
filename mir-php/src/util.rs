/// Utilities shared across mir-php modules.
use php_ast::{TypeHint, TypeHintKind};

/// Convert a byte offset into a `(line, character)` position.
pub fn offset_to_position(source: &str, offset: u32) -> (u32, u32) {
    let offset = (offset as usize).min(source.len());
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
    let last_nl = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = (offset - last_nl) as u32;
    (line, character)
}

/// Format a `TypeHint` as a PHP type string, e.g. `?int`, `string|null`.
pub fn format_type_hint(hint: &TypeHint<'_, '_>) -> String {
    fmt_kind(&hint.kind)
}

fn fmt_kind(kind: &TypeHintKind<'_, '_>) -> String {
    match kind {
        TypeHintKind::Named(name) => name.to_string_repr().to_string(),
        TypeHintKind::Keyword(builtin, _) => builtin.as_str().to_string(),
        TypeHintKind::Nullable(inner) => format!("?{}", format_type_hint(inner)),
        TypeHintKind::Union(types) => types
            .iter()
            .map(format_type_hint)
            .collect::<Vec<_>>()
            .join("|"),
        TypeHintKind::Intersection(types) => types
            .iter()
            .map(format_type_hint)
            .collect::<Vec<_>>()
            .join("&"),
    }
}
