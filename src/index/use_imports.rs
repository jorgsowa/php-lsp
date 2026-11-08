//! Token-level scan of a file's `use` imports, cheap enough to run on every
//! file that mentions a symbol without analyzing it.

use php_lexer::{Lexer, TokenKind};

/// Lowercased names `source`'s `use` statements import, without a leading `\`.
/// Covers aliases, group members and `use function`/`use const`; trait `use`
/// lists inside a class body are collected too, which only over-matches.
pub(crate) fn imported_names(source: &str) -> Vec<String> {
    let mut tokens = Tokens::new(source);
    let mut out = Vec::new();
    while let Some(kind) = tokens.kind() {
        tokens.bump();
        if kind == TokenKind::Use && tokens.kind() != Some(TokenKind::LeftParen) {
            use_statement(&mut tokens, &mut out);
        }
    }
    out
}

/// Whether an import of `imported` can name `target` (both lowercased, no
/// leading `\`): the import is the target itself or one of its namespaces,
/// as in `use Foo\Bar;` followed by `Bar\Baz`.
pub(crate) fn import_reaches(imported: &str, target: &str) -> bool {
    target
        .strip_prefix(imported)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
}

fn use_statement(tokens: &mut Tokens<'_>, out: &mut Vec<String>) {
    skip_kind_modifier(tokens);
    loop {
        let Some(name) = tokens.name() else { return };
        if tokens.kind() == Some(TokenKind::LeftBrace) {
            tokens.bump();
            loop {
                skip_kind_modifier(tokens);
                let Some(member) = tokens.name() else { return };
                out.push(format!("{}\\{member}", name.trim_end_matches('\\')));
                skip_alias(tokens);
                match tokens.kind() {
                    Some(TokenKind::Comma) => tokens.bump(),
                    Some(TokenKind::RightBrace) => {
                        tokens.bump();
                        break;
                    }
                    _ => return,
                }
                if tokens.kind() == Some(TokenKind::RightBrace) {
                    tokens.bump();
                    break;
                }
            }
        } else {
            out.push(name);
            skip_alias(tokens);
        }
        if tokens.kind() == Some(TokenKind::Comma) {
            tokens.bump();
        } else {
            return;
        }
    }
}

fn skip_kind_modifier(tokens: &mut Tokens<'_>) {
    if matches!(tokens.kind(), Some(TokenKind::Function | TokenKind::Const)) {
        tokens.bump();
    }
}

fn skip_alias(tokens: &mut Tokens<'_>) {
    if tokens.kind() == Some(TokenKind::As) {
        tokens.bump();
        if tokens.segment().is_some() {
            tokens.bump();
        }
    }
}

/// Comment-free token cursor over `source`.
struct Tokens<'a> {
    lexer: Lexer<'a>,
    current: Option<(TokenKind, &'a str)>,
}

impl<'a> Tokens<'a> {
    fn new(source: &'a str) -> Self {
        let mut tokens = Self {
            lexer: Lexer::new(source),
            current: None,
        };
        tokens.bump();
        tokens
    }

    fn kind(&self) -> Option<TokenKind> {
        self.current.map(|(kind, _)| kind)
    }

    fn bump(&mut self) {
        self.current = loop {
            let token = self.lexer.next_token();
            if token.kind == TokenKind::Eof {
                break None;
            }
            if !token.kind.is_comment() {
                break Some((token.kind, self.lexer.token_text(&token)));
            }
        };
    }

    /// The current token's text when it can be a name segment; keywords
    /// qualify (`use Foo\List;`).
    fn segment(&self) -> Option<&'a str> {
        let (_, text) = self.current?;
        let mut bytes = text.bytes();
        let first = bytes.next()?;
        let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
        (ident(first) && !first.is_ascii_digit() && bytes.all(ident)).then_some(text)
    }

    /// A lowercased, possibly qualified name; a trailing `\` is kept when a
    /// group `{` follows.
    fn name(&mut self) -> Option<String> {
        let mut name = String::new();
        if self.kind() == Some(TokenKind::Backslash) {
            self.bump();
        }
        loop {
            let segment = self.segment()?;
            name.push_str(&segment.to_ascii_lowercase());
            self.bump();
            if self.kind() != Some(TokenKind::Backslash) {
                return Some(name);
            }
            self.bump();
            if self.kind() == Some(TokenKind::LeftBrace) {
                name.push('\\');
                return Some(name);
            }
            name.push('\\');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<String> {
        imported_names(source)
    }

    #[test]
    fn plain_aliased_and_leading_backslash_imports() {
        assert_eq!(
            names("<?php\nnamespace App;\nuse Foo\\Bar;\nuse \\Baz\\Qux as Q, LogicException;\n"),
            ["foo\\bar", "baz\\qux", "logicexception"]
        );
    }

    #[test]
    fn group_members_expand_with_their_prefix() {
        assert_eq!(
            names("<?php\nuse Foo\\{Bar, Sub\\Baz as B, function helper, const LIMIT};\n"),
            ["foo\\bar", "foo\\sub\\baz", "foo\\helper", "foo\\limit"]
        );
        assert_eq!(names("<?php\nuse Foo\\{Bar,};\n"), ["foo\\bar"]);
    }

    #[test]
    fn function_and_const_imports_and_keyword_segments() {
        assert_eq!(
            names("<?php\nuse function App\\Util\\helper;\nuse const App\\MAX;\nuse Foo\\List;\n"),
            ["app\\util\\helper", "app\\max", "foo\\list"]
        );
    }

    #[test]
    fn closure_use_strings_and_comments_are_not_imports() {
        assert_eq!(
            names(
                "<?php\n// use Fake\\One;\n$s = 'use Fake\\Two;';\n$f = function () use ($x) {};\n/* use Fake\\Three; */\nuse Real\\Four;\n"
            ),
            ["real\\four"]
        );
    }

    #[test]
    fn case_is_folded() {
        assert_eq!(names("<?php\nUSE Foo\\BAR;\n"), ["foo\\bar"]);
    }

    #[test]
    fn import_reaches_the_target_or_its_namespaces() {
        assert!(import_reaches("foo\\bar\\baz", "foo\\bar\\baz"));
        assert!(import_reaches("foo\\bar", "foo\\bar\\baz"));
        assert!(!import_reaches("foo\\ba", "foo\\bar\\baz"));
        assert!(!import_reaches("foo\\bar\\baz\\qux", "foo\\bar\\baz"));
    }
}
