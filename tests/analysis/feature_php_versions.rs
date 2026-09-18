use super::*;
use expect_test::expect;
use serde_json::json;

// ── PHP 8.0 functions (str_contains, str_starts_with, str_ends_with) ────────────

// PHP version-aware @since/@removed filtering: stdlib functions are reported
// as undefined below the version that introduced them.
#[tokio::test]
async fn str_contains_undefined_on_php74() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "7.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let notif = s
        .open("test.php", "<?php\nstr_contains(\"hello\", \"ell\");\n")
        .await;
    expect!["1:0-1:28 [1] UndefinedFunction: Function str_contains() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
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

    let notif = s
        .open("test.php", "<?php\nstr_starts_with(\"hello\", \"hel\");\n")
        .await;
    expect!["1:0-1:31 [1] UndefinedFunction: Function str_starts_with() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn str_ends_with_undefined_on_php74() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "7.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let notif = s
        .open("test.php", "<?php\nstr_ends_with(\"hello\", \"lo\");\n")
        .await;
    expect!["1:0-1:28 [1] UndefinedFunction: Function str_ends_with() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
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

    let notif = s
        .open(
            "test.php",
            "<?php\n$arr = [1, 2, 3];\n$pair = each($arr);\n",
        )
        .await;
    expect!["2:8-2:18 [1] UndefinedFunction: Function each() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
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

    let notif = s
        .open(
            "test.php",
            "<?php\n$result = money_format(\"%.2n\", 1234.56);\n",
        )
        .await;
    expect!["1:10-1:39 [1] UndefinedFunction: Function money_format() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

// ── PHP 8.1 functions (array_is_list) ──────────────────────────────────────────

#[tokio::test]
async fn array_is_list_undefined_on_php74() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "7.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let notif = s
        .open("test.php", "<?php\n$is_list = array_is_list([1, 2, 3]);\n")
        .await;
    expect!["1:11-1:35 [1] UndefinedFunction: Function array_is_list() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn array_is_list_undefined_on_php80() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.0",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let notif = s
        .open("test.php", "<?php\n$is_list = array_is_list([1, 2, 3]);\n")
        .await;
    expect!["1:11-1:35 [1] UndefinedFunction: Function array_is_list() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
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

    let notif = s
        .open(
            "test.php",
            "<?php\n$found = array_find([1, 2, 3], fn ($n) => $n > 1);\n",
        )
        .await;
    expect!["1:9-1:49 [1] UndefinedFunction: Function array_find() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
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

    let notif = s
        .open(
            "test.php",
            "<?php\n$any = array_any([1, 2, 3], fn ($n) => $n > 1);\n",
        )
        .await;
    expect!["1:7-1:46 [1] UndefinedFunction: Function array_any() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn array_all_undefined_on_php83() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.3",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let notif = s
        .open(
            "test.php",
            "<?php\n$all = array_all([1, 2, 3], fn ($n) => $n > 0);\n",
        )
        .await;
    expect!["1:7-1:46 [1] UndefinedFunction: Function array_all() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn array_any_defined_on_php84() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.4",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics("<?php\n$any = array_any([1, 2, 3], fn ($n) => $n > 1);\n")
        .await;
}

#[tokio::test]
async fn array_all_defined_on_php84() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.4",
        "diagnostics": { "enabled": true }
    }))
    .await;
    s.check_no_diagnostics("<?php\n$all = array_all([1, 2, 3], fn ($n) => $n > 0);\n")
        .await;
}

// ── PHP 8.5 functions (array_first, array_last) ────────────────────────────────

#[tokio::test]
async fn array_first_undefined_on_php84() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "phpVersion": "8.4",
        "diagnostics": { "enabled": true }
    }))
    .await;

    let notif = s
        .open("test.php", "<?php\n$first = array_first([1, 2, 3]);\n")
        .await;
    expect!["1:9-1:31 [1] UndefinedFunction: Function array_first() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
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

    let notif = s
        .open("test.php", "<?php\n$last = array_last([1, 2, 3]);\n")
        .await;
    expect!["1:8-1:29 [1] UndefinedFunction: Function array_last() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
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

    // PHP 7.4: str_contains not yet defined
    let notif1 = s
        .open("test.php", "<?php\n$x = str_contains('hello', 'ell');\n")
        .await;
    expect!["1:5-1:33 [1] UndefinedFunction: Function str_contains() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif1));

    // Change to PHP 8.0 — str_contains is now defined
    s.change_configuration(json!({"phpVersion": "8.0"})).await;

    // Re-open the same file; now should have no diagnostics
    let notif2 = s
        .open("test.php", "<?php\n$x = str_contains('hello', 'ell');\n")
        .await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&notif2));
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
