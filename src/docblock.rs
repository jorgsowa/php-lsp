/// Docblock (`/** ... */`) parser.
///
/// Strips the `/**` / `*/` markers and leading `*` from each line, then
/// extracts `@param`, `@return`, and `@var` annotations.
use php_parser_rs::parser::ast::{
    classes::ClassMember, comments::CommentFormat, namespaces::NamespaceStatement, Statement,
};

#[derive(Debug, Default, PartialEq)]
pub struct Docblock {
    /// Free-text description (lines before the first `@` tag).
    pub description: String,
    /// `@param  TypeHint  $name  description`
    pub params: Vec<DocParam>,
    /// `@return  TypeHint  description`
    pub return_type: Option<DocReturn>,
    /// `@var  TypeHint`
    pub var_type: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct DocParam {
    pub type_hint: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, PartialEq)]
pub struct DocReturn {
    pub type_hint: String,
    pub description: String,
}

impl Docblock {
    /// Format as a Markdown string suitable for LSP hover content.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        if !self.description.is_empty() {
            out.push_str(&self.description);
            out.push_str("\n\n");
        }
        if let Some(ret) = &self.return_type {
            out.push_str(&format!("**@return** `{}`", ret.type_hint));
            if !ret.description.is_empty() {
                out.push_str(&format!(" — {}", ret.description));
            }
            out.push('\n');
        }
        for p in &self.params {
            out.push_str(&format!("**@param** `{}` `{}`", p.type_hint, p.name));
            if !p.description.is_empty() {
                out.push_str(&format!(" — {}", p.description));
            }
            out.push('\n');
        }
        out.trim_end().to_string()
    }
}

/// Parse a raw docblock string (the full `/** ... */` text, or just the
/// inner content — either form is handled).
pub fn parse_docblock(raw: &str) -> Docblock {
    // Strip /** and */
    let inner = raw.trim();
    let inner = inner.strip_prefix("/**").unwrap_or(inner);
    let inner = inner.strip_suffix("*/").unwrap_or(inner);

    let mut description_lines: Vec<String> = Vec::new();
    let mut params: Vec<DocParam> = Vec::new();
    let mut return_type: Option<DocReturn> = None;
    let mut var_type: Option<String> = None;

    for line in inner.lines() {
        // Strip leading whitespace and a leading `*`
        let line = line.trim();
        let line = line.strip_prefix('*').unwrap_or(line).trim();

        if line.starts_with('@') {
            let mut parts = line[1..].splitn(2, char::is_whitespace);
            let tag = parts.next().unwrap_or("").to_lowercase();
            let rest = parts.next().unwrap_or("").trim();

            match tag.as_str() {
                "param" => {
                    let (type_hint, rest) = split_first_word(rest);
                    let (name, desc) = split_first_word(rest);
                    params.push(DocParam {
                        type_hint: type_hint.to_string(),
                        name: name.to_string(),
                        description: desc.trim().to_string(),
                    });
                }
                "return" | "returns" => {
                    let (type_hint, desc) = split_first_word(rest);
                    return_type = Some(DocReturn {
                        type_hint: type_hint.to_string(),
                        description: desc.trim().to_string(),
                    });
                }
                "var" => {
                    let (type_hint, _) = split_first_word(rest);
                    var_type = Some(type_hint.to_string());
                }
                _ => {} // ignore unknown tags
            }
        } else if !line.is_empty() && return_type.is_none() && params.is_empty() {
            description_lines.push(line.to_string());
        }
    }

    Docblock {
        description: description_lines.join("\n").trim().to_string(),
        params,
        return_type,
        var_type,
    }
}

