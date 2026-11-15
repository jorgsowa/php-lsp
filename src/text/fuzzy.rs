//! Fuzzy matching for completion and workspace-symbol filtering.

/// A fuzzy-match query with its lowercase form computed once per request.
/// Per-candidate checks allocate nothing (char-iterator comparisons).
pub(crate) struct FuzzyQuery {
    lower: String,
}

impl FuzzyQuery {
    pub(crate) fn new(query: &str) -> Self {
        FuzzyQuery {
            lower: query.to_lowercase(),
        }
    }

    /// Returns `true` if the query matches `candidate` using
    /// camelCase/underscore abbreviation rules (no substring fallback).
    ///
    /// Rules (applied in order, first match wins):
    /// 1. `candidate` starts with the query (case-insensitive prefix match).
    /// 2. Every character of the query matches a camelCase word boundary or
    ///    character after `_` / `$` in the candidate.
    ///
    /// Examples:
    /// - `"GRF"` matches `"getRecentFiles"`
    /// - `"str_r"` matches `"str_replace"` (boundary after `_`)
    ///
    /// See [`Self::symbol_rank`] for the variant that also accepts
    /// substrings, which is appropriate for workspace symbol search but not
    /// for completions.
    pub(crate) fn camel_match(&self, candidate: &str) -> bool {
        if self.lower.is_empty() {
            return true;
        }
        // Rule 1: plain prefix
        if starts_with_at(candidate, &self.lower, 0) {
            return true;
        }
        // Rule 2: camel / underscore abbreviation
        self.camel_abbrev(candidate)
    }

    /// Like [`Self::camel_match`] but also accepts contiguous substrings as a
    /// fallback.  Use for workspace symbol search (where "Controller" should
    /// match "BlogController") but NOT for completions (substring produces too
    /// many hits). Ranked: 0 exact, 1 prefix, 2 abbreviation or substring.
    pub(crate) fn symbol_rank(&self, candidate: &str) -> Option<u8> {
        if starts_with_at(candidate, &self.lower, 0) {
            let exact = candidate.chars().flat_map(char::to_lowercase).count()
                == self.lower.chars().count();
            return Some(if exact { 0 } else { 1 });
        }
        self.symbol_fallback(candidate).then_some(2)
    }

    fn symbol_fallback(&self, candidate: &str) -> bool {
        if self.camel_abbrev(candidate) {
            return true;
        }
        // Substring fallback: "Controller" matches "BlogController".
        // char_indices keeps the scan allocation-free; candidates are short
        // identifiers, so the O(n·m) window walk beats two String allocations.
        candidate
            .char_indices()
            .any(|(i, _)| starts_with_at(candidate, &self.lower, i))
    }

    /// Core camel/underscore abbreviation check against the lowercased query.
    fn camel_abbrev(&self, candidate: &str) -> bool {
        let mut query = self.lower.chars().peekable();
        let mut prev: Option<char> = None;
        for cc in candidate.chars() {
            let Some(&qc) = query.peek() else {
                return true;
            };
            // A "word boundary" in the candidate is: position 0, after '_' or
            // '$', or an uppercase letter after a lowercase letter (camelCase
            // transition).
            let is_boundary = match prev {
                None => true,
                Some('_') | Some('$') => true,
                Some(p) => cc.is_uppercase() && p.is_lowercase(),
            };
            if is_boundary && cc.to_lowercase().next() == Some(qc) {
                query.next();
            }
            prev = Some(cc);
        }
        query.peek().is_none()
    }
}

/// Case-insensitive (Unicode-lowercase) test that `candidate[at..]` starts
/// with the already-lowercased `query_lower`, without allocating.
fn starts_with_at(candidate: &str, query_lower: &str, at: usize) -> bool {
    let mut c = candidate[at..].chars().flat_map(char::to_lowercase);
    let mut q = query_lower.chars();
    loop {
        match (q.next(), c.next()) {
            (None, _) => return true,
            (Some(qc), Some(cc)) if qc == cc => continue,
            _ => return false,
        }
    }
}

/// Compute a sort key so prefix matches sort before camel-abbreviation matches.
/// Lower string = higher priority.  Only called on items that passed
/// [`FuzzyQuery::camel_match`], so the `else` branch (substring) is unreachable here.
pub(crate) fn camel_sort_key(query: &str, label: &str) -> String {
    let lq = query.to_lowercase();
    let ll = label.to_lowercase();
    if ll.starts_with(&lq) {
        format!("0{}", ll)
    } else {
        format!("1{}", ll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camel(query: &str, candidate: &str) -> bool {
        FuzzyQuery::new(query).camel_match(candidate)
    }
    fn symbol(query: &str, candidate: &str) -> bool {
        FuzzyQuery::new(query).symbol_rank(candidate).is_some()
    }

    #[test]
    fn symbol_rank_orders_exact_prefix_then_fuzzy() {
        let q = FuzzyQuery::new("user");
        assert_eq!(q.symbol_rank("User"), Some(0));
        assert_eq!(q.symbol_rank("UserRepository"), Some(1));
        assert_eq!(q.symbol_rank("AppUser"), Some(2));
        assert_eq!(q.symbol_rank("Order"), None);
    }

    #[test]
    fn fuzzy_camel_match_prefix() {
        assert!(camel("Blog", "BlogController"));
        assert!(camel("blog", "BlogController"));
    }

    #[test]
    fn fuzzy_camel_match_abbreviation() {
        assert!(camel("BC", "BlogController"));
        assert!(camel("GRF", "getRecentFiles"));
        assert!(camel("str_r", "str_replace")); // boundary after '_'
    }

    #[test]
    fn fuzzy_camel_match_no_substring() {
        // camel_match (used for completions) must NOT match substrings
        assert!(!camel("Controller", "BlogController"));
        assert!(!camel("xyz", "BlogController"));
    }

    #[test]
    fn fuzzy_symbol_match_substring_fallback() {
        // symbol_rank (used for workspace symbols) DOES match substrings
        assert!(symbol("Controller", "BlogController"));
        assert!(symbol("controller", "BlogController"));
        assert!(symbol("controller", "UserController"));
        // prefix and camel still work
        assert!(symbol("Blog", "BlogController"));
        assert!(symbol("BC", "BlogController"));
        // no match
        assert!(!symbol("xyz", "BlogController"));
    }
}
