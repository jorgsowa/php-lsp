pub mod add_throws_action;
pub mod arrow_function_action;
pub mod extract_action;
pub mod extract_constant_action;
pub mod extract_interface_action;
pub mod extract_method_action;
pub mod facade_to_di_action;
pub mod generate_action;
pub mod generate_validation_rules_action;
pub mod implement_action;
pub mod inline_action;
pub mod local_to_property_action;
pub mod phpdoc_action;
pub mod promote_action;
pub mod route_scaffold_action;
pub mod switch_to_match_action;
pub mod type_action;
pub mod update_phpdoc_action;
pub mod visibility_action;

use tower_lsp_server::ls_types::Range;

pub(crate) fn line_in_range(line: u32, range: Range) -> bool {
    line >= range.start.line && line <= range.end.line
}

/// Leading whitespace of the line containing byte `pos`, up to `pos`.
pub(crate) fn indent_before(source: &str, pos: usize) -> String {
    let line_start = source[..pos].rfind('\n').map_or(0, |i| i + 1);
    source[line_start..pos]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect()
}

/// Leading whitespace of 0-based `line`.
pub(crate) fn line_indent(source: &str, line: u32) -> String {
    source
        .lines()
        .nth(line as usize)
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_default()
}
