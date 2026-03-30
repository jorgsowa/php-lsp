/// PHP type representation used throughout mir-php.
use std::collections::HashMap;

/// A PHP type.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    /// Type is not known / not yet inferred.
    Unknown,
    /// The `never` return type (function never returns normally).
    Never,
    /// The `void` return type.
    Void,
    /// `null`.
    Null,
    /// `bool`.
    Bool,
    /// `int`.
    Int,
    /// `float`.
    Float,
    /// `string`.
    Str,
    /// Untyped `array`.
    Array,
    /// `callable`.
    Callable,
    /// A named object type (fully-qualified or short class/interface name).
    Object(String),
    /// `T1|T2|…`
    Union(Vec<Ty>),
    /// `T1&T2&…`
    Intersection(Vec<Ty>),
}

impl Ty {
    /// Parse from a PHP type-hint string (e.g. `"?int"`, `"Foo|null"`).
    pub fn from_str(s: &str) -> Self {
        let s = s.trim();
        // DNF types: split on `|` respecting parenthesized groups like `(A&B)|C`
        if s.contains('|') {
            let parts: Vec<Ty> = split_union(s).iter().map(|p| Ty::from_str(p)).collect();
            return Ty::Union(parts);
        }
        // Intersection type: `A&B&C` (outside any parens at this point)
        if s.contains('&') && !s.starts_with('(') {
            let parts: Vec<Ty> = s.split('&').map(|p| Ty::from_str(p.trim())).collect();
            return Ty::Intersection(parts);
        }
        // Parenthesized intersection group: `(A&B)` — unwrap and parse as intersection
        if s.starts_with('(') && s.ends_with(')') {
            let inner = &s[1..s.len() - 1];
            let parts: Vec<Ty> = inner.split('&').map(|p| Ty::from_str(p.trim())).collect();
            return Ty::Intersection(parts);
        }
        if let Some(inner) = s.strip_prefix('?') {
            return Ty::Union(vec![Ty::from_str(inner), Ty::Null]);
        }
        match s {
            "null" | "NULL" => Ty::Null,
            "bool" | "boolean" => Ty::Bool,
            "int" | "integer" => Ty::Int,
            "float" | "double" => Ty::Float,
            "string" => Ty::Str,
            "array" => Ty::Array,
            "callable" => Ty::Callable,
            "void" => Ty::Void,
            "never" => Ty::Never,
            "mixed" | "" => Ty::Unknown,
            name => Ty::Object(name.to_string()),
        }
    }

    /// If this is a single named object type, return its class name.
    pub fn class_name(&self) -> Option<&str> {
        match self {
            Ty::Object(n) => Some(n),
            _ => None,
        }
    }

    /// True if `null` is a valid value for this type.
    pub fn is_nullable(&self) -> bool {
        match self {
            Ty::Null | Ty::Unknown => true,
            Ty::Union(ts) => ts.iter().any(|t| matches!(t, Ty::Null)),
            _ => false,
        }
    }
}

/// Split a union type string on `|` while respecting parenthesized groups.
/// `"(A&B)|C|null"` → `["(A&B)", "C", "null"]`
fn split_union(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth: usize = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => {
                parts.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].trim());
    parts
}

/// Maps variable name (with `$`) → inferred `Ty`.
#[derive(Debug, Default, Clone)]
pub struct TypeEnv(pub HashMap<String, Ty>);

impl TypeEnv {
    pub fn get(&self, var: &str) -> &Ty {
        self.0.get(var).unwrap_or(&Ty::Unknown)
    }

    /// Convenience: return the class name when the type is `Object(_)`.
    pub fn class_name(&self, var: &str) -> Option<&str> {
        self.0.get(var)?.class_name()
    }

    pub fn insert(&mut self, var: String, ty: Ty) {
        self.0.insert(var, ty);
    }
}
