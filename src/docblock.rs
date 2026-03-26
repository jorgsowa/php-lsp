/// Docblock (`/** ... */`) parser.
///
/// Strips the `/**` / `*/` markers and leading `*` from each line, then
/// extracts `@param`, `@return`, `@var`, `@throws`, `@deprecated`, `@see`,
/// `@link`, `@template`, and `@mixin` annotations.

#[derive(Debug, Default, PartialEq)]
pub struct Docblock {
    /// Free-text description (lines before the first `@` tag).
    pub description: String,
    /// `@param  TypeHint  $name  description`
    pub params: Vec<DocParam>,
    /// `@return  TypeHint  description`
    pub return_type: Option<DocReturn>,
    /// `@var  TypeHint` or `@var  TypeHint  $varName`
    pub var_type: Option<String>,
    /// Variable name from `@var TypeHint $varName`, if present.
    pub var_name: Option<String>,
    /// `@deprecated  message`  — `Some("")` when present without a message.
    pub deprecated: Option<String>,
    /// `@throws  ClassName  description`
    pub throws: Vec<DocThrows>,
    /// `@see target` and `@link url`
    pub see: Vec<String>,
    /// `@template T` or `@template T of BaseClass`
    pub templates: Vec<DocTemplate>,
    /// `@mixin ClassName`
    pub mixins: Vec<String>,
    /// `@psalm-type Alias = TypeExpr` / `@phpstan-type Alias = TypeExpr`
    pub type_aliases: Vec<DocTypeAlias>,
}

#[derive(Debug, PartialEq)]
pub struct DocTypeAlias {
    /// Alias name, e.g. `UserId`.
    pub name: String,
    /// Right-hand side type expression, e.g. `string|int`.
    pub type_expr: String,
}

