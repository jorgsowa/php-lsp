/// Docblock (`/** ... */`) parser.
///
/// Strips the `/**` / `*/` markers and leading `*` from each line, then
/// extracts `@param`, `@return`, `@var`, `@throws`, `@deprecated`, `@see`,
/// and `@link` annotations.

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
    /// `@deprecated  message`  — `Some("")` when present without a message.
    pub deprecated: Option<String>,
    /// `@throws  ClassName  description`
    pub throws: Vec<DocThrows>,
    /// `@see target` and `@link url`
    pub see: Vec<String>,
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

#[derive(Debug, PartialEq)]
pub struct DocThrows {
    pub class: String,
    pub description: String,
}

impl Docblock {
    /// Returns `true` if the `@deprecated` tag is present.
    pub fn is_deprecated(&self) -> bool {
        self.deprecated.is_some()
    }

    /// Format as a Markdown string suitable for LSP hover content.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();

        if let Some(msg) = &self.deprecated {
            if msg.is_empty() {
                out.push_str("> **Deprecated**\n\n");
            } else {
                out.push_str(&format!("> **Deprecated**: {}\n\n", msg));
            }
        }

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
        for t in &self.throws {
            out.push_str(&format!("**@throws** `{}`", t.class));
            if !t.description.is_empty() {
                out.push_str(&format!(" — {}", t.description));
            }
            out.push('\n');
        }
        for s in &self.see {
            out.push_str(&format!("**@see** {}\n", s));
        }
        out.trim_end().to_string()
    }
}

/// Parse a raw docblock string (the full `/** ... */` text, or just the
/// inner content — either form is handled).
pub fn parse_docblock(raw: &str) -> Docblock {
    let inner = raw.trim();
    let inner = inner.strip_prefix("/**").unwrap_or(inner);
    let inner = inner.strip_suffix("*/").unwrap_or(inner);

    let mut description_lines: Vec<String> = Vec::new();
    let mut params: Vec<DocParam> = Vec::new();
    let mut return_type: Option<DocReturn> = None;
    let mut var_type: Option<String> = None;
    let mut deprecated: Option<String> = None;
    let mut throws: Vec<DocThrows> = Vec::new();
    let mut see: Vec<String> = Vec::new();

    for line in inner.lines() {
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
                "deprecated" => {
                    deprecated = Some(rest.to_string());
                }
                "throws" | "throw" => {
                    let (class, desc) = split_first_word(rest);
                    throws.push(DocThrows {
                        class: class.to_string(),
                        description: desc.trim().to_string(),
                    });
                }
                "see" | "link" => {
                    if !rest.is_empty() {
                        see.push(rest.to_string());
                    }
                }
                _ => {}
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
        deprecated,
        throws,
        see,
    }
}

