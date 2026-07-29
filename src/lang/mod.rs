//! PHP-language model: configuration, autoload/PSR-4 resolution, docblock
//! parsing, and built-in name knowledge. Everything here is about the PHP
//! language and project conventions, as opposed to the generic text mechanics
//! in [`crate::text`].

pub mod config;
pub mod docblock;
pub mod php_names;

pub(crate) mod autoload;
