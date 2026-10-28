use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mir_analyzer::Visibility;
use mir_analyzer::db::ClassLike;
use tower_lsp_server::ls_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Position};

use crate::document::ast::ParsedDoc;
use crate::document::document_store::DocumentStore;
use crate::navigation::moniker::resolve_fqn;
use crate::text::utf16_offset_to_byte;
use crate::types::type_map::{
    ClassMembers, enclosing_class_at, members_of_class, mixin_classes_of, parent_class_name,
    resolve_class_ref,
};

use super::callable_item;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberAccess {
    /// `$obj->`: instance methods and properties.
    Instance,
    /// `Class::`: static methods and properties, constants.
    Static,
}

pub(super) fn all_instance_members(
    class_name: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    find_class_doc: Option<super::ClassDocLookup<'_>>,
    session: Option<&DocumentStore>,
    scope_class: Option<&str>,
) -> Vec<CompletionItem> {
    all_members(
        class_name,
        doc,
        other_docs,
        find_class_doc,
        session,
        MemberAccess::Instance,
        scope_class,
    )
}

pub(super) fn all_static_members(
    class_name: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    find_class_doc: Option<super::ClassDocLookup<'_>>,
    session: Option<&DocumentStore>,
    scope_class: Option<&str>,
) -> Vec<CompletionItem> {
    all_members(
        class_name,
        doc,
        other_docs,
        find_class_doc,
        session,
        MemberAccess::Static,
        scope_class,
    )
}

/// Visibility check relative to the class the cursor sits in.
#[derive(Clone, Copy)]
struct Scope {
    /// The cursor is inside some class body.
    in_class: bool,
    /// The cursor is inside the receiver class itself.
    in_root_class: bool,
}

impl Scope {
    fn new(class_name: &str, scope_class: Option<&str>) -> Self {
        let short = |n: &str| n.rsplit('\\').next().unwrap_or(n).to_owned();
        Self {
            in_class: scope_class.is_some(),
            in_root_class: scope_class
                .is_some_and(|s| short(s).eq_ignore_ascii_case(&short(class_name))),
        }
    }

    /// Protected members are approximated as visible from any class body.
    fn hides(self, visibility: Visibility, inherited: bool) -> bool {
        match visibility {
            Visibility::Public => false,
            Visibility::Protected => !self.in_class,
            Visibility::Private => inherited || !self.in_root_class,
        }
    }
}

