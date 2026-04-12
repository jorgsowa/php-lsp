/// Docblock (`/** ... */`) parser.
///
/// Delegates to [`mir_analyzer::DocblockParser`] for type parsing and
/// [`php_rs_parser::phpdoc`] for description extraction.
use std::collections::HashMap;

use mir_analyzer::DocblockParser;
use php_rs_parser::phpdoc::{self, PhpDocTag};

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
    /// Free-text description after the type in `@var TypeHint description`.
    pub var_description: Option<String>,
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
    /// `@property Type $name` / `@property-read Type $name` / `@property-write Type $name`
    pub properties: Vec<DocProperty>,
    /// `@method [static] ReturnType name([params])`
    pub methods: Vec<DocMethod>,
}

#[derive(Debug, PartialEq)]
pub struct DocProperty {
    pub type_hint: String,
    pub name: String,    // without $
    pub read_only: bool, // true for @property-read
}

#[derive(Debug, PartialEq)]
pub struct DocMethod {
    pub return_type: String,
    pub name: String,
    pub is_static: bool,
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
        if let Some(vt) = &self.var_type {
            out.push_str(&format!("**@var** `{}`", vt));
            if let Some(vd) = &self.var_description
                && !vd.is_empty()
            {
                out.push_str(&format!(" — {}", vd));
            }
            out.push('\n');
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
///
/// Delegates to [`mir_analyzer::DocblockParser`] for type resolution and
/// [`php_rs_parser::phpdoc`] for description fields.
pub fn parse_docblock(raw: &str) -> Docblock {
    let mir = DocblockParser::parse(raw);
    let raw_doc = phpdoc::parse(raw);

    // Collect descriptions from the raw tags (mir discards them).
    let mut param_descs: HashMap<String, String> = HashMap::new();
    let mut return_desc = String::new();
    let mut throws_descs: Vec<String> = Vec::new();
    let mut var_desc: Option<String> = None;

    for tag in &raw_doc.tags {
        match tag {
            PhpDocTag::Param {
                name: Some(n),
                description: Some(d),
                ..
            } => {
                param_descs.insert(n.trim_start_matches('$').to_string(), d.to_string());
            }
            PhpDocTag::Return {
                description: Some(d),
                ..
            } => {
                return_desc = d.to_string();
            }
            PhpDocTag::Throws {
                type_str: Some(ts),
                description,
            } => {
                let class = ts.split_whitespace().next().unwrap_or("");
                if !class.is_empty() {
                    throws_descs.push(
                        description
                            .as_ref()
                            .map(|d| d.to_string())
                            .unwrap_or_default(),
                    );
                }
            }
            PhpDocTag::Var {
                description: Some(d),
                ..
            } => {
                var_desc = Some(d.to_string());
            }
            _ => {}
        }
    }

    let params: Vec<DocParam> = mir
        .params
        .iter()
        .map(|(name, union)| {
            let description = param_descs.get(name.as_str()).cloned().unwrap_or_default();
            DocParam {
                type_hint: union.to_string(),
                name: format!("${}", name),
                description,
            }
        })
        .collect();

    let return_type = mir.return_type.as_ref().map(|union| DocReturn {
        type_hint: union.to_string(),
        description: return_desc,
    });

    let throws: Vec<DocThrows> = mir
        .throws
        .iter()
        .enumerate()
        .map(|(i, class)| DocThrows {
            class: class.clone(),
            description: throws_descs.get(i).cloned().unwrap_or_default(),
        })
        .collect();

    let deprecated = if mir.is_deprecated {
        Some(mir.deprecated.as_deref().unwrap_or("").to_string())
    } else {
        None
    };

    let templates: Vec<DocTemplate> = mir
        .templates
        .iter()
        .map(|(name, bound, _variance)| DocTemplate {
            name: name.clone(),
            bound: bound.as_ref().map(|u| u.to_string()),
        })
        .collect();

    let properties: Vec<DocProperty> = mir
        .properties
        .iter()
        .map(|p| DocProperty {
            type_hint: p.type_hint.clone(),
            name: p.name.clone(),
            read_only: p.read_only,
        })
        .collect();

    let methods: Vec<DocMethod> = mir
        .methods
        .iter()
        .map(|m| DocMethod {
            return_type: m.return_type.clone(),
            name: m.name.clone(),
            is_static: m.is_static,
        })
        .collect();

    let type_aliases: Vec<DocTypeAlias> = mir
        .type_aliases
        .iter()
        .map(|ta| DocTypeAlias {
            name: ta.name.clone(),
            type_expr: ta.type_expr.clone(),
        })
        .collect();

    Docblock {
        description: mir.description.clone(),
        params,
        return_type,
        var_type: mir.var_type.as_ref().map(|u| u.to_string()),
        var_name: mir.var_name.clone(),
        var_description: var_desc,
        deprecated,
        throws,
        see: mir.see.clone(),
        templates,
        mixins: mir.mixins.clone(),
        type_aliases,
        properties,
        methods,
    }
}

/// Scan `source` for a `/** ... */` docblock that ends immediately before
/// `node_start` (byte offset). Whitespace between the `*/` and the node is
/// allowed; non-whitespace text in between disqualifies the block.
pub fn docblock_before(source: &str, node_start: u32) -> Option<String> {
    mir_analyzer::parser::find_preceding_docblock(source, node_start)
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
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == word
                    {
                        let raw = docblock_before(source, member.span.start)?;
                        return Some(parse_docblock(&raw));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(db) = find_docblock(source, inner, word)
                {
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
    fn parses_var_tag_with_description() {
        let raw = "/** @var string The user's name */";
        let db = parse_docblock(raw);
        assert_eq!(db.var_type.as_deref(), Some("string"));
        assert_eq!(db.var_description.as_deref(), Some("The user's name"));
    }

    #[test]
    fn to_markdown_shows_var_type() {
        let db = Docblock {
            var_type: Some("string".to_string()),
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(
            md.contains("@var"),
            "expected @var in markdown, got: {}",
            md
        );
        assert!(
            md.contains("string"),
            "expected type in markdown, got: {}",
            md
        );
    }

    #[test]
    fn to_markdown_shows_var_type_with_description() {
        let db = Docblock {
            var_type: Some("string".to_string()),
            var_description: Some("The user's name".to_string()),
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(
            md.contains("@var"),
            "expected @var in markdown, got: {}",
            md
        );
        assert!(
            md.contains("string"),
            "expected type in markdown, got: {}",
            md
        );
        assert!(
            md.contains("The user's name"),
            "expected description in markdown, got: {}",
            md
        );
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
        let raw =
            "/**\n * @throws InvalidArgumentException\n * @throws RuntimeException Bad state\n */";
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
        assert!(
            md.contains("> **Deprecated**"),
            "expected deprecated banner, got: {}",
            md
        );
        assert!(
            md.contains("Use bar() instead"),
            "expected deprecation message, got: {}",
            md
        );
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
        assert!(
            md.contains("@throws"),
            "expected @throws in markdown, got: {}",
            md
        );
        assert!(
            md.contains("RuntimeException"),
            "expected class name, got: {}",
            md
        );
    }

    #[test]
    fn to_markdown_shows_see() {
        let db = Docblock {
            see: vec!["https://example.com".to_string()],
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(
            md.contains("@see"),
            "expected @see in markdown, got: {}",
            md
        );
        assert!(
            md.contains("https://example.com"),
            "expected url, got: {}",
            md
        );
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
            templates: vec![DocTemplate {
                name: "T".to_string(),
                bound: Some("Base".to_string()),
            }],
            ..Default::default()
        };
        let md = db.to_markdown();
        assert!(
            md.contains("@template"),
            "expected @template in markdown, got: {}",
            md
        );
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
        assert!(
            md.contains("@mixin"),
            "expected @mixin in markdown, got: {}",
            md
        );
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

    #[test]
    fn parses_property_tag() {
        let src = "/** @property string $name */";
        let db = parse_docblock(src);
        assert_eq!(db.properties.len(), 1);
        assert_eq!(db.properties[0].name, "name");
        assert_eq!(db.properties[0].type_hint, "string");
        assert!(!db.properties[0].read_only);
    }

    #[test]
    fn parses_property_read_tag() {
        let src = "/** @property-read Carbon $createdAt */";
        let db = parse_docblock(src);
        assert_eq!(db.properties[0].name, "createdAt");
        assert!(db.properties[0].read_only);
    }

    #[test]
    fn parses_method_tag() {
        let src = "/** @method User find(int $id) */";
        let db = parse_docblock(src);
        assert_eq!(db.methods.len(), 1);
        assert_eq!(db.methods[0].name, "find");
        assert_eq!(db.methods[0].return_type, "User");
        assert!(!db.methods[0].is_static);
    }

    #[test]
    fn parses_static_method_tag() {
        let src = "/** @method static Builder where(string $col, mixed $val) */";
        let db = parse_docblock(src);
        assert!(db.methods[0].is_static);
        assert_eq!(db.methods[0].name, "where");
    }

    #[test]
    fn psalm_param_alias_parsed_as_param() {
        let raw = "/**\n * @psalm-param string $x The value\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.params.len(), 1);
        assert_eq!(db.params[0].type_hint, "string");
        assert_eq!(db.params[0].name, "$x");
    }

    #[test]
    fn phpstan_param_alias_parsed_as_param() {
        let raw = "/**\n * @phpstan-param int $count\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.params.len(), 1);
        assert_eq!(db.params[0].type_hint, "int");
        assert_eq!(db.params[0].name, "$count");
    }

    #[test]
    fn psalm_return_alias_parsed_as_return() {
        let raw = "/**\n * @psalm-return non-empty-string\n */";
        let db = parse_docblock(raw);
        assert_eq!(
            db.return_type.as_ref().map(|r| r.type_hint.as_str()),
            Some("non-empty-string")
        );
    }

    #[test]
    fn phpstan_return_alias_parsed_as_return() {
        let raw = "/**\n * @phpstan-return array<int, string>\n */";
        let db = parse_docblock(raw);
        assert_eq!(
            db.return_type.as_ref().map(|r| r.type_hint.as_str()),
            Some("array<int, string>")
        );
    }

    #[test]
    fn psalm_var_alias_parsed_as_var() {
        let raw = "/** @psalm-var Foo $item */";
        let db = parse_docblock(raw);
        assert_eq!(db.var_type.as_deref(), Some("Foo"));
        assert_eq!(db.var_name.as_deref(), Some("item"));
    }

    #[test]
    fn phpstan_var_alias_parsed_as_var() {
        let raw = "/** @phpstan-var string */";
        let db = parse_docblock(raw);
        assert_eq!(db.var_type.as_deref(), Some("string"));
    }

    #[test]
    fn param_without_description_parses_correctly() {
        let raw = "/**\n * @param string $x\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.params.len(), 1);
        assert_eq!(
            db.params[0].type_hint, "string",
            "type_hint should be 'string'"
        );
        assert_eq!(db.params[0].name, "$x", "name should be '$x'");
        assert_eq!(
            db.params[0].description, "",
            "description should be empty when absent"
        );
    }

    #[test]
    fn union_type_param_parsed() {
        let raw = "/**\n * @param Foo|Bar $x Some value\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.params.len(), 1);
        assert_eq!(
            db.params[0].type_hint, "Foo|Bar",
            "union type should be 'Foo|Bar', got: {}",
            db.params[0].type_hint
        );
        assert_eq!(db.params[0].name, "$x");
    }

    #[test]
    fn nullable_type_param_parsed() {
        // `?Foo` is normalized to the canonical `Foo|null` form.
        let raw = "/**\n * @param ?Foo $x\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.params.len(), 1);
        assert_eq!(
            db.params[0].type_hint, "Foo|null",
            "nullable type should be 'Foo|null', got: {}",
            db.params[0].type_hint
        );
        assert_eq!(db.params[0].name, "$x");
    }

    #[test]
    fn method_tag_extracts_return_type() {
        let raw = "/**\n * @method string getName()\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.methods.len(), 1);
        assert_eq!(
            db.methods[0].return_type, "string",
            "return_type should be 'string', got: {}",
            db.methods[0].return_type
        );
        assert_eq!(
            db.methods[0].name, "getName",
            "name should be 'getName', got: {}",
            db.methods[0].name
        );
        assert!(!db.methods[0].is_static, "should not be static");
    }

    #[test]
    fn advanced_type_non_empty_string() {
        // mir resolves psalm/phpstan special types; non-empty-string must round-trip.
        let raw = "/**\n * @return non-empty-string\n */";
        let db = parse_docblock(raw);
        assert_eq!(
            db.return_type.as_ref().map(|r| r.type_hint.as_str()),
            Some("non-empty-string"),
            "non-empty-string should be preserved, got: {:?}",
            db.return_type
        );
    }

    #[test]
    fn advanced_type_generic_array() {
        // array<K, V> generic syntax must round-trip through mir's Union display.
        let raw = "/**\n * @param array<int, string> $map\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.params.len(), 1);
        assert_eq!(
            db.params[0].type_hint, "array<int, string>",
            "generic array type should be preserved, got: {}",
            db.params[0].type_hint
        );
    }

    #[test]
    fn param_and_return_descriptions_preserved() {
        // Descriptions from @param and @return are captured via php-rs-parser
        // (mir discards them). Verify they survive the full parse_docblock() call.
        let raw = "/**\n * @param string $name The user name\n * @return int The age\n */";
        let db = parse_docblock(raw);
        assert_eq!(
            db.params[0].description, "The user name",
            "param description should be preserved"
        );
        assert_eq!(
            db.return_type.as_ref().map(|r| r.description.as_str()),
            Some("The age"),
            "return description should be preserved"
        );
    }

    #[test]
    fn throws_description_preserved() {
        // @throws description must survive the adapter (mir only stores the class).
        let raw = "/**\n * @throws RuntimeException When the server is down\n */";
        let db = parse_docblock(raw);
        assert_eq!(db.throws.len(), 1);
        assert_eq!(db.throws[0].class, "RuntimeException");
        assert_eq!(
            db.throws[0].description, "When the server is down",
            "throws description should be preserved"
        );
    }
}
