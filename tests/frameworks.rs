#![allow(dead_code, unused_imports)]

#[path = "common/mod.rs"]
mod common;

use common::render::render_code_actions;
pub use common::render::{
    assert_linked_editing_ranges_share_text, assert_selection_range_invariant,
};
pub use common::{
    TestServer, canonicalize_workspace_edit, lines_of, render_call_hierarchy, render_completion,
    render_document_symbols, render_hover, render_inlay_hints, render_locations,
    render_semantic_tokens, render_signature_help, render_workspace_symbols,
};

#[path = "frameworks/feature_laravel.rs"]
mod feature_laravel;
#[path = "frameworks/feature_laravel_code_action.rs"]
mod feature_laravel_code_action;
#[path = "frameworks/feature_laravel_config.rs"]
mod feature_laravel_config;
#[path = "frameworks/feature_laravel_env.rs"]
mod feature_laravel_env;
#[path = "frameworks/feature_laravel_references.rs"]
mod feature_laravel_references;
#[path = "frameworks/feature_laravel_route.rs"]
mod feature_laravel_route;
#[path = "frameworks/feature_laravel_route_scaffold.rs"]
mod feature_laravel_route_scaffold;
#[path = "frameworks/feature_laravel_string_key_references.rs"]
mod feature_laravel_string_key_references;
#[path = "frameworks/feature_laravel_translation.rs"]
mod feature_laravel_translation;
#[path = "frameworks/feature_laravel_view.rs"]
mod feature_laravel_view;
#[path = "frameworks/feature_symfony.rs"]
mod feature_symfony;
