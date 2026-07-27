//! Semantic token coverage: full, range, delta, and delta-fallback cases.

use super::*;
use expect_test::expect;

async fn get_legend_types(init_resp: &serde_json::Value) -> Vec<&str> {
    init_resp["result"]["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect()
}

#[tokio::test]
async fn semantic_tokens_full_returned() {
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    let out = server
        .check_semantic_tokens_full(
            "<?php\nfunction tokenized(int $x): int { return $x; }\n",
            &legend_types,
        )
        .await;

    expect![[r#"
        1:9 len=9 type=function mods=0b1
        1:19 len=3 type=type mods=0b0
        1:23 len=2 type=parameter mods=0b1
        1:28 len=3 type=type mods=0b0
        1:41 len=2 type=variable mods=0b0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn semantic_tokens_range_returns_data() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "st_range.php",
            "<?php\nfunction ranged(int $x): int { return $x; }\n",
        )
        .await;

    // Request semanticTokens/range from the already-open file (not via check_semantic_tokens_range
    // which would try to reopen the "st_range.php" string as PHP source code).
    let resp = server
        .semantic_tokens_range("st_range.php", 0, 0, 2, 0)
        .await;

    let out = render_semantic_tokens(&resp, &legend_types);
    // Range request from line 0-2 includes all tokens (whole file, the function on line 1)
    expect![[r#"
        1:9 len=6 type=function mods=0b1
        1:16 len=3 type=type mods=0b0
        1:20 len=2 type=parameter mods=0b1
        1:25 len=3 type=type mods=0b0
        1:38 len=2 type=variable mods=0b0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn semantic_tokens_full_delta_returns_result() {
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({})).await;
    let legend_types = get_legend_types(&init_resp).await;
    server
        .open(
            "st_delta.php",
            "<?php\nfunction delta(int $x): int { return $x; }\n",
        )
        .await;

    let full = server.semantic_tokens_full("st_delta.php").await;
    let result_id = full["result"]["resultId"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        !result_id.is_empty(),
        "semanticTokens/full must return a resultId to support delta requests"
    );

    let resp = server
        .semantic_tokens_full_delta("st_delta.php", &result_id)
        .await;

    expect!["<no edits>"].assert_eq(&render_semantic_tokens_delta(&resp, &legend_types));
}

/// Delta request with an unknown `previousResultId` must degrade gracefully
/// to a full-token response — the server must never error out or panic when
/// the client's baseline is stale / unknown (e.g. after a server restart).
#[tokio::test]
async fn semantic_tokens_delta_with_stale_previous_result_id_degrades_to_full() {
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({})).await;
    let legend_types = get_legend_types(&init_resp).await;
    server
        .open(
            "st_stale.php",
            "<?php\nfunction stale(int $x): int { return $x; }\n",
        )
        .await;

    let resp = server
        .semantic_tokens_full_delta("st_stale.php", "definitely-not-a-real-id")
        .await;

    expect![[r#"
        full:
        1:9 len=5 type=function mods=0b1
        1:15 len=3 type=type mods=0b0
        1:19 len=2 type=parameter mods=0b1
        1:24 len=3 type=type mods=0b0
        1:37 len=2 type=variable mods=0b0"#]]
    .assert_eq(&render_semantic_tokens_delta(&resp, &legend_types));
}

#[tokio::test]
async fn semantic_tokens_delta_without_baseline_degrades_to_full() {
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({})).await;
    let legend_types = get_legend_types(&init_resp).await;
    server
        .open(
            "st_noprior.php",
            "<?php\nfunction nobaseline(): int { return 1; }\n",
        )
        .await;

    let resp = server
        .semantic_tokens_full_delta("st_noprior.php", "0")
        .await;

    expect![[r#"
        full:
        1:9 len=10 type=function mods=0b1
        1:23 len=3 type=type mods=0b0
        1:36 len=1 type=number mods=0b0"#]]
    .assert_eq(&render_semantic_tokens_delta(&resp, &legend_types));
}

/// After `didChange`, requesting delta with the pre-edit resultId must reflect
/// the new content. Either an `edits` diff or a full `data` set is acceptable,
/// but the post-edit token count must exceed the pre-edit count since we added
/// an entire function.
#[tokio::test]
async fn semantic_tokens_delta_after_didchange_reflects_new_content() {
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({})).await;
    let legend_types = get_legend_types(&init_resp).await;
    server
        .open("st_edit.php", "<?php\nfunction one(): int { return 1; }\n")
        .await;

    let full = server.semantic_tokens_full("st_edit.php").await;
    let pre_id = full["result"]["resultId"]
        .as_str()
        .expect("resultId")
        .to_string();

    server
        .change(
            "st_edit.php",
            2,
            "<?php\nfunction one(): int { return 1; }\nfunction two(): int { return 2; }\n",
        )
        .await;

    let resp = server
        .semantic_tokens_full_delta("st_edit.php", &pre_id)
        .await;

    expect![[r#"
        edit start=15 deleteCount=0
        1:9 len=3 type=function mods=0b1
        1:16 len=3 type=type mods=0b0
        1:29 len=1 type=number mods=0b0"#]]
    .assert_eq(&render_semantic_tokens_delta(&resp, &legend_types));
}

/// Verify that semantic tokens can be decoded and contain specific token types.
/// This test decodes raw token integers and snapshots the full token stream,
/// ensuring that function declarations and parameters are properly tokenized.
#[tokio::test]
async fn semantic_tokens_decode_function_tokens() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "decode.php",
            "<?php\nfunction greet(string $name): void { echo $name; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("decode.php").await;
    assert!(resp["error"].is_null(), "error: {resp:?}");

    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:9 len=5 type=function mods=0b1
        1:15 len=6 type=type mods=0b0
        1:22 len=5 type=parameter mods=0b1
        1:30 len=4 type=type mods=0b0
        1:42 len=5 type=variable mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that `semanticTokens/range` request respects range boundaries.
/// LSP range is [start_line:start_char, end_line:end_char). This test requests
/// from line 1 char 0 to line 2 char 0 (exclusive end), which captures line 1 only.
/// In a two-function file, only the first function's tokens are returned.
#[tokio::test]
async fn semantic_tokens_range_bounds_respected() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    let src = "<?php\nfunction one(): int { return 1; }\nfunction two(): int { return 2; }\n";
    server.open("range.php", src).await;

    // Request semanticTokens/range for line 1 only (range is [start, end) exclusive end).
    let resp = server.semantic_tokens_range("range.php", 1, 0, 2, 0).await;
    assert!(resp["error"].is_null(), "error: {resp:?}");

    let out = render_semantic_tokens(&resp, &legend_types);
    // Only line 1 tokens returned; line 2 (second function) is excluded
    expect![[r#"
        1:9 len=3 type=function mods=0b1
        1:16 len=3 type=type mods=0b0
        1:29 len=1 type=number mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that deprecated functions get the `deprecated` modifier.
#[tokio::test]
async fn semantic_tokens_deprecated_function_modifier() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "deprecated.php",
            "<?php\n/** @deprecated */ function old(): void {}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("deprecated.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // deprecated modifier is bit 4 (value 16 = 0b10000)
    expect![[r#"
        1:0 len=18 type=comment mods=0b0
        1:28 len=3 type=function mods=0b10001
        1:35 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that classes, interfaces, and enums are properly tokenized.
/// Classes and enums use type=class, while interfaces use type=interface.
#[tokio::test]
async fn semantic_tokens_class_interface_enum() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "types.php",
            "<?php\nclass C {}\ninterface I {}\nenum E {}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("types.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Classes and enums use type=class; interfaces use type=interface
    // All have declaration modifier (0b1)
    expect![[r#"
        1:6 len=1 type=class mods=0b1
        2:10 len=1 type=interface mods=0b1
        3:5 len=1 type=class mods=0b1"#]]
    .assert_eq(&out);
}

/// Verify that static methods are marked with the `static` modifier.
/// Static methods have declaration (bit 0) + static (bit 1) = 0b11 = 3
#[tokio::test]
async fn semantic_tokens_static_method_modifier() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "static.php",
            "<?php\nclass C { static function m(): void {} }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("static.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Modifiers: declaration=bit 0, static=bit 1, so static method = 0b11
    expect![[r#"
        1:6 len=1 type=class mods=0b1
        1:26 len=1 type=method mods=0b11
        1:31 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that empty files return empty token data (not an error).
#[tokio::test]
async fn semantic_tokens_empty_file() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server.open("empty.php", "<?php\n").await;

    let resp = server.semantic_tokens_full("empty.php").await;
    assert!(resp["error"].is_null(), "error: {resp:?}");

    let out = render_semantic_tokens(&resp, &legend_types);
    expect!["<no tokens>"].assert_eq(&out);
}

/// Verify that parse errors don't break semantic token reporting.
#[tokio::test]
async fn semantic_tokens_with_parse_error() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open("broken.php", "<?php\nfunction broken(;\n")
        .await;

    let resp = server.semantic_tokens_full("broken.php").await;
    assert!(resp["error"].is_null(), "error: {resp:?}");
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        0:0 len=7 type=parameter mods=0b1
        1:9 len=6 type=function mods=0b1"#]]
    .assert_eq(&out);
}

/// Verify that class properties are tokenized as property type.
#[tokio::test]
async fn semantic_tokens_class_properties() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "props.php",
            "<?php\nclass C { public string $name; private int $age; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("props.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Properties should be tokenized with type=property
    expect![[r#"
        1:6 len=1 type=class mods=0b1
        1:17 len=6 type=type mods=0b0
        1:24 len=5 type=property mods=0b1
        1:39 len=3 type=type mods=0b0
        1:43 len=4 type=property mods=0b1"#]]
    .assert_eq(&out);
}

/// A plain class's own `const` members must be tokenized — `collect_class_member`
/// previously handled `Method`/`Property` but silently dropped `ClassConst`,
/// even though the identical enum-const case (`EnumMemberKind::ClassConst`)
/// was already handled.
#[tokio::test]
async fn semantic_tokens_class_constant() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "consts.php",
            "<?php\nclass Status { const int PENDING = 0; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("consts.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:6 len=6 type=class mods=0b1
        1:21 len=3 type=type mods=0b0
        1:25 len=7 type=property mods=0b1
        1:35 len=1 type=number mods=0b0"#]]
    .assert_eq(&out);
}

/// Property access, static method call, static property access, and class
/// constant access expressions must all be tokenized (and must not swallow
/// the tokens of the sub-expressions they contain) — these previously fell
/// through to the catch-all `_ => {}` arm and produced zero tokens.
#[tokio::test]
async fn semantic_tokens_member_and_static_access_expressions() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "access.php",
            "<?php\nclass Foo {\n    function bar() {\n        $x = $this->prop;\n        $y = Foo::method();\n        $z = Foo::CONST;\n    }\n}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("access.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:6 len=3 type=class mods=0b1
        2:13 len=3 type=method mods=0b1
        3:8 len=2 type=variable mods=0b0
        3:13 len=5 type=variable mods=0b0
        3:20 len=4 type=property mods=0b0
        4:8 len=2 type=variable mods=0b0
        4:13 len=3 type=class mods=0b0
        4:18 len=6 type=method mods=0b10
        5:8 len=2 type=variable mods=0b0
        5:13 len=3 type=class mods=0b0
        5:18 len=5 type=property mods=0b10"#]]
    .assert_eq(&out);
}

/// Verify that enum declarations and cases are tokenized properly.
/// Enum cases are tokenized as `type=property` (similar to class properties).
#[tokio::test]
async fn semantic_tokens_enum_cases() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "enums.php",
            "<?php\nenum Status { case Pending; case Active; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("enums.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Enum declaration (Status) and cases (Pending, Active) are tokenized.
    // Cases use type=property (declaration modifier 0b1).
    expect![[r#"
        1:5 len=6 type=class mods=0b1
        1:19 len=7 type=enumMember mods=0b1
        1:33 len=6 type=enumMember mods=0b1"#]]
    .assert_eq(&out);
}

/// Verify that backed enums (with string values) tokenize both cases and values.
#[tokio::test]
async fn semantic_tokens_backed_enum_string() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "backed.php",
            "<?php\nenum Status: string { case Pending = 'pending'; case Active = 'active'; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("backed.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Backed enum: cases and their string values should both be tokenized
    expect![[r#"
        1:5 len=6 type=class mods=0b1
        1:27 len=7 type=enumMember mods=0b1
        1:37 len=9 type=string mods=0b0
        1:53 len=6 type=enumMember mods=0b1
        1:62 len=8 type=string mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that backed enums with int values tokenize both cases and numbers.
#[tokio::test]
async fn semantic_tokens_backed_enum_int() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "backed_int.php",
            "<?php\nenum Count: int { case One = 1; case Two = 2; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("backed_int.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Backed enum with int values: cases and numbers should be tokenized
    expect![[r#"
        1:5 len=5 type=class mods=0b1
        1:23 len=3 type=enumMember mods=0b1
        1:29 len=1 type=number mods=0b0
        1:37 len=3 type=enumMember mods=0b1
        1:43 len=1 type=number mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that enum cases with attributes are tokenized correctly.
#[tokio::test]
async fn semantic_tokens_enum_case_attributes() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "enum_attr.php",
            "<?php\nenum Status { #[Deprecated] case Old; case New; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("enum_attr.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Attributes should be tokenized as class tokens
    expect![[r#"
        1:5 len=6 type=class mods=0b1
        1:16 len=10 type=class mods=0b0
        1:33 len=3 type=enumMember mods=0b1
        1:43 len=3 type=enumMember mods=0b1"#]]
    .assert_eq(&out);
}

/// Verify that mixed enums (cases + methods) are tokenized correctly.
#[tokio::test]
async fn semantic_tokens_enum_mixed() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "enum_mixed.php",
            "<?php\nenum Status { case Pending; public function label(): string { return 'x'; } }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("enum_mixed.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Mixed enum: both cases and methods should be tokenized
    expect![[r#"
        1:5 len=6 type=class mods=0b1
        1:19 len=7 type=enumMember mods=0b1
        1:44 len=5 type=method mods=0b1
        1:53 len=6 type=type mods=0b0
        1:69 len=3 type=string mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that deprecated enum cases (via @deprecated PHPDoc) get the deprecated modifier.
#[tokio::test]
async fn semantic_tokens_deprecated_enum_case() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "deprecated_case.php",
            "<?php\nenum Status { /** @deprecated */ case Old; case New; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("deprecated_case.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Deprecated case should have both declaration (0b1) and deprecated (0b10000) = 0b10001
    expect![[r#"
        1:5 len=6 type=class mods=0b1
        1:14 len=18 type=comment mods=0b0
        1:38 len=3 type=enumMember mods=0b10001
        1:48 len=3 type=enumMember mods=0b1"#]]
    .assert_eq(&out);
}

/// Verify that traits are tokenized as class type.
#[tokio::test]
async fn semantic_tokens_traits() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "traits.php",
            "<?php\ntrait Logger { public function log(): void {} }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("traits.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Traits use type=class, methods use type=method
    expect![[r#"
        1:6 len=6 type=class mods=0b1
        1:31 len=3 type=method mods=0b1
        1:38 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that readonly properties are tokenized.
/// NOTE: This test verifies that readonly properties are recognized as properties,
/// but does NOT verify the exact modifier value since the server may not distinguish
/// readonly properties with a specific modifier bit.
#[tokio::test]
async fn semantic_tokens_readonly_property() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "readonly.php",
            "<?php\nclass C { readonly string $value; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("readonly.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Verify readonly property is tokenized as a property
    // The exact modifier value may vary depending on how the server handles readonly
    expect![[r#"
        1:6 len=1 type=class mods=0b1
        1:19 len=6 type=type mods=0b0
        1:26 len=6 type=property mods=0b1001"#]]
    .assert_eq(&out);
}

/// Verify that abstract methods are tokenized with declaration modifier.
#[tokio::test]
async fn semantic_tokens_abstract_method() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "abstract.php",
            "<?php\nabstract class Base { abstract function process(): void; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("abstract.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Abstract class and method should have declaration modifier
    expect![[r#"
        1:15 len=4 type=class mods=0b101
        1:40 len=7 type=method mods=0b101
        1:51 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that union types are tokenized correctly.
/// Union type syntax (int|string, int|null) should tokenize both types.
#[tokio::test]
async fn semantic_tokens_union_types() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "union.php",
            "<?php\nfunction process(int|string $value): int|null {}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("union.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Union types should tokenize each type in the union
    // The | operator may or may not be tokenized separately
    expect![[r#"
        1:9 len=7 type=function mods=0b1
        1:17 len=3 type=type mods=0b0
        1:21 len=6 type=type mods=0b0
        1:28 len=6 type=parameter mods=0b1
        1:37 len=3 type=type mods=0b0
        1:41 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that zero-width range requests work correctly.
#[tokio::test]
async fn semantic_tokens_zero_width_range() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open("zero.php", "<?php\nfunction test(): void {}\n")
        .await;

    // Request a zero-width range (start == end)
    let resp = server.semantic_tokens_range("zero.php", 1, 9, 1, 9).await;

    let out = render_semantic_tokens(&resp, &legend_types);
    // Zero-width range should return no tokens (or possibly empty)
    expect!["<no tokens>"].assert_eq(&out);
}

/// Verify that final methods are marked with declaration modifier.
#[tokio::test]
async fn semantic_tokens_final_method() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "final.php",
            "<?php\nclass C { final function lock(): void {} }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("final.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Final method should have declaration modifier (bit 0)
    expect![[r#"
        1:6 len=1 type=class mods=0b1
        1:25 len=4 type=method mods=0b1
        1:33 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify enum class constants with type hints are tokenized.
#[tokio::test]
async fn semantic_tokens_enum_class_constant_with_type() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "enum_const.php",
            "<?php\nenum Status { const int PENDING = 0; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("enum_const.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Enum name, type hint, property (constant name), and number value
    expect![[r#"
        1:5 len=6 type=class mods=0b1
        1:20 len=3 type=type mods=0b0
        1:24 len=7 type=property mods=0b1
        1:34 len=1 type=number mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify enum class constants with string values are tokenized.
#[tokio::test]
async fn semantic_tokens_enum_class_constant_string() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "enum_string.php",
            "<?php\nenum Config { const string URL = \"https://api\"; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("enum_string.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Enum name, type hint (string), property (constant name), and string value
    expect![[r#"
        1:5 len=6 type=class mods=0b1
        1:20 len=6 type=type mods=0b0
        1:27 len=3 type=property mods=0b1
        1:33 len=13 type=string mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify mixed enum members (cases, constants, methods) are all tokenized.
#[tokio::test]
async fn semantic_tokens_enum_mixed_members() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "enum_mixed.php",
            "<?php\nenum Response {\n    case Success;\n    const int ERROR_CODE = 500;\n    const string MESSAGE = \"error\";\n    public function status(): void {}\n}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("enum_mixed.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    // Verify all enum members: name, case, constants (with types), method
    expect![[r#"
        1:5 len=8 type=class mods=0b1
        2:9 len=7 type=enumMember mods=0b1
        3:10 len=3 type=type mods=0b0
        3:14 len=10 type=property mods=0b1
        3:27 len=3 type=number mods=0b0
        4:10 len=6 type=type mods=0b0
        4:17 len=7 type=property mods=0b1
        4:27 len=7 type=string mods=0b0
        5:20 len=6 type=method mods=0b1
        5:30 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify switch statements tokenize test expr, case values, and body variables.
#[tokio::test]
async fn semantic_tokens_switch_statement() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "switch.php",
            "<?php\n$status = 1;\nswitch ($status) {\n    case 0:\n        echo \"off\";\n        break;\n    default:\n        echo \"on\";\n}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("switch.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=7 type=variable mods=0b0
        1:10 len=1 type=number mods=0b0
        2:8 len=7 type=variable mods=0b0
        3:9 len=1 type=number mods=0b0
        4:13 len=5 type=string mods=0b0
        7:13 len=4 type=string mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify match expressions tokenize subject, arm conditions, and body variables.
#[tokio::test]
async fn semantic_tokens_match_expression() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "match.php",
            "<?php\n$code = 200;\n$result = match ($code) {\n    200, 201 => \"ok\",\n    default => \"error\"\n};\n",
        )
        .await;

    let resp = server.semantic_tokens_full("match.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=5 type=variable mods=0b0
        1:8 len=3 type=number mods=0b0
        2:0 len=7 type=variable mods=0b0
        2:17 len=5 type=variable mods=0b0
        3:4 len=3 type=number mods=0b0
        3:9 len=3 type=number mods=0b0
        3:16 len=4 type=string mods=0b0
        4:15 len=7 type=string mods=0b0"#]]
    .assert_eq(&out);
}

/// foreach key/value variables are tokenized as `TT_VARIABLE`.
#[tokio::test]
async fn semantic_tokens_foreach_key_value_variables() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "foreach.php",
            "<?php\n$items = [1, 2, 3];\nforeach ($items as $k => $v) {\n    echo $k . $v;\n}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("foreach.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=6 type=variable mods=0b0
        1:10 len=1 type=number mods=0b0
        1:13 len=1 type=number mods=0b0
        1:16 len=1 type=number mods=0b0
        2:9 len=6 type=variable mods=0b0
        2:19 len=2 type=variable mods=0b0
        2:25 len=2 type=variable mods=0b0
        3:9 len=2 type=variable mods=0b0
        3:14 len=2 type=variable mods=0b0"#]]
    .assert_eq(&out);
}

/// Edge case: foreach with only value variable (no key).
#[tokio::test]
async fn semantic_tokens_foreach_value_only() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "foreach_val.php",
            "<?php\n$data = ['a', 'b'];\nforeach ($data as $item) {\n    echo $item;\n}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("foreach_val.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=5 type=variable mods=0b0
        1:9 len=3 type=string mods=0b0
        1:14 len=3 type=string mods=0b0
        2:9 len=5 type=variable mods=0b0
        2:18 len=5 type=variable mods=0b0
        3:9 len=5 type=variable mods=0b0"#]]
    .assert_eq(&out);
}

/// Edge case: nested foreach with key/value variables.
#[tokio::test]
async fn semantic_tokens_nested_foreach() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "nested_foreach.php",
            "<?php\n$matrix = [[1, 2], [3, 4]];\nforeach ($matrix as $row) {\n    foreach ($row as $k => $v) {\n        echo $k . $v;\n    }\n}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("nested_foreach.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=7 type=variable mods=0b0
        1:12 len=1 type=number mods=0b0
        1:15 len=1 type=number mods=0b0
        1:20 len=1 type=number mods=0b0
        1:23 len=1 type=number mods=0b0
        2:9 len=7 type=variable mods=0b0
        2:20 len=4 type=variable mods=0b0
        3:13 len=4 type=variable mods=0b0
        3:21 len=2 type=variable mods=0b0
        3:27 len=2 type=variable mods=0b0
        4:13 len=2 type=variable mods=0b0
        4:18 len=2 type=variable mods=0b0"#]]
    .assert_eq(&out);
}

/// Edge case: foreach with reference binding should tokenize the variable.
#[tokio::test]
async fn semantic_tokens_foreach_with_reference() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "foreach_ref.php",
            "<?php\n$items = [1, 2, 3];\nforeach ($items as &$item) {\n    $item *= 2;\n}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("foreach_ref.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=6 type=variable mods=0b0
        1:10 len=1 type=number mods=0b0
        1:13 len=1 type=number mods=0b0
        1:16 len=1 type=number mods=0b0
        2:9 len=6 type=variable mods=0b0
        2:20 len=5 type=variable mods=0b0
        3:4 len=5 type=variable mods=0b0
        3:13 len=1 type=number mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that function call sites emit function tokens without the declaration modifier.
#[tokio::test]
async fn semantic_tokens_function_call() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "func_call.php",
            "<?php\nfunction greet(): void {}\ngreet();\n",
        )
        .await;

    let resp = server.semantic_tokens_full("func_call.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:9 len=5 type=function mods=0b1
        1:18 len=4 type=type mods=0b0
        2:0 len=5 type=function mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that method call sites emit method tokens without the declaration modifier.
#[tokio::test]
async fn semantic_tokens_method_call() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open("method_call.php", "<?php\n$obj->run();\n")
        .await;

    let resp = server.semantic_tokens_full("method_call.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=4 type=variable mods=0b0
        1:6 len=3 type=method mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that functions inside a namespace are tokenized.
#[tokio::test]
async fn semantic_tokens_namespace_contents() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "ns.php",
            "<?php\nnamespace App;\nfunction boot(): void {}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("ns.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        2:9 len=4 type=function mods=0b1
        2:17 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that a deprecated method gets the deprecated modifier.
#[tokio::test]
async fn semantic_tokens_deprecated_method() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "dep_method.php",
            "<?php\nclass Foo {\n    /** @deprecated */\n    public function old(): void {}\n}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("dep_method.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:6 len=3 type=class mods=0b1
        2:4 len=18 type=comment mods=0b0
        3:20 len=3 type=method mods=0b10001
        3:27 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that attribute names on functions are tokenized as class tokens.
#[tokio::test]
async fn semantic_tokens_attribute_on_function() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "attr_fn.php",
            "<?php\n#[Route(\"/home\")]\nfunction index(): void {}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("attr_fn.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:2 len=5 type=class mods=0b0
        2:9 len=5 type=function mods=0b1
        2:18 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that float literals are tokenized as number tokens.
#[tokio::test]
async fn semantic_tokens_float_literal() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server.open("float.php", "<?php\n$x = 3.14;\n").await;

    let resp = server.semantic_tokens_full("float.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=2 type=variable mods=0b0
        1:5 len=4 type=number mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that single-line `//` comments are tokenized as comment tokens.
#[tokio::test]
async fn semantic_tokens_single_line_comment() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open("slcomment.php", "<?php\n// this is a comment\n$x = 1;\n")
        .await;

    let resp = server.semantic_tokens_full("slcomment.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=20 type=comment mods=0b0
        2:0 len=2 type=variable mods=0b0
        2:5 len=1 type=number mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that multi-line `/* ... */` block comments emit per-line comment tokens.
#[tokio::test]
async fn semantic_tokens_multiline_comment() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open("mlcomment.php", "<?php\n/* block\n   comment */\n$x = 1;\n")
        .await;

    let resp = server.semantic_tokens_full("mlcomment.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=8 type=comment mods=0b0
        2:0 len=13 type=comment mods=0b0
        3:0 len=2 type=variable mods=0b0
        3:5 len=1 type=number mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that for-loop init, condition, and update expressions are all tokenized.
#[tokio::test]
async fn semantic_tokens_for_loop() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open("forloop.php", "<?php\nfor ($i = 0; $i < 10; $i++) {}\n")
        .await;

    let resp = server.semantic_tokens_full("forloop.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:5 len=2 type=variable mods=0b0
        1:10 len=1 type=number mods=0b0
        1:13 len=2 type=variable mods=0b0
        1:18 len=2 type=number mods=0b0
        1:22 len=2 type=variable mods=0b0"#]]
    .assert_eq(&out);
}

/// Verify that method return types are tokenized as type tokens.
#[tokio::test]
async fn semantic_tokens_method_return_type() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "method_rt.php",
            "<?php\nclass Foo { public function get(): string { return ''; } }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("method_rt.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:6 len=3 type=class mods=0b1
        1:28 len=3 type=method mods=0b1
        1:35 len=6 type=type mods=0b0
        1:51 len=2 type=string mods=0b0"#]]
    .assert_eq(&out);
}

/// Test delta encoding with line insertion: adding a new function creates delta edits.
#[tokio::test]
async fn semantic_tokens_delta_with_line_insertion() {
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({})).await;
    let legend_types = get_legend_types(&init_resp).await;
    server
        .open(
            "st_insert.php",
            "<?php\nfunction first(): int { return 1; }\n",
        )
        .await;

    let full = server.semantic_tokens_full("st_insert.php").await;
    let pre_id = full["result"]["resultId"]
        .as_str()
        .expect("resultId")
        .to_string();

    // Insert a new function before the existing one
    server
        .change(
            "st_insert.php",
            2,
            "<?php\nfunction zero(): void {}\nfunction first(): int { return 1; }\n",
        )
        .await;

    let resp = server
        .semantic_tokens_full_delta("st_insert.php", &pre_id)
        .await;

    expect![[r#"
        edit start=0 deleteCount=0
        1:9 len=4 type=function mods=0b1
        1:17 len=4 type=type mods=0b0"#]]
    .assert_eq(&render_semantic_tokens_delta(&resp, &legend_types));
}

/// Test delta encoding with line deletion: removing a function creates delta edits.
#[tokio::test]
async fn semantic_tokens_delta_with_line_deletion() {
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({})).await;
    let legend_types = get_legend_types(&init_resp).await;
    server
        .open(
            "st_delete.php",
            "<?php\nfunction first(): int { return 1; }\nfunction second(): int { return 2; }\n",
        )
        .await;

    let full = server.semantic_tokens_full("st_delete.php").await;
    let pre_id = full["result"]["resultId"]
        .as_str()
        .expect("resultId")
        .to_string();

    // Remove the second function
    server
        .change(
            "st_delete.php",
            2,
            "<?php\nfunction first(): int { return 1; }\n",
        )
        .await;

    let resp = server
        .semantic_tokens_full_delta("st_delete.php", &pre_id)
        .await;

    expect![[r#"
        edit start=15 deleteCount=15
        <no tokens>"#]]
    .assert_eq(&render_semantic_tokens_delta(&resp, &legend_types));
}

/// Test delta encoding with token modification: changing return type updates delta.
#[tokio::test]
async fn semantic_tokens_delta_with_token_modification() {
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({})).await;
    let legend_types = get_legend_types(&init_resp).await;
    server
        .open(
            "st_modify.php",
            "<?php\nfunction getValue(): int { return 42; }\n",
        )
        .await;

    let full = server.semantic_tokens_full("st_modify.php").await;
    let pre_id = full["result"]["resultId"]
        .as_str()
        .expect("resultId")
        .to_string();

    // Change return type from int to string
    server
        .change(
            "st_modify.php",
            2,
            "<?php\nfunction getValue(): string { return '42'; }\n",
        )
        .await;

    let resp = server
        .semantic_tokens_full_delta("st_modify.php", &pre_id)
        .await;

    expect![[r#"
        edit start=5 deleteCount=10
        0:12 len=6 type=type mods=0b0
        0:28 len=4 type=string mods=0b0"#]]
    .assert_eq(&render_semantic_tokens_delta(&resp, &legend_types));
}

/// Test incremental delta application: multiple sequential edits maintain correctness.
#[tokio::test]
async fn semantic_tokens_delta_incremental_accumulation() {
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({})).await;
    let legend_types = get_legend_types(&init_resp).await;
    server.open("st_incr.php", "<?php\nfunction a() {}\n").await;

    let full1 = server.semantic_tokens_full("st_incr.php").await;
    let id1 = full1["result"]["resultId"]
        .as_str()
        .expect("resultId")
        .to_string();

    // First edit: add a second function
    server
        .change(
            "st_incr.php",
            2,
            "<?php\nfunction a() {}\nfunction b() {}\n",
        )
        .await;
    let resp1 = server.semantic_tokens_full_delta("st_incr.php", &id1).await;
    expect![[r#"
        edit start=5 deleteCount=0
        1:9 len=1 type=function mods=0b1"#]]
    .assert_eq(&render_semantic_tokens_delta(&resp1, &legend_types));

    let full2 = server.semantic_tokens_full("st_incr.php").await;
    let id2 = full2["result"]["resultId"]
        .as_str()
        .expect("resultId")
        .to_string();

    // Second edit: add a third function
    server
        .change(
            "st_incr.php",
            2,
            "<?php\nfunction a() {}\nfunction b() {}\nfunction c() {}\n",
        )
        .await;
    let resp2 = server.semantic_tokens_full_delta("st_incr.php", &id2).await;
    expect![[r#"
        edit start=10 deleteCount=0
        1:9 len=1 type=function mods=0b1"#]]
    .assert_eq(&render_semantic_tokens_delta(&resp2, &legend_types));

    // Both deltas apply on top of a consistent final full snapshot.
    let final_full = server.semantic_tokens_full("st_incr.php").await;
    expect![[r#"
        1:9 len=1 type=function mods=0b1
        2:9 len=1 type=function mods=0b1
        3:9 len=1 type=function mods=0b1"#]]
    .assert_eq(&render_semantic_tokens(&final_full, &legend_types));
}

/// Test delta degradation on large file changes: ensure graceful handling of extensive edits.
#[tokio::test]
async fn semantic_tokens_delta_large_file_changes() {
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({})).await;
    let legend_types = get_legend_types(&init_resp).await;

    // Start with a moderately sized file
    let initial = "<?php\n".to_string()
        + &(0..10)
            .map(|i| format!("function fn{i}() {{}}\n", i = i))
            .collect::<String>();

    server.open("st_large.php", &initial).await;

    let full = server.semantic_tokens_full("st_large.php").await;
    let pre_id = full["result"]["resultId"]
        .as_str()
        .expect("resultId")
        .to_string();

    // Replace all content with a different set of functions
    let modified = "<?php\n".to_string()
        + &(10..20)
            .map(|i| format!("function fn{i}() {{}}\n", i = i))
            .collect::<String>();

    server.change("st_large.php", 11, &modified).await;

    let resp = server
        .semantic_tokens_full_delta("st_large.php", &pre_id)
        .await;

    expect![[r#"
        edit start=0 deleteCount=50
        1:9 len=4 type=function mods=0b1
        2:9 len=4 type=function mods=0b1
        3:9 len=4 type=function mods=0b1
        4:9 len=4 type=function mods=0b1
        5:9 len=4 type=function mods=0b1
        6:9 len=4 type=function mods=0b1
        7:9 len=4 type=function mods=0b1
        8:9 len=4 type=function mods=0b1
        9:9 len=4 type=function mods=0b1
        10:9 len=4 type=function mods=0b1"#]]
    .assert_eq(&render_semantic_tokens_delta(&resp, &legend_types));
}

/// `é` (U+00E9) is 2 UTF-8 bytes but 1 UTF-16 code unit.
/// A named type hint `Héros` has byte-length 6 but UTF-16 length 5.
/// The token's `length` field must reflect UTF-16 units.
#[tokio::test]
async fn semantic_tokens_named_type_hint_multibyte_utf16_length() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open("mb_type.php", "<?php\nfunction greet(Héros $h): void {}\n")
        .await;

    let resp = server.semantic_tokens_full("mb_type.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:9 len=5 type=function mods=0b1
        1:15 len=5 type=type mods=0b0
        1:21 len=2 type=parameter mods=0b1
        1:26 len=4 type=type mods=0b0"#]]
    .assert_eq(&out);
}

/// Attribute names with multibyte characters must use UTF-16 lengths.
/// `Routé`: R(1) o(1) u(1) t(1) é(2 bytes, 1 UTF-16) → len=5, not 6.
#[tokio::test]
async fn semantic_tokens_attribute_name_multibyte_utf16_length() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server
        .open(
            "mb_attr.php",
            "<?php\n#[Routé(\"/home\")]\nfunction index() {}\n",
        )
        .await;

    let resp = server.semantic_tokens_full("mb_attr.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:2 len=5 type=class mods=0b0
        2:9 len=5 type=function mods=0b1"#]]
    .assert_eq(&out);
}

/// String literals with multibyte characters must report UTF-16 length.
/// `"café"`: " + c + a + f + é(1 UTF-16) + " = 6 UTF-16 units, not 7 bytes.
#[tokio::test]
async fn semantic_tokens_string_literal_multibyte_utf16_length() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server.open("mb_str.php", "<?php\n$s = \"café\";\n").await;

    let resp = server.semantic_tokens_full("mb_str.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=2 type=variable mods=0b0
        1:5 len=6 type=string mods=0b0"#]]
    .assert_eq(&out);
}

/// Variable names with multibyte characters must report UTF-16 length.
/// `$café`: $ + c + a + f + é(1 UTF-16) = 5 UTF-16 units, not 6 bytes.
#[tokio::test]
async fn semantic_tokens_variable_multibyte_utf16_length() {
    use common::render_semantic_tokens;
    use serde_json::json;

    let (mut server, init_resp) = TestServer::new_with_options(json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let legend_types = get_legend_types(&init_resp).await;

    server.open("mb_var.php", "<?php\n$café = 1;\n").await;

    let resp = server.semantic_tokens_full("mb_var.php").await;
    let out = render_semantic_tokens(&resp, &legend_types);
    expect![[r#"
        1:0 len=5 type=variable mods=0b0
        1:8 len=1 type=number mods=0b0"#]]
    .assert_eq(&out);
}
