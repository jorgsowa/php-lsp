//! PHP-language model: configuration, autoload/PSR-4 resolution, docblock
//! parsing, and built-in name knowledge. Everything here is about the PHP
//! language and project conventions, as opposed to the generic text mechanics
//! in [`crate::text`].

use tower_lsp_server::ls_types::Position;

pub mod config;
pub mod docblock;
pub mod php_names;

pub(crate) mod autoload;
pub(crate) mod docblock_gate;
pub(crate) mod keywords;

use docblock_gate::is_unresolvable_docblock_token_at;
pub(crate) use keywords::is_php_keyword;
use keywords::is_bare_keyword_at;

/// Whether the word at `position` is a bareword token that can never
/// resolve to a real declaration: a PHP reserved keyword/type-hint word
/// ([`keywords::is_bare_keyword_at`]) or a documentation-only PHPDoc token
/// ([`docblock_gate::is_unresolvable_docblock_token_at`]). Every navigation/
/// hover feature that must skip the former must skip the latter too —
/// combined here so callers only need one check.
pub(crate) fn is_unresolvable_bareword_at(source: &str, position: Position, word: &str) -> bool {
    is_bare_keyword_at(source, position, word)
        || is_unresolvable_docblock_token_at(source, position, word)
}
