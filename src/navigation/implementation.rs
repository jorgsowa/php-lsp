/// `textDocument/implementation` — name-matching helpers shared by the
/// type-hierarchy walkers. Implementation lookups themselves are answered
/// from mir's subtype edge index (`DocumentStore::indexed_subtype_classes`).
/// Returns `true` when the name written in an `extends`/`implements` clause
/// (given as its `to_string_repr()` string) refers to the symbol we are
/// searching for.
///
/// Three forms are accepted:
/// - Short-name match: `repr == word`
///   Covers the common case where both files use the same unqualified name.
/// - FQN match: `repr` (with any leading `\` stripped) `== fqn`
///   Covers files that write the fully-qualified form (`\App\Animal` or
///   `App\Animal`) while the cursor file imports the class with a `use`
///   statement and the cursor sits on the short alias.
/// - Global-namespace backslash match: `repr.trim_start_matches('\\') == word`
///   when `fqn` is `None` and `word` has no namespace separator.
///   Covers the case where a class writes `extends \Animal` (explicit global-
///   namespace form) and the cursor sits on a global-namespace `Animal`
///   interface with no `use` import.
#[inline]
pub(crate) fn name_matches(repr: &str, word: &str, fqn: Option<&str>) -> bool {
    repr == word
        || fqn.is_some_and(|f| repr.trim_start_matches('\\') == f)
        || (fqn.is_none() && !word.contains('\\') && repr.trim_start_matches('\\') == word)
}

/// Returns `true` when `name` is a use-import alias in `use_imports` that
/// resolves to `fqn`. Handles both `Ns\Name` and `\Ns\Name` stored forms.
pub(crate) fn alias_resolves_to(
    name: &str,
    fqn: &str,
    use_imports: &[(Box<str>, Box<str>)],
) -> bool {
    use_imports.iter().any(|(alias, resolved)| {
        alias.as_ref() == name
            && (resolved.as_ref() == fqn || resolved.trim_start_matches('\\') == fqn)
    })
}

/// Returns `true` when `written` — a name from an `extends`/`implements`/`use`
/// clause in `idx`'s file — refers to `target_fqn`.
///
/// Unlike [`name_matches`], this does not treat a bare short-name match as
/// automatically correct: when `written` has no explicit FQN form, it is
/// resolved through `idx.use_imports` first, and an entry found there is
/// authoritative even if its resolved FQN differs from `target_fqn` (an
/// explicit import always shadows a same-named symbol elsewhere). Only when
/// no import shadows `written` does it fall back to implicit same-namespace
/// resolution, mirroring how PHP resolves an unqualified class name with no
/// matching `use` statement to `<current-namespace>\<written>`.
///
/// This is the disambiguation the raw `subtypes_of` short-name prefilter
/// cannot do on its own: many unrelated classes across a large workspace can
/// share both a short name (e.g. `Factory`) and a same-named `use` alias
/// (e.g. `FactoryContract`) while resolving to entirely different FQNs.
pub(crate) fn resolves_to_fqn(
    written: &str,
    target_fqn: &str,
    idx: &crate::index::file_index::FileIndex,
) -> bool {
    idx.resolve_name_to_fqn(written) == target_fqn
}