fn split_first_word(s: &str) -> (&str, &str) {
    let s = s.trim();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

/// Scan `source` for a `/** ... */` docblock that ends immediately before
/// `node_start` (byte offset). Whitespace between the `*/` and the node is
/// allowed; non-whitespace text in between disqualifies the block.
pub fn docblock_before(source: &str, node_start: u32) -> Option<String> {
    let before = &source[..node_start.min(source.len() as u32) as usize];
    // Trim trailing whitespace to find where `*/` might be
    let trimmed = before.trim_end();
    let end = trimmed.strip_suffix("*/")?;
    let doc_start = end.rfind("/**")?;
    Some(trimmed[doc_start..].to_string() + "*/")
}

/// Walk an AST and return the parsed docblock for the declaration named `word`.
pub fn find_docblock(
    source: &str,
    stmts: &[php_ast::Stmt<'_, '_>],
    word: &str,
) -> Option<Docblock> {
    use php_ast::{ClassMemberKind, NamespaceBody, StmtKind};
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                let raw = docblock_before(source, stmt.span.start)?;
                return Some(parse_docblock(&raw));
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == word {
                            let raw = docblock_before(source, member.span.start)?;
                            return Some(parse_docblock(&raw));
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(db) = find_docblock(source, inner, word) {
                        return Some(db);
                    }
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
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(md.contains("Greets the user."));
        assert!(md.contains("@return"));
        assert!(md.contains("string"));
    }

    #[test]
    fn find_docblock_from_ast() {
        use crate::ast::ParsedDoc;
        let src = "<?php\n/** Greets someone. */\nfunction greet() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let db = find_docblock(src, &doc.program().stmts, "greet");
        assert!(db.is_some(), "expected docblock for greet");
        assert!(db.unwrap().description.contains("Greets"));
    }

    #[test]
    fn find_docblock_returns_none_without_docblock() {
        use crate::ast::ParsedDoc;
        let src = "<?php\nfunction greet() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let db = find_docblock(src, &doc.program().stmts, "greet");
        assert!(db.is_none());
    }

    #[test]
    fn empty_docblock_gives_defaults() {
        let db = parse_docblock("/** */");
        assert_eq!(db.description, "");
        assert!(db.return_type.is_none());
        assert!(db.params.is_empty());
    }

    #[test]
    fn parses_deprecated_with_message() {
        let raw = "/**\n * @deprecated Use newMethod() instead\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.deprecated.as_deref(), Some("Use newMethod() instead"));
        assert!(db.is_deprecated());
    }

    #[test]
    fn parses_deprecated_without_message() {
        let raw = "/** @deprecated */";
        let db = parse_docblock(raw);
        assert_eq!(db.deprecated.as_deref(), Some(""));
        assert!(db.is_deprecated());
    }

    #[test]
    fn not_deprecated_when_tag_absent() {
        let raw = "/** Does stuff. */";
        let db = parse_docblock(raw);
        assert!(!db.is_deprecated());
    }

    #[test]
    fn parses_throws_tag() {
        let raw = "/**\n * @throws RuntimeException When something fails\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.throws.len(), 1);
        assert_eq!(db.throws[0].class, "RuntimeException");
        assert_eq!(db.throws[0].description, "When something fails");
    }

    #[test]
    fn parses_multiple_throws() {
        let raw = "/**\n * @throws InvalidArgumentException\n * @throws RuntimeException Bad state\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.throws.len(), 2);
        assert_eq!(db.throws[0].class, "InvalidArgumentException");
        assert_eq!(db.throws[1].class, "RuntimeException");
    }

    #[test]
    fn parses_see_tag() {
        let raw = "/**\n * @see OtherClass::method()\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.see.len(), 1);
        assert_eq!(db.see[0], "OtherClass::method()");
    }

    #[test]
    fn parses_link_tag() {
        let raw = "/**\n * @link https://example.com/docs\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.see.len(), 1);
        assert_eq!(db.see[0], "https://example.com/docs");
    }

    #[test]
    fn to_markdown_shows_deprecated_banner() {
        let db = Docblock {
            deprecated: Some("Use bar() instead".to_string()),
            description: "Does foo.".to_string(),
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(md.contains("> **Deprecated**"), "expected deprecated banner, got: {}", md);
        assert!(md.contains("Use bar() instead"), "expected deprecation message, got: {}", md);
    }

    #[test]
    fn to_markdown_shows_throws() {
        let db = Docblock {
            throws: vec![DocThrows {
                class: "RuntimeException".to_string(),
                description: "On failure".to_string(),
            }],
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(md.contains("@throws"), "expected @throws in markdown, got: {}", md);
        assert!(md.contains("RuntimeException"), "expected class name, got: {}", md);
    }

    #[test]
    fn to_markdown_shows_see() {
        let db = Docblock {
            see: vec!["https://example.com".to_string()],
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(md.contains("@see"), "expected @see in markdown, got: {}", md);
        assert!(md.contains("https://example.com"), "expected url, got: {}", md);
    }
}
