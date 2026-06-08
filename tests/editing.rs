#![allow(dead_code, unused_imports)]

#[path = "common/mod.rs"]
mod common;

pub use common::render::{
    assert_document_symbol_containment, assert_linked_editing_ranges_share_text,
    assert_selection_range_invariant,
};
pub use common::{
    TestServer, canonicalize_workspace_edit, lines_of, render_completion, render_document_symbols,
    render_hover, render_inlay_hints, render_locations, render_resolved_code_action,
    render_resolved_code_lens, render_resolved_completion_item, render_resolved_document_link,
    render_resolved_inlay_hint, render_resolved_workspace_symbol, render_semantic_tokens,
    render_workspace_symbols,
};

#[path = "editing/feature_code_action_extract_constant.rs"]
mod feature_code_action_extract_constant;
#[path = "editing/feature_code_action_extract_method.rs"]
mod feature_code_action_extract_method;
#[path = "editing/feature_code_action_extract_variable.rs"]
mod feature_code_action_extract_variable;
#[path = "editing/feature_code_action_generate.rs"]
mod feature_code_action_generate;
#[path = "editing/feature_code_action_implement_interface.rs"]
mod feature_code_action_implement_interface;
#[path = "editing/feature_code_action_inline_variable.rs"]
mod feature_code_action_inline_variable;
#[path = "editing/feature_code_action_organize_imports.rs"]
mod feature_code_action_organize_imports;
#[path = "editing/feature_code_action_phpdoc.rs"]
mod feature_code_action_phpdoc;
#[path = "editing/feature_code_action_return_type.rs"]
mod feature_code_action_return_type;
#[path = "editing/feature_code_actions.rs"]
mod feature_code_actions;
#[path = "editing/feature_completion.rs"]
mod feature_completion;
#[path = "editing/feature_cursor_helpers.rs"]
mod feature_cursor_helpers;
#[path = "editing/feature_document_link.rs"]
mod feature_document_link;
#[path = "editing/feature_folding.rs"]
mod feature_folding;
#[path = "editing/feature_formatting.rs"]
mod feature_formatting;
#[path = "editing/feature_hover_advanced.rs"]
mod feature_hover_advanced;
#[path = "editing/feature_hover_basic.rs"]
mod feature_hover_basic;
#[path = "editing/feature_hover_cross_file.rs"]
mod feature_hover_cross_file;
#[path = "editing/feature_hover_inheritance.rs"]
mod feature_hover_inheritance;
#[path = "editing/feature_hover_types.rs"]
mod feature_hover_types;
#[path = "editing/feature_item_resolve.rs"]
mod feature_item_resolve;
#[path = "editing/feature_linked_editing.rs"]
mod feature_linked_editing;
#[path = "editing/feature_selection_range.rs"]
mod feature_selection_range;
#[path = "editing/feature_signature_help.rs"]
mod feature_signature_help;
#[path = "editing/feature_symbols.rs"]
mod feature_symbols;