/// Common class-hierarchy traversal for both instance (`->`) and static (`::`)
/// member completion. `access` selects which subset of members to surface.
///
/// Private members are offered only inside the receiver class itself, and
/// never when inherited; protected ones only inside a class (`scope_class`).
///
/// Each class is read from mir's class table when `session` knows it, else
/// from the AST of its defining document.
fn all_members(
    class_name: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    find_class_doc: Option<super::ClassDocLookup<'_>>,
    session: Option<&DocumentStore>,
    access: MemberAccess,
    scope_class: Option<&str>,
) -> Vec<CompletionItem> {
    let is_static = access != MemberAccess::Instance;
    let scope = Scope::new(class_name, scope_class);
    let wants_method = |name: &str, meth_is_static: bool| match access {
        MemberAccess::Instance => !meth_is_static && !name.eq_ignore_ascii_case("__construct"),
        MemberAccess::Static => meth_is_static,
    };
    let all: Vec<&ParsedDoc> = std::iter::once(doc)
        .chain(other_docs.iter().map(|d| d.as_ref()))
        .collect();
    let mut items = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: Vec<(String, bool)> = vec![(class_name.to_string(), false)];
    let mut is_root = true;
    while let Some((current, inherited)) = queue.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        let class_like = session.and_then(|docs| mir_class_like(docs, doc, &current, is_root));
        is_root = false;
        if let Some(class_like) = class_like {
            push_mir_members(
                &class_like,
                inherited,
                is_static,
                &wants_method,
                scope,
                &mut items,
                &mut seen_names,
            );
            let own_traits = class_like.class_traits();
            let mut next: Vec<(String, bool)> = class_like
                .ancestor_fqcns()
                .into_iter()
                .map(|a| {
                    // Private members of a class's own traits stay visible to it.
                    let via_own_trait = own_traits.contains(&a);
                    (a.to_string(), inherited || !via_own_trait)
                })
                .collect();
            if !is_static {
                next.extend(class_like.mixins().iter().map(|m| (m.to_string(), true)));
            }
            // `queue` is a stack: push in reverse so traits are visited before the parent.
            queue.extend(next.into_iter().rev());
            continue;
        }

        // `current` may be a bare short name or a full FQN (an import-
        // resolved receiver keeps its FQN so `find_class_doc` can pick the
        // right same-named class from a different namespace — see
        // `resolve_static_receiver`). AST-level matching (`members_of_class`
        // and friends below) always compares against the short name as
        // declared in source, so that's what they get; `find_class_doc`
        // wants the disambiguating full name.
        let short: &str = current.rsplit('\\').next().unwrap_or(&current);

        // Fast path: workspace-index lookup gives O(1) access to the one doc
        // that defines `current`. Fallback: scan all docs linearly (original
        // behaviour, used when the index is unavailable or the class is not
        // yet indexed, e.g. unsaved files).
        let fast_doc: Option<Arc<ParsedDoc>> = find_class_doc.and_then(|f| f(&current));
        let fast_ref: Option<&ParsedDoc> = fast_doc.as_deref();
        let defining: Option<(&ParsedDoc, ClassMembers)> = if let Some(fd) = fast_ref {
            let m = members_of_class(fd, &current);
            m.found.then_some((fd, m))
        } else {
            // PHP defines a class in exactly one file, so stop scanning once
            // the defining doc is hit.
            all.iter().find_map(|d| {
                let m = members_of_class(d, &current);
                m.found.then_some((*d, m))
            })
        };

        let Some((d, members)) = defining else {
            continue;
        };
        let parent = members
            .parent
            .as_deref()
            .map(|p| resolve_class_ref(d, short, p));
        let hidden = |restricted: &[(String, bool)], name: &str| {
            restricted.iter().any(|(n, private)| {
                let visibility = if *private {
                    Visibility::Private
                } else {
                    Visibility::Protected
                };
                n == name && scope.hides(visibility, inherited)
            })
        };
        for (name, meth_is_static, has_params) in members.methods {
            if wants_method(&name, meth_is_static)
                && !hidden(&members.restricted_methods, &name)
                && seen_names.insert(name.clone())
            {
                let mut item = callable_item(&name, CompletionItemKind::METHOD, has_params);
                // Disambiguates completion_resolve's lookup when another
                // class elsewhere in the workspace declares a method of
                // the same name — see resolve_static_receiver's FQN note.
                item.data = Some(serde_json::json!({ "class": current }));
                items.push(item);
            }
        }
        for (name, prop_is_static) in &members.properties {
            if *prop_is_static == is_static && !hidden(&members.restricted_properties, name) {
                let label = format!("${name}");
                if seen_names.insert(label.clone()) {
                    let detail = if !is_static && members.readonly_properties.contains(name) {
                        Some("readonly".to_string())
                    } else {
                        None
                    };
                    // Static access (Class::$prop) keeps "$name" as the inserted text because
                    // the $ is syntactically required. Instance access ($obj->prop) must NOT
                    // insert the $, or the client produces `$obj->$prop` (invalid PHP).
                    let insert_text = if is_static { None } else { Some(name.clone()) };
                    items.push(CompletionItem {
                        label,
                        kind: Some(CompletionItemKind::PROPERTY),
                        detail,
                        insert_text,
                        ..Default::default()
                    });
                }
            }
        }
        if is_static {
            for name in members.constants {
                if !hidden(&members.restricted_constants, &name) && seen_names.insert(name.clone())
                {
                    items.push(CompletionItem {
                        label: name,
                        kind: Some(CompletionItemKind::CONSTANT),
                        ..Default::default()
                    });
                }
            }
        } else {
            for mixin in mixin_classes_of(d, short) {
                queue.push((resolve_class_ref(d, short, &mixin), true));
            }
        }
        for trait_name in members.trait_uses {
            queue.push((resolve_class_ref(d, short, &trait_name), inherited));
        }
        if let Some(p) = parent {
            queue.push((p, true));
        }
    }
    items
}

