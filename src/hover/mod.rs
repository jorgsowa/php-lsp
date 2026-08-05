mod closures;
pub(crate) mod formatting;
mod hover_impl;
mod members;
mod named_args;
mod parsing;
mod symbols;

pub use formatting::class_hover_from_workspace_index;
pub use formatting::format_params_str;
pub use formatting::method_hover_from_index;
pub use formatting::method_hover_from_workspace_index;
pub use formatting::{
    docs_for_symbol_from_workspace_index_scoped, signature_for_symbol_from_workspace_index_scoped,
};
pub use hover_impl::hover_info_with_maps;
pub use parsing::extract_receiver_var_before_cursor;
pub use parsing::extract_static_class_before_cursor;
pub use parsing::resolve_use_alias;
pub use parsing::resolve_use_alias_fqn;
pub use symbols::{
    class_hover_from_index, docs_for_symbol_from_index, docs_for_symbol_from_index_scoped,
    signature_for_symbol_from_index, signature_for_symbol_from_index_scoped,
};