fn split_first_word(s: &str) -> (&str, &str) {
    let s = s.trim();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

/// Extract the docblock string (if any) immediately preceding `node_span`
/// from a `CommentGroup`.  Returns the content `ByteString` as a `String`.
pub fn docblock_for_function(f: &php_parser_rs::parser::ast::functions::FunctionStatement) -> Option<String> {
    docblock_from_group(&f.comments)
}

pub fn docblock_for_method(m: &php_parser_rs::parser::ast::functions::ConcreteMethod) -> Option<String> {
    docblock_from_group(&m.comments)
}

fn docblock_from_group(
    group: &php_parser_rs::parser::ast::comments::CommentGroup,
) -> Option<String> {
    group
        .comments
        .iter()
        .rev()
        .find(|c| c.format == CommentFormat::Document)
        .map(|c| c.content.to_string())
}

/// Walk an AST and return the parsed docblock for the declaration named `word`.
pub fn find_docblock(stmts: &[Statement], word: &str) -> Option<Docblock> {
    for stmt in stmts {
        match stmt {
            Statement::Function(f) if f.name.value.to_string() == word => {
                let raw = docblock_for_function(f)?;
                return Some(parse_docblock(&raw));
            }
            Statement::Class(c) => {
                for member in &c.body.members {
                    if let ClassMember::ConcreteMethod(m) = member {
                        if m.name.value.to_string() == word {
                            let raw = docblock_for_method(m)?;
                            return Some(parse_docblock(&raw));
                        }
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                if let Some(db) = find_docblock(inner, word) {
                    return Some(db);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_description() {
        let raw = "/** Does something useful. */";
        let db = parse_docblock(raw);
        assert_eq!(db.description, "Does something useful.");
    }

    #[test]
    fn parses_return_tag() {
        let raw = "/**\n * @return string The greeting\n */";
        let db = parse_docblock(raw);
        let ret = db.return_type.unwrap();
        assert_eq!(ret.type_hint, "string");
        assert_eq!(ret.description, "The greeting");
    }

    #[test]
    fn parses_param_tag() {
        let raw = "/**\n * @param string $name The user name\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.params.len(), 1);
        assert_eq!(db.params[0].type_hint, "string");
        assert_eq!(db.params[0].name, "$name");
        assert_eq!(db.params[0].description, "The user name");
    }

    #[test]
    fn parses_var_tag() {
        let raw = "/** @var string */";
        let db = parse_docblock(raw);
        assert_eq!(db.var_type.as_deref(), Some("string"));
    }

    #[test]
    fn multiple_params() {
        let raw = "/**\n * @param int $a First\n * @param int $b Second\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.params.len(), 2);
        assert_eq!(db.params[0].name, "$a");
        assert_eq!(db.params[1].name, "$b");
    }

    #[test]
    fn to_markdown_includes_description_and_return() {
        let db = Docblock {
            description: "Greets the user.".to_string(),
            params: vec![],
            return_type: Some(DocReturn {
                type_hint: "string".to_string(),
                description: "The greeting".to_string(),
            }),
            var_type: None,
        };
        let md = db.to_markdown();
        assert!(md.contains("Greets the user."));
        assert!(md.contains("@return"));
        assert!(md.contains("string"));
    }

    #[test]
    fn find_docblock_from_ast() {
        let src = "<?php\n/** Greets someone. */\nfunction greet() {}";
        let ast = match php_parser_rs::parser::parse(src) {
            Ok(a) => a,
            Err(s) => s.partial,
        };
        let db = find_docblock(&ast, "greet");
        assert!(db.is_some(), "expected docblock for greet");
        assert!(db.unwrap().description.contains("Greets"));
    }

    #[test]
    fn find_docblock_returns_none_without_docblock() {
        let src = "<?php\nfunction greet() {}";
        let ast = match php_parser_rs::parser::parse(src) {
            Ok(a) => a,
            Err(s) => s.partial,
        };
        let db = find_docblock(&ast, "greet");
        assert!(db.is_none());
    }

    #[test]
    fn empty_docblock_gives_defaults() {
        let db = parse_docblock("/** */");
        assert_eq!(db.description, "");
        assert!(db.return_type.is_none());
        assert!(db.params.is_empty());
    }
}
