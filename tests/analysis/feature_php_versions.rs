use super::*;
use serde_json::json;

// ── PHP 8.0 functions (str_contains, str_starts_with, str_ends_with) ────────────

// PHP version-aware @since/@removed filtering: AnalysisSession::new() seeds the
// configured PHP version into the salsa db, so stdlib functions are reported as
// undefined below the version that introduced them. Fixed in mir-analyzer 0.31.0
// ("wire php_version into salsa db").
#[tokio::test]
async fn str_contains_undefined_on_php74() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "7.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open("test.php", "<?php\nstr_contains(\"hello\", \"ell\");\n")
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("str_contains")),
        "Expected undefined str_contains error on PHP 7.4"
    );
}

#[tokio::test]
async fn str_contains_defined_on_php80() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.0",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics("<?php\nstr_contains(\"hello\", \"ell\");\n")
        .await;
}

#[tokio::test]
async fn str_starts_with_undefined_on_php74() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "7.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open("test.php", "<?php\nstr_starts_with(\"hello\", \"hel\");\n")
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags.iter().any(|d| d["message"]
            .as_str()
            .unwrap_or("")
            .contains("str_starts_with")),
        "Expected undefined str_starts_with error on PHP 7.4"
    );
}

#[tokio::test]
async fn str_ends_with_undefined_on_php74() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "7.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open("test.php", "<?php\nstr_ends_with(\"hello\", \"lo\");\n")
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags.iter().any(|d| d["message"]
            .as_str()
            .unwrap_or("")
            .contains("str_ends_with")),
        "Expected undefined str_ends_with error on PHP 7.4"
    );
}

// ── PHP 8.0 removed functions (each, money_format) ────────────────────────────

#[tokio::test]
async fn each_defined_on_php74() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "7.4",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics("<?php\n$arr = [1, 2, 3];\n$pair = each($arr);\n")
        .await;
}

#[tokio::test]
async fn each_undefined_on_php80() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.0",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open(
            "test.php",
            "<?php\n$arr = [1, 2, 3];\n$pair = each($arr);\n",
        )
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("each")),
        "Expected undefined each error on PHP 8.0"
    );
}

#[tokio::test]
async fn money_format_defined_on_php74() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "7.4",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics("<?php\n$result = money_format(\"%.2n\", 1234.56);\n")
        .await;
}

#[tokio::test]
async fn money_format_undefined_on_php80() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.0",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open(
            "test.php",
            "<?php\n$result = money_format(\"%.2n\", 1234.56);\n",
        )
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("money_format")),
        "Expected undefined money_format error on PHP 8.0"
    );
}

// ── PHP 8.1 functions (array_is_list) ──────────────────────────────────────────

#[tokio::test]
async fn array_is_list_undefined_on_php74() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "7.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open("test.php", "<?php\n$is_list = array_is_list([1, 2, 3]);\n")
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags.iter().any(|d| d["message"]
            .as_str()
            .unwrap_or("")
            .contains("array_is_list")),
        "Expected undefined array_is_list error on PHP 7.4"
    );
}

#[tokio::test]
async fn array_is_list_undefined_on_php80() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.0",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open("test.php", "<?php\n$is_list = array_is_list([1, 2, 3]);\n")
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags.iter().any(|d| d["message"]
            .as_str()
            .unwrap_or("")
            .contains("array_is_list")),
        "Expected undefined array_is_list error on PHP 8.0"
    );
}

#[tokio::test]
async fn array_is_list_defined_on_php81() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.1",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics("<?php\n$is_list = array_is_list([1, 2, 3]);\n")
        .await;
}

// ── PHP 8.4 functions (array_find, array_any, array_all) ────────────────────────

#[tokio::test]
async fn array_find_undefined_on_php83() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.3",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open(
            "test.php",
            "<?php\n$found = array_find([1, 2, 3], fn ($n) => $n > 1);\n",
        )
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("array_find")),
        "Expected undefined array_find error on PHP 8.3"
    );
}

#[tokio::test]
async fn array_find_defined_on_php84() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.4",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics("<?php\n$found = array_find([1, 2, 3], fn ($n) => $n > 1);\n")
        .await;
}

#[tokio::test]
async fn array_any_undefined_on_php83() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.3",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open(
            "test.php",
            "<?php\n$any = array_any([1, 2, 3], fn ($n) => $n > 1);\n",
        )
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("array_any")),
        "Expected undefined array_any error on PHP 8.3"
    );
}

#[tokio::test]
async fn array_all_undefined_on_php83() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.3",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open(
            "test.php",
            "<?php\n$all = array_all([1, 2, 3], fn ($n) => $n > 0);\n",
        )
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("array_all")),
        "Expected undefined array_all error on PHP 8.3"
    );
}

// ── PHP 8.5 functions (array_first, array_last) ────────────────────────────────

#[tokio::test]
async fn array_first_undefined_on_php84() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open("test.php", "<?php\n$first = array_first([1, 2, 3]);\n")
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("array_first")),
        "Expected undefined array_first error on PHP 8.4"
    );
}

#[tokio::test]
async fn array_first_defined_on_php85() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.5",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics("<?php\n$first = array_first([1, 2, 3]);\n")
        .await;
}

#[tokio::test]
async fn array_last_undefined_on_php84() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let empty = vec![];
    let notif = s
        .open("test.php", "<?php\n$last = array_last([1, 2, 3]);\n")
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("array_last")),
        "Expected undefined array_last error on PHP 8.4"
    );
}

#[tokio::test]
async fn array_last_defined_on_php85() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.5",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics("<?php\n$last = array_last([1, 2, 3]);\n")
        .await;
}

// ── Version change re-triggers diagnostics ─────────────────────────────────────

#[tokio::test]
async fn version_change_clears_diagnostics() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "7.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    // Open file with PHP 7.4 - str_contains should error
    let empty = vec![];
    let notif1 = s
        .open("test.php", "<?php\n$x = str_contains('hello', 'ell');\n")
        .await;
    let diags1 = notif1["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags1
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("str_contains")),
        "Expected str_contains error with PHP 7.4"
    );

    // Change to PHP 8.0
    s.change_configuration(json!({"phpVersion": "8.0"})).await;

    // Open the same file again - should have no diagnostic now
    let notif2 = s
        .open("test.php", "<?php\n$x = str_contains('hello', 'ell');\n")
        .await;
    let diags2 = notif2["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        !diags2
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("str_contains")),
        "Expected no str_contains error with PHP 8.0"
    );
}

// ── Boundary tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn latest_version_has_all_85_functions() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.5",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics(
        "<?php\n$first = array_first([1]);\n$last = array_last([1]);\n$found = array_find([1], fn ($n) => true);\n$any = array_any([1], fn ($n) => true);\n$all = array_all([1], fn ($n) => true);\n",
    )
    .await;
}

#[tokio::test]
async fn all_versions_have_basic_stdlib() {
    for version in &["7.4", "8.0", "8.1", "8.2", "8.3", "8.4", "8.5"] {
        let (mut s, _) = TestServer::new_with_options(json!({
            "phpVersion": version,
            "diagnostics": { "enabled": true }
        }))
        .await;
        s.check_no_diagnostics(
            "<?php\nstrlen(\"test\");\narray_map(fn ($x) => $x, []);\nin_array(1, [1, 2, 3]);\n",
        )
        .await;
    }
}

// ── Edge case: invalid version falls back to latest ───────────────────────────

#[tokio::test]
async fn invalid_version_falls_back_to_latest_stubs() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "99.99",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics("<?php\n$first = array_first([1]);\n$last = array_last([1]);\n")
        .await;
}