#[derive(Debug, PartialEq)]
pub struct DocTemplate {
    /// Template parameter name, e.g. `T`.
    pub name: String,
    /// Optional upper bound, e.g. `Base` from `@template T of Base`.
    pub bound: Option<String>,
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
        for t in &self.templates {
            if let Some(bound) = &t.bound {
                out.push_str(&format!("**@template** `{}` of `{}`\n", t.name, bound));
            } else {
                out.push_str(&format!("**@template** `{}`\n", t.name));
            }
        }
        for m in &self.mixins {
            out.push_str(&format!("**@mixin** `{}`\n", m));
        }
        for ta in &self.type_aliases {
            if ta.type_expr.is_empty() {
                out.push_str(&format!("**@type** `{}`\n", ta.name));
            } else {
                out.push_str(&format!("**@type** `{}` = `{}`\n", ta.name, ta.type_expr));
            }
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
    let mut var_name: Option<String> = None;
    let mut deprecated: Option<String> = None;
    let mut throws: Vec<DocThrows> = Vec::new();
    let mut see: Vec<String> = Vec::new();
    let mut templates: Vec<DocTemplate> = Vec::new();
    let mut mixins: Vec<String> = Vec::new();
    let mut type_aliases: Vec<DocTypeAlias> = Vec::new();

    for line in inner.lines() {
        let line = line.trim();
        let line = line.strip_prefix('*').unwrap_or(line).trim();

        if line.starts_with('@') {
            let mut parts = line[1..].splitn(2, char::is_whitespace);
            let tag = parts.next().unwrap_or("").to_lowercase();
            let rest = parts.next().unwrap_or("").trim();

            match tag.as_str() {
                "param" => {
                    let (type_hint, rest) = split_type_hint(rest);
                    let (name, desc) = split_first_word(rest);
                    params.push(DocParam {
                        type_hint: type_hint.to_string(),
                        name: name.to_string(),
                        description: desc.trim().to_string(),
                    });
                }
                "return" | "returns" => {
                    let (type_hint, desc) = split_type_hint(rest);
                    return_type = Some(DocReturn {
                        type_hint: type_hint.to_string(),
                        description: desc.trim().to_string(),
                    });
                }
                "var" => {
                    let (type_hint, remainder) = split_type_hint(rest);
                    var_type = Some(type_hint.to_string());
                    // Optional `$varName` after the type hint.
                    let vname = remainder.trim();
                    if vname.starts_with('$') {
                        let name: String = vname[1..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                        if !name.is_empty() {
                            var_name = Some(name);
                        }
                    }
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
                "template" => {
                    // @template T  or  @template T of BaseClass
                    let (name, rest) = split_first_word(rest);
                    if !name.is_empty() {
                        let rest = rest.trim();
                        let bound = if rest.to_lowercase().starts_with("of ") {
                            let (b, _) = split_first_word(&rest[3..]);
                            if b.is_empty() { None } else { Some(b.to_string()) }
                        } else {
                            None
                        };
                        templates.push(DocTemplate { name: name.to_string(), bound });
                    }
                }
                "mixin" => {
                    let (class, _) = split_first_word(rest);
                    if !class.is_empty() {
                        mixins.push(class.to_string());
                    }
                }
                // Psalm / PHPStan type aliases: @psalm-type Alias = TypeExpr
                "psalm-type" | "phpstan-type" => {
                    let (name, rest2) = split_first_word(rest);
                    if !name.is_empty() {
                        let type_expr = rest2.trim().trim_start_matches('=').trim().to_string();
                        type_aliases.push(DocTypeAlias {
                            name: name.to_string(),
                            type_expr,
                        });
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
        var_name,
        deprecated,
        throws,
        see,
        templates,
        mixins,
        type_aliases,
    }
}

fn split_first_word(s: &str) -> (&str, &str) {
    let s = s.trim();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

/// Like `split_first_word` but respects balanced parentheses so that
/// `callable(int, string): void $x desc` splits into
/// `callable(int, string): void` and `$x desc`.
///
/// Handles the PSR-5 callable return-type syntax: after `): ` the next word
/// is part of the type hint, not the description.
fn split_type_hint(s: &str) -> (&str, &str) {
    let s = s.trim();
    let mut depth: usize = 0;
    let mut first_boundary: Option<usize> = None;

    for (i, c) in s.char_indices() {
        match c {
            '(' | '<' | '[' => depth += 1,
            ')' | '>' | ']' => depth = depth.saturating_sub(1),
            c if c.is_whitespace() && depth == 0 => {
                first_boundary = Some(i);
                break;
            }
            _ => {}
        }
    }

    let i = match first_boundary {
        Some(i) => i,
        None => return (s, ""),
    };

    let type_hint = &s[..i];
    let after = &s[i..]; // includes leading whitespace

    // Callable return-type: `callable(int, string): void $x`.
    // The token ending in `:` means the return type follows after whitespace.
    if type_hint.ends_with(':') {
        let rest = after.trim_start();
        // Only extend if the next token looks like a type (not a `$variable`).
        if !rest.is_empty() && !rest.starts_with('$') {
            // Find where the return-type word ends.
            let (ret, _) = split_first_word(rest);
            if !ret.is_empty() {
                let rest_offset = rest.as_ptr() as usize - s.as_ptr() as usize;
                let ret_end = rest_offset + ret.len();
                return (&s[..ret_end], &s[ret_end..]);
            }
        }
    }

    (type_hint, after)
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

    #[test]
    fn parses_template_tag() {
        let raw = "/**\n * @template T\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.templates.len(), 1);
        assert_eq!(db.templates[0].name, "T");
        assert!(db.templates[0].bound.is_none());
    }

    #[test]
    fn parses_template_with_bound() {
        let raw = "/**\n * @template T of BaseClass\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.templates.len(), 1);
        assert_eq!(db.templates[0].name, "T");
        assert_eq!(db.templates[0].bound.as_deref(), Some("BaseClass"));
    }

    #[test]
    fn parses_mixin_tag() {
        let raw = "/**\n * @mixin SomeTrait\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.mixins.len(), 1);
        assert_eq!(db.mixins[0], "SomeTrait");
    }

    #[test]
    fn parses_callable_param() {
        let raw = "/**\n * @param callable(int, string): void $fn The callback\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.params.len(), 1);
        assert_eq!(db.params[0].type_hint, "callable(int, string): void");
        assert_eq!(db.params[0].name, "$fn");
        assert_eq!(db.params[0].description, "The callback");
    }

    #[test]
    fn to_markdown_shows_template() {
        let db = Docblock {
            templates: vec![DocTemplate { name: "T".to_string(), bound: Some("Base".to_string()) }],
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(md.contains("@template"), "expected @template in markdown, got: {}", md);
        assert!(md.contains("T"), "expected T in markdown");
        assert!(md.contains("Base"), "expected Base in markdown");
    }

    #[test]
    fn to_markdown_shows_mixin() {
        let db = Docblock {
            mixins: vec!["SomeTrait".to_string()],
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(md.contains("@mixin"), "expected @mixin in markdown, got: {}", md);
        assert!(md.contains("SomeTrait"), "expected SomeTrait in markdown");
    }

    #[test]
    fn parses_psalm_type_alias() {
        let raw = "/**\n * @psalm-type UserId = string|int\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.type_aliases.len(), 1);
        assert_eq!(db.type_aliases[0].name, "UserId");
        assert_eq!(db.type_aliases[0].type_expr, "string|int");
    }

    #[test]
    fn parses_phpstan_type_alias() {
        let raw = "/** @phpstan-type Row = array{id: int, name: string} */";
        let db = parse_docblock(raw);
        assert_eq!(db.type_aliases.len(), 1);
        assert_eq!(db.type_aliases[0].name, "Row");
        assert!(db.type_aliases[0].type_expr.contains("array"));
    }

    #[test]
    fn to_markdown_shows_type_alias() {
        let db = Docblock {
            type_aliases: vec![DocTypeAlias {
                name: "Status".to_string(),
                type_expr: "string".to_string(),
            }],
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(md.contains("Status"), "expected alias name in markdown");
        assert!(md.contains("string"), "expected type expr in markdown");
    }
}