/// Look up `name` in mir. A bare root name is tried against the current
/// file's namespace and imports first, then as a global name.
fn mir_class_like(
    docs: &DocumentStore,
    doc: &ParsedDoc,
    name: &str,
    is_root: bool,
) -> Option<ClassLike> {
    if !is_root {
        return docs.find_class_like(name);
    }
    mir_name_candidates(doc, name).find_map(|fqcn| docs.find_class_like(&fqcn))
}

/// Parameter names of `method` as mir resolves it on `class`, inherited,
/// trait and `@mixin` methods included.
pub(super) fn mir_method_params(
    docs: &DocumentStore,
    doc: &ParsedDoc,
    class: &str,
    method: &str,
) -> Option<Vec<String>> {
    let method = mir_name_candidates(doc, class)
        .find_map(|fqcn| docs.find_method_in_chain(&fqcn, method))?;
    Some(
        method
            .params
            .iter()
            .map(|p| p.name.as_str().trim_start_matches('$'))
            // mir's synthetic variadic for `func_get_args()` bodies.
            .filter(|name| *name != "...")
            .map(str::to_owned)
            .collect(),
    )
}

/// Spellings of a class name to try in mir, most specific first: a bare name
/// resolved against the current file's namespace and imports, then as written.
fn mir_name_candidates(doc: &ParsedDoc, name: &str) -> impl Iterator<Item = String> {
    let resolved = (!name.contains('\\'))
        .then(|| resolve_fqn(doc, name, &doc.file_imports()))
        .filter(|resolved| resolved != name);
    resolved.into_iter().chain(std::iter::once(name.to_owned()))
}

fn push_mir_members(
    class_like: &ClassLike,
    inherited: bool,
    is_static: bool,
    wants_method: &dyn Fn(&str, bool) -> bool,
    scope: Scope,
    items: &mut Vec<CompletionItem>,
    seen_names: &mut HashSet<String>,
) {
    let hidden = |visibility: Visibility| scope.hides(visibility, inherited);
    let owner = class_like.fqcn().to_string();
    for method in class_like.own_methods().values() {
        if wants_method(&method.name, method.is_static)
            && !hidden(method.visibility)
            && seen_names.insert(method.name.to_string())
        {
            let mut item = callable_item(
                &method.name,
                CompletionItemKind::METHOD,
                !method.params.is_empty(),
            );
            item.data = Some(serde_json::json!({ "class": owner }));
            items.push(item);
        }
    }
    for prop in class_like
        .own_properties()
        .into_iter()
        .flat_map(|p| p.values())
    {
        if prop.is_static != is_static || hidden(prop.visibility) {
            continue;
        }
        let label = format!("${}", prop.name);
        if seen_names.insert(label.clone()) {
            let detail = (!is_static && prop.has_native_readonly).then(|| "readonly".to_string());
            // `Class::$prop` needs the `$`; `$obj->prop` must not get it.
            let insert_text = (!is_static).then(|| prop.name.to_string());
            items.push(CompletionItem {
                label,
                kind: Some(CompletionItemKind::PROPERTY),
                detail,
                insert_text,
                ..Default::default()
            });
        }
    }
    if is_static {
        let cases = match class_like {
            ClassLike::Enum(e) => e.cases.keys().cloned().collect(),
            _ => Vec::new(),
        };
        let constants = class_like
            .own_constants()
            .values()
            .filter(|c| !c.visibility.is_some_and(hidden))
            .map(|c| c.name.clone());
        for name in cases.into_iter().chain(constants) {
            if seen_names.insert(name.to_string()) {
                items.push(CompletionItem {
                    label: name.to_string(),
                    kind: Some(CompletionItemKind::CONSTANT),
                    ..Default::default()
                });
            }
        }
    }
}

