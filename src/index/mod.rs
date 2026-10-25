//! Workspace indexing: the compact per-file `FileIndex`, the background
//! workspace scan that builds it for every file, and the on-disk index cache.

pub mod file_index;

pub mod cache;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod workspace_scan;

// Path patterns are globs over `/`-separated segments: `*` and `?` match within a segment,
// `**` matches any number of segments. A pattern matches a path, or anything beneath it.

/// `None` for patterns that would otherwise match everything by accident (`""`, `"/"`).
#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn pattern_segments(pat: &str) -> Option<Vec<&str>> {
    let mut p = pat.trim_start_matches("./");
    while let Some(rest) = p.strip_suffix("/**").or_else(|| p.strip_suffix('/')) {
        p = rest;
    }
    let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    (!segs.is_empty()).then_some(segs)
}

#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn segment_matches(pat: &str, name: &str) -> bool {
    if !pat.contains(['*', '?']) {
        return pat == name;
    }
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            backtrack = Some((pi, ni));
            pi += 1;
        } else if let Some((star_pi, star_ni)) = backtrack {
            pi = star_pi + 1;
            ni = star_ni + 1;
            backtrack = Some((star_pi, star_ni + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn segments_match(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| segments_match(rest, &path[i..])),
        Some((seg, rest)) => path
            .split_first()
            .is_some_and(|(name, tail)| segment_matches(seg, name) && segments_match(rest, tail)),
    }
}

/// Anchored at the start of `path`; matches `path` or any of its ancestors.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn matches_from_start(pat: &[&str], path: &[&str]) -> bool {
    (0..=path.len()).any(|end| segments_match(pat, &path[..end]))
}

#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn matches_at_any_depth(pat: &[&str], path: &[&str]) -> bool {
    (0..path.len()).any(|start| matches_from_start(pat, &path[start..]))
}

/// Matches `pat` at any depth; a `.php` file also matches by its stem (`Greeter` → `Greeter.php`).
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn matches_any(path: &str, patterns: &[String]) -> bool {
    let segs = path_segments(path);
    let stem_segs = segs
        .last()
        .and_then(|l| l.strip_suffix(".php"))
        .map(|stem| {
            let mut s = segs.clone();
            *s.last_mut().unwrap() = stem;
            s
        });
    patterns
        .iter()
        .filter_map(|p| pattern_segments(p))
        .any(|pat| {
            matches_at_any_depth(&pat, &segs)
                || stem_segs
                    .as_ref()
                    .is_some_and(|s| matches_at_any_depth(&pat, s))
        })
}

#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn matches_include_prefix(rel_path: &str, patterns: &[String]) -> bool {
    let segs = path_segments(rel_path);
    patterns
        .iter()
        .filter_map(|p| pattern_segments(p))
        .any(|pat| matches_from_start(&pat, &segs))
}

/// Whether some path beneath directory `rel_path` could match one of `patterns`.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn has_included_children(rel_path: &str, patterns: &[String]) -> bool {
    fn could_extend_to_match(pat: &[&str], path: &[&str]) -> bool {
        match (pat.split_first(), path.split_first()) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some((&"**", _)), Some(_)) => true,
            (Some((seg, rest)), Some((name, tail))) => {
                segment_matches(seg, name) && could_extend_to_match(rest, tail)
            }
        }
    }
    let segs = path_segments(rel_path);
    patterns
        .iter()
        .filter_map(|p| pattern_segments(p))
        .any(|pat| could_extend_to_match(&pat, &segs))
}

#[cfg(test)]
mod tests {
    use super::{has_included_children, matches_any, matches_include_prefix, segment_matches};

    fn pats(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn segment_wildcards() {
        assert!(segment_matches("*Test", "FooTest"));
        assert!(segment_matches("Foo*", "Foo"));
        assert!(segment_matches("F?o*r", "Fooobar"));
        assert!(!segment_matches("F?o", "Fo"));
        assert!(!segment_matches("*Test", "TestFoo"));
    }

    #[test]
    fn plain_names_match_any_segment_but_not_substrings() {
        let p = pats(&["src/"]);
        assert!(matches_any("src/A.php", &p));
        assert!(matches_any("pkg/src/A.php", &p));
        assert!(!matches_any("tests/source/A.php", &p));
        assert!(!matches_any("test_src/A.php", &p));
    }

    #[test]
    fn double_star_at_either_end() {
        let p = pats(&["**/var/**", "**/node_modules/**"]);
        assert!(matches_any("var", &p));
        assert!(matches_any("var/cache/A.php", &p));
        assert!(matches_any("src/var/A.php", &p));
        assert!(matches_any("web/node_modules/x/B.php", &p));
        assert!(!matches_any("src/variables/A.php", &p));
    }

    #[test]
    fn double_star_in_the_middle() {
        let p = pats(&["src/**/Generated"]);
        assert!(matches_any("src/Generated/A.php", &p));
        assert!(matches_any("src/a/b/Generated/A.php", &p));
        assert!(matches_any("pkg/src/a/Generated", &p));
        assert!(!matches_any("src/a/Generator/A.php", &p));
        assert!(!matches_any("lib/a/Generated/A.php", &p));
    }

    #[test]
    fn trailing_star_matches_children_not_the_dir() {
        let p = pats(&["src/Service/*"]);
        assert!(!matches_any("src/Service", &p));
        assert!(matches_any("src/Service/Greeter.php", &p));
        assert!(matches_any("src/Service/Sub/X.php", &p));
    }

    #[test]
    fn wildcard_file_patterns_and_php_stem() {
        assert!(matches_any("tests/FooTest.php", &pats(&["*Test.php"])));
        assert!(matches_any("tests/FooTest.php", &pats(&["**/*Test"])));
        assert!(matches_any("src/Service/Greeter.php", &pats(&["Greeter"])));
        assert!(!matches_any("src/Service/Greeter.php", &pats(&["Greet"])));
    }

    #[test]
    fn degenerate_patterns_match_nothing() {
        let p = pats(&["", "/", "./"]);
        assert!(!matches_any("src/A.php", &p));
        assert!(!has_included_children("src", &p));
    }

    #[test]
    fn absolute_paths() {
        assert!(matches_any(
            "/home/u/proj/vendor/x/A.php",
            &pats(&["**/vendor/**"])
        ));
    }

    #[test]
    fn include_prefix_is_anchored() {
        let p = pats(&["vendor/yiisoft", "lib/*/src"]);
        assert!(matches_include_prefix("vendor/yiisoft", &p));
        assert!(matches_include_prefix("vendor/yiisoft/A.php", &p));
        assert!(matches_include_prefix("lib/pkg/src/A.php", &p));
        assert!(!matches_include_prefix("x/vendor/yiisoft", &p));
        assert!(!matches_include_prefix("lib/pkg/tests/A.php", &p));
    }

    #[test]
    fn included_children_lookahead() {
        let p = pats(&["vendor/yiisoft", "lib/*/src", "**/keep"]);
        assert!(has_included_children("vendor", &p));
        assert!(!has_included_children("other", &pats(&["vendor/yiisoft"])));
        assert!(has_included_children("lib/pkg", &p));
        assert!(!has_included_children(
            "lib/pkg/tests",
            &pats(&["lib/*/src"])
        ));
        assert!(has_included_children("anything/deep", &pats(&["**/keep"])));
    }
}
