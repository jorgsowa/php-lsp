//! Text mechanics: UTF-16/byte offset conversions, word extraction, fuzzy
//! matching, and zero-width range constructors. Pure string/position math with
//! no PHP-language knowledge (that lives in [`crate::lang`]).

mod fuzzy;
mod offset;
mod range;
mod source;
mod word;

pub(crate) use fuzzy::*;
pub(crate) use offset::*;
pub(crate) use range::*;
#[cfg_attr(target_family = "wasm", allow(unused_imports))]
pub(crate) use source::*;
pub(crate) use word::*;