/// Resolve `ClassName::` or the aliases `self::`, `static::`, `parent::`.
///
/// `imports` is the file's `use`-import map (`alias → FQCN`). When the name
/// before `::` is an import alias (including `as`-renamed aliases such as
/// `use Foo\Bar as Baz`), the short class name extracted from the FQCN is
/// returned so that `all_static_members` can find the class in the workspace
/// index by its canonical short name.
pub(super) fn resolve_static_receiver(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    position: Position,
    imports: &HashMap<String, String>,
) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..col];
    let before = before.strip_suffix("::").unwrap_or(before);
    let name: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '\\')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    match name.as_str() {
        "" => None,
        "self" | "static" => enclosing_class_at(source, doc, position),
        "parent" => {
            let enclosing = enclosing_class_at(source, doc, position)?;
            // Look for the parent class in current doc then other docs
            if let Some(p) = parent_class_name(doc, &enclosing) {
                return Some(p);
            }
            for other in other_docs {
                if let Some(p) = parent_class_name(other, &enclosing) {
                    return Some(p);
                }
            }
            None
        }
        _ => {
            // If the name contains a backslash it's already a FQN — return it
            // as-is (minus a leading `\`) rather than collapsing to the short
            // name, so callers can disambiguate same-named classes in
            // different namespaces instead of matching the first class with
            // that short name found anywhere in the workspace.
            if name.contains('\\') {
                return Some(name.trim_start_matches('\\').to_owned());
            }
            // Otherwise try to expand a `use`-import alias. This handles both
            // the unaliased case (`use Foo\Bar\Str` → alias `Str` = short name `Str`)
            // and the `as`-aliased case (`use Foo\Bar\Str as Helper` → alias `Helper`
            // maps to FQN `Foo\Bar\Str`, short name `Str`). Keep the full FQN
            // (not just its short name) so a namespace collision — e.g.
            // Laravel's `Illuminate\Support\Facades\Auth` vs `Illuminate\
            // Container\Attributes\Auth` — resolves to the imported class,
            // not whichever same-named class the workspace index hits first.
            if let Some(fqcn) = imports.get(&name) {
                return Some(fqcn.trim_start_matches('\\').to_owned());
            }
            Some(name)
        }
    }
}

const PHP_MAGIC_METHODS: &[(&str, &str)] = &[
    (
        "__construct",
        "public function __construct($1)\n{\n    $2\n}",
    ),
    ("__destruct", "public function __destruct()\n{\n    $1\n}"),
    (
        "__get",
        "public function __get(string $name): mixed\n{\n    $1\n}",
    ),
    (
        "__set",
        "public function __set(string $name, mixed $value): void\n{\n    $1\n}",
    ),
    (
        "__isset",
        "public function __isset(string $name): bool\n{\n    $1\n}",
    ),
    (
        "__unset",
        "public function __unset(string $name): void\n{\n    $1\n}",
    ),
    (
        "__call",
        "public function __call(string $name, array $arguments): mixed\n{\n    $1\n}",
    ),
    (
        "__callStatic",
        "public static function __callStatic(string $name, array $arguments): mixed\n{\n    $1\n}",
    ),
    (
        "__toString",
        "public function __toString(): string\n{\n    $1\n}",
    ),
    (
        "__invoke",
        "public function __invoke($1): mixed\n{\n    $2\n}",
    ),
    ("__clone", "public function __clone(): void\n{\n    $1\n}"),
    ("__sleep", "public function __sleep(): array\n{\n    $1\n}"),
    ("__wakeup", "public function __wakeup(): void\n{\n    $1\n}"),
    (
        "__serialize",
        "public function __serialize(): array\n{\n    $1\n}",
    ),
    (
        "__unserialize",
        "public function __unserialize(array $data): void\n{\n    $1\n}",
    ),
    (
        "__debugInfo",
        "public function __debugInfo(): ?array\n{\n    $1\n}",
    ),
];

pub(super) fn magic_method_completions() -> Vec<CompletionItem> {
    PHP_MAGIC_METHODS
        .iter()
        .map(|(name, snippet)| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::METHOD),
            insert_text: Some(snippet.to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            detail: Some("magic method".to_string()),
            ..Default::default()
        })
        .collect()
}

