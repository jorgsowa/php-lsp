//! Laravel-project detection: gates every other module in `laravel/` behind a
//! cheap one-time check so non-Laravel workspaces pay no cost.

use std::path::Path;

/// True if `root` looks like the root of a Laravel application: either an
/// `artisan` CLI script (present in every Laravel skeleton) or a
/// `composer.json` that requires `laravel/framework`.
pub(super) fn is_laravel_project(root: &Path) -> bool {
    if root.join("artisan").is_file() {
        return true;
    }
    let Ok(text) = std::fs::read(root.join("composer.json")).map(crate::text::decode_source) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    json["require"]["laravel/framework"].is_string()
        || json["require-dev"]["laravel/framework"].is_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_via_artisan_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        assert!(is_laravel_project(tmp.path()));
    }

    #[test]
    fn detects_via_composer_require() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("composer.json"),
            r#"{"require": {"laravel/framework": "^11.0"}}"#,
        )
        .unwrap();
        assert!(is_laravel_project(tmp.path()));
    }

    #[test]
    fn detects_via_composer_require_dev() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("composer.json"),
            r#"{"require-dev": {"laravel/framework": "^11.0"}}"#,
        )
        .unwrap();
        assert!(is_laravel_project(tmp.path()));
    }

    #[test]
    fn non_laravel_project_not_detected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("composer.json"),
            r#"{"require": {"symfony/framework-bundle": "^7.0"}}"#,
        )
        .unwrap();
        assert!(!is_laravel_project(tmp.path()));
    }

    #[test]
    fn missing_composer_json_not_detected() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_laravel_project(tmp.path()));
    }

    #[test]
    fn malformed_composer_json_not_detected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("composer.json"), "{not valid json").unwrap();
        assert!(!is_laravel_project(tmp.path()));
    }
}
