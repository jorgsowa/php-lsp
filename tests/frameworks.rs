#![allow(dead_code, unused_imports)]

#[path = "common/mod.rs"]
mod common;

pub use common::render::{
    assert_linked_editing_ranges_share_text, assert_selection_range_invariant,
};
pub use common::{
    TestServer, canonicalize_workspace_edit, lines_of, render_completion, render_document_symbols,
    render_hover, render_inlay_hints, render_locations, render_semantic_tokens,
    render_workspace_symbols,
};

#[path = "frameworks/feature_laravel_gaps.rs"]
mod feature_laravel_gaps;
#[path = "frameworks/feature_symfony.rs"]
mod feature_symfony;