pub(super) fn resolve_receiver_class(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    analysis: Option<&mir_analyzer::FileAnalysis>,
) -> Option<Vec<String>> {
    let line = source.lines().nth(position.line as usize)?;
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..col];
    // Try ?-> first (longer pattern) so `$s?->` doesn't get stripped to `$s?` by the `->` rule.
    let before = before
        .strip_suffix("?->")
        .or_else(|| before.strip_suffix("->"))
        .unwrap_or(before);

    // Handle (new ClassName()) before ->
    if let Some(class_name) = extract_new_class_before_arrow(before) {
        // Preserve an explicit FQCN and expand a direct `use` alias before
        // member lookup. `all_members` may use a short name only as a source
        // spelling *within an already-selected document*; using one to select
        // the document makes same-named classes ambiguous.
        if class_name.starts_with('\\') || class_name.contains('\\') {
            return Some(vec![class_name.trim_start_matches('\\').to_owned()]);
        }
        let imports = doc.file_imports();
        if let Some(fqcn) = imports.get(&class_name) {
            return Some(vec![fqcn.trim_start_matches('\\').to_owned()]);
        }
        if let Some((_, fqcn)) = imports
            .iter()
            .find(|(alias, _)| alias.eq_ignore_ascii_case(&class_name))
        {
            return Some(vec![fqcn.trim_start_matches('\\').to_owned()]);
        }
        return Some(vec![class_name]);
    }

    let var_name: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '$')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if var_name.is_empty() {
        // Method-chain receiver like `$obj->getUser()->`: `before` ends with `)`,
        // so the take_while extractor collects nothing.  Ask mir for the type of
        // the call-expression at that position; mir records the return type there.
        let chain_offset = line_byte_offset(doc, position.line, before.len());
        return analysis.and_then(|a| receiver_class_at(a, chain_offset));
    }
    let var_name = if var_name.starts_with('$') {
        var_name
    } else {
        format!("${var_name}")
    };
    // `before` now ends with the receiver variable; its last byte lands strictly
    // inside the (end-exclusive, identifier-only) mir symbol span.
    let var_offset = line_byte_offset(doc, position.line, before.len());
    if var_name == "$this" {
        return enclosing_class_at(source, doc, position)
            .map(|c| vec![c])
            .or_else(|| analysis.and_then(|a| receiver_class_at(a, var_offset)));
    }
    analysis.and_then(|a| receiver_class_at(a, var_offset))
}

/// Resolve the class(es) of a receiver variable from mir's recorded symbol at
/// `var_offset` (a byte offset inside the variable token), preserving fully
/// qualified names for member lookup, one per union member. Keeping the namespace is essential: the workspace
/// can contain unrelated classes with the same short name, and
/// `all_members` uses the FQCN to select the right defining document.
/// Returns `None` if mir recorded no class-typed symbol there.
pub(super) fn receiver_class_at(
    analysis: &mir_analyzer::FileAnalysis,
    var_offset: u32,
) -> Option<Vec<String>> {
    let ty = crate::types::type_query::type_at_offset(analysis, var_offset)?;
    let names = crate::types::type_query::class_names(ty);
    (!names.is_empty()).then_some(names)
}

/// Doc byte offset of `byte_in_line` bytes into line `line`, minus one — i.e.
/// the last source byte of a token that ends at `byte_in_line`. Used to land an
/// offset strictly inside a receiver variable's end-exclusive mir span.
pub(super) fn line_byte_offset(doc: &ParsedDoc, line: u32, byte_in_line: usize) -> u32 {
    let line_start = doc.view().byte_of_position(Position { line, character: 0 });
    line_start + byte_in_line.saturating_sub(1) as u32
}

/// Extract the class name from `(new ClassName(...))` or `new ClassName(...)` text
/// appearing immediately before `->`.
fn extract_new_class_before_arrow(text: &str) -> Option<String> {
    let text = text.trim_end();
    // Strip optional closing paren wrapping: `(new Foo())`
    let inner = if let Some(without_last) = text.strip_suffix(')') {
        // Find matching open paren — look for `(new` pattern
        if let Some(pos) = without_last.rfind("(new ") {
            &without_last[pos + 1..]
        } else if let Some(pos) = without_last.rfind("(new\t") {
            &without_last[pos + 1..]
        } else {
            text
        }
    } else {
        text
    };
    // Now inner should start with `new ClassName(...)`
    let inner = inner.trim();
    if !inner.starts_with("new ") && !inner.starts_with("new\t") {
        return None;
    }
    let after_new = inner[3..].trim_start();
    // Extract class name (alphanumeric + _ + \)
    let class: String = after_new
        .chars()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '\\')
        .collect();
    if class.is_empty() {
        return None;
    }
    // Keep the exact source spelling. The caller resolves imports and only
    // accepts a short name when no stronger class identity is available.
    Some(class)
}
