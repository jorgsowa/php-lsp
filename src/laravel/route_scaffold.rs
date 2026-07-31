//! Naming-convention heuristics for the "Create route" quickfix
//! (`src/actions/route_scaffold_action.rs`).
//!
//! `RouteIndex` only ever sees route *names*, never HTTP verbs, URIs, or
//! controller targets (see its module doc), so scaffolding a controller
//! method for an unresolved `route('...')` call has nothing to go on but the
//! name itself. This applies Laravel's common `{resource}.{action}` naming
//! convention (`posts.show` → `PostsController::show`) — best-effort, not
//! guaranteed to match a project's actual controller naming (no
//! singularization is attempted: `posts.show` yields `PostsController`, not
//! `PostController`).

/// Splits a route name on its last `.` into a studly-cased resource and an
/// action, e.g. `"admin.posts.show"` → `("AdminPosts", "show")`. `None` for
/// names with no `.` (nothing to convention-derive a controller from) or an
/// empty segment on either side.
pub(crate) fn resource_and_action(route_name: &str) -> Option<(String, String)> {
    let (resource_part, action) = route_name.rsplit_once('.')?;
    if resource_part.is_empty() || action.is_empty() {
        return None;
    }
    let resource = resource_part.split('.').map(studly).collect::<String>();
    Some((resource, action.to_string()))
}

fn studly(segment: &str) -> String {
    let mut chars = segment.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_simple_resource_and_action() {
        assert_eq!(
            resource_and_action("posts.show"),
            Some(("Posts".to_string(), "show".to_string()))
        );
    }

    #[test]
    fn studly_cases_multi_segment_resource() {
        assert_eq!(
            resource_and_action("admin.posts.show"),
            Some(("AdminPosts".to_string(), "show".to_string()))
        );
    }

    #[test]
    fn none_for_name_without_a_dot() {
        assert_eq!(resource_and_action("home"), None);
    }

    #[test]
    fn none_for_trailing_dot() {
        assert_eq!(resource_and_action("posts."), None);
    }
}
