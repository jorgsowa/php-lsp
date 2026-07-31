//! Code action smoke coverage. Each scenario uses a two-`$0` selection to
//! name the range the action acts on.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn code_actions_offers_generate_constructor() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class U$0ser$0 {
    public string $name = '';
    public int $age = 0;
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate 2 getters/setters
        refactor         Generate constructor"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_actions_offers_extract_variable_on_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
function f(): int {
    return $01 + 2$0;
}
"#,
        )
        .await;
    expect!["refactor.extract Extract variable [edit]"].assert_eq(&out);
}

#[tokio::test]
async fn code_actions_offers_add_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
function $0noReturn$0() { return 42; }
"#,
        )
        .await;
    expect![[r#"
        refactor         Add return type `: mixed`
        refactor         Generate PHPDoc"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_actions_offers_implement_missing_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
interface Writable { public function write(): void; }
class $0My$0 implements Writable {}
"#,
        )
        .await;
    expect!["quickfix         Implement missing method"].assert_eq(&out);
}

// --- codeAction/resolve roundtrip tests ---
// Each test verifies that a deferred action (no `edit` on initial request)
// gets its full WorkspaceEdit populated after calling codeAction/resolve.

#[tokio::test]
async fn code_action_resolve_return_type() {
    let mut server = TestServer::new().await;
    server
        .open("rt.php", "<?php\nfunction noReturn() { return 42; }\n")
        .await;

    let resp = server.code_action("rt.php", 1, 0, 1, 30).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"].as_str() == Some("Add return type `: mixed`"))
        .cloned()
        .expect("return type action");

    assert!(
        action["edit"].is_null(),
        "action must be deferred (no edit)"
    );
    assert!(!action["data"].is_null(), "action must carry resolve data");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null(), "resolve must not error");
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // rt.php
        1:19-1:19 → ": mixed""#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_action_resolve_phpdoc() {
    let mut server = TestServer::new().await;
    server
        .open("doc.php", "<?php\nfunction greet(string $name): void {}\n")
        .await;

    let resp = server.code_action("doc.php", 1, 0, 1, 40).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"].as_str() == Some("Generate PHPDoc"))
        .cloned()
        .expect("phpdoc action");

    assert!(action["edit"].is_null(), "action must be deferred");
    assert!(!action["data"].is_null(), "action must carry resolve data");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // doc.php
        1:0-1:0 → "/**\n * @param string $name\n * @return void\n */\n""#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_action_resolve_constructor() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ctor.php",
            "<?php\nclass Point {\n    public float $x = 0.0;\n    public float $y = 0.0;\n}\n",
        )
        .await;

    let resp = server.code_action("ctor.php", 1, 0, 1, 11).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"].as_str() == Some("Generate constructor"))
        .cloned()
        .expect("constructor action");

    assert!(action["edit"].is_null(), "action must be deferred");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // ctor.php
        4:0-4:0 → "    public function __construct(\n        float $x,\n        float $y,\n    ) {\n        $this->x = $x;\n        $this->y = $y;\n    }\n\n""#]].assert_eq(&out);
}

#[tokio::test]
async fn code_action_resolve_getters_setters() {
    let mut server = TestServer::new().await;
    server
        .open(
            "gs.php",
            "<?php\nclass Box {\n    public int $width = 0;\n    public int $height = 0;\n}\n",
        )
        .await;

    let resp = server.code_action("gs.php", 1, 0, 1, 9).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.contains("getters/setters"))
                .unwrap_or(false)
        })
        .cloned()
        .expect("getters/setters action");

    assert!(action["edit"].is_null(), "action must be deferred");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // gs.php
        4:0-4:0 → "    public function getWidth(): int\n    {\n        return $this->width;\n    }\n\n    public function setWidth(int $width): void\n    {\n        $this->width = $width;\n    }\n\n    public function getHeight(): int\n    {\n        return $this->height;\n    }\n\n    public function setHeight(int $height): void\n    {\n        $this->height = $height;\n    }\n\n""#]].assert_eq(&out);
}

#[tokio::test]
async fn code_action_resolve_implement_missing_methods() {
    let mut server = TestServer::new().await;
    server
        .open(
            "impl.php",
            "<?php\ninterface Loggable { public function log(): void; }\nclass App implements Loggable {}\n",
        )
        .await;

    let resp = server.code_action("impl.php", 2, 0, 2, 30).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"].as_str() == Some("Implement missing method"))
        .cloned()
        .expect("implement missing method action");

    assert!(action["edit"].is_null(), "action must be deferred");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // impl.php
        2:31-2:31 → "\n    public function log(): void\n    {\n        throw new \\RuntimeException('Not implemented');\n    }\n\n""#]].assert_eq(&out);
}

#[tokio::test]
async fn code_action_resolve_promote_constructor_params() {
    let mut server = TestServer::new().await;
    server
        .open(
            "promote.php",
            "<?php\nclass Service {\n    private string $name;\n    private int $port;\n    public function __construct(string $name, int $port) {\n        $this->name = $name;\n        $this->port = $port;\n    }\n}\n",
        )
        .await;

    let resp = server.code_action("promote.php", 2, 0, 7, 6).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.to_lowercase().contains("promot"))
                .unwrap_or(false)
        })
        .cloned()
        .expect("promote action must be offered for non-promoted constructor params");

    assert!(action["edit"].is_null(), "action must be deferred");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // promote.php
        2:0-3:0 → ""
        3:0-4:0 → ""
        4:32-4:32 → "private "
        4:46-4:46 → "private "
        5:0-6:0 → ""
        6:0-7:0 → """#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_action_resolve_promote_simple_private_property() {
    let mut server = TestServer::new().await;
    server
        .open(
            "promote.php",
            "<?php\nclass Foo {\n    private string $name$0;\n    public function __construct(string $name) {\n        $this->name = $name;\n    }\n}\n",
        )
        .await;

    let resp = server.code_action("promote.php", 2, 0, 5, 6).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.to_lowercase().contains("promot"))
                .unwrap_or(false)
        })
        .cloned()
        .expect("promote action should be offered for promotable properties");

    let title = action["title"].as_str().expect("action must have title");
    assert_eq!(
        title, "Promote constructor parameter",
        "single property should use singular form"
    );

    assert!(action["edit"].is_null(), "action must be deferred");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // promote.php
        2:0-3:0 → ""
        3:32-3:32 → "private "
        4:0-5:0 → """#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_action_resolve_promote_readonly_property() {
    let mut server = TestServer::new().await;
    server
        .open(
            "promote.php",
            "<?php\nclass Bar {\n    private readonly string $id$0;\n    public function __construct(string $id) {\n        $this->id = $id;\n    }\n}\n",
        )
        .await;

    let resp = server.code_action("promote.php", 2, 0, 5, 6).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.to_lowercase().contains("promot"))
                .unwrap_or(false)
        })
        .cloned()
        .expect("promote action should be offered for readonly properties");

    let title = action["title"].as_str().expect("action must have title");
    assert_eq!(
        title, "Promote constructor parameter",
        "readonly property should also use singular form"
    );

    assert!(action["edit"].is_null(), "action must be deferred");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // promote.php
        2:0-3:0 → ""
        3:32-3:32 → "private readonly "
        4:0-5:0 → """#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_action_resolve_promote_multiple_properties() {
    let mut server = TestServer::new().await;
    server
        .open(
            "promote.php",
            "<?php\nclass Baz {\n    private string $name$0;\n    protected int $age;\n    public function __construct(string $name, int $age) {\n        $this->name = $name;\n        $this->age = $age;\n    }\n}\n",
        )
        .await;

    let resp = server.code_action("promote.php", 2, 0, 7, 6).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.to_lowercase().contains("promot"))
                .unwrap_or(false)
        })
        .cloned()
        .expect("promote action should be offered for multiple properties");

    let title = action["title"].as_str().expect("action must have title");
    assert_eq!(
        title, "Promote 2 constructor parameters",
        "multiple properties should use plural form with count"
    );

    assert!(action["edit"].is_null(), "action must be deferred");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // promote.php
        2:0-3:0 → ""
        3:0-4:0 → ""
        4:32-4:32 → "private "
        4:46-4:46 → "protected "
        5:0-6:0 → ""
        6:0-7:0 → """#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_action_resolve_without_data_is_passthrough() {
    let mut server = TestServer::new().await;
    server.open("noop.php", "<?php").await;

    let action = serde_json::json!({
        "title": "My Action",
        "kind": "refactor"
    });
    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    assert_eq!(
        resolved["result"]["title"].as_str(),
        Some("My Action"),
        "title must roundtrip"
    );
    assert!(
        resolved["result"]["edit"].is_null(),
        "no edit should be added for data-less actions"
    );
}

// ── Negative tests for promote_action: verify action is NOT offered ────

#[tokio::test]
async fn promote_action_not_offered_without_constructor() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Foo {
    private string $name$0;
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate constructor
        refactor         Generate getter/setter"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn promote_action_not_offered_for_static_properties() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Foo {
    private static string $name$0;
    public function __construct() {}
}
"#,
        )
        .await;
    expect!["<no actions>"].assert_eq(&out);
}

#[tokio::test]
async fn promote_action_not_offered_for_mismatched_names() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Foo {
    private string $title$0;
    public function __construct(string $name) {
        $this->title = $name;
    }
}
"#,
        )
        .await;
    expect!["refactor         Generate getter/setter"].assert_eq(&out);
}

#[tokio::test]
async fn promote_action_not_offered_for_complex_assignments() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Foo {
    private string $name$0;
    public function __construct(string $name) {
        $this->name = strtolower($name);
    }
}
"#,
        )
        .await;
    expect!["refactor         Generate getter/setter"].assert_eq(&out);
}

#[tokio::test]
async fn promote_action_not_offered_without_visibility_modifier() {
    let mut server = TestServer::new().await;
    server.validate_syntax(false);
    let out = server
        .check_code_actions(
            r#"<?php
class Foo {
    string $name$0;
    public function __construct(string $name) {
        $this->name = $name;
    }
}
"#,
        )
        .await;
    expect!["refactor         Generate getter/setter"].assert_eq(&out);
}

/// Properties immediately before constructor (no blank line) should still be promotable.
#[tokio::test]
async fn promote_action_with_no_blank_line_before_constructor() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Foo {
    public string $name$0;
    public function __construct(string $name) {
        $this->name = $name;
    }
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate getter/setter
        refactor         Promote constructor parameter"#]]
    .assert_eq(&out);
}

/// Promote action with multiple properties works when using range selection.
#[tokio::test]
async fn promote_action_on_multiple_properties() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
$0class User {
    public string $firstName;
    public string $lastName;
    public function __construct(string $firstName, string $lastName) {
        $this->firstName = $firstName;
        $this->lastName = $lastName;
    }
}$0
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate 2 getters/setters
        refactor         Generate PHPDoc
        refactor         Promote 2 constructor parameters"#]]
    .assert_eq(&out);
}

/// Constructor parameters with default values should be promotable.
#[tokio::test]
async fn promote_action_with_constructor_default_value() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Config {
    public string $envir$0onment;
    public function __construct(string $environment = 'dev') {
        $this->environment = $environment;
    }
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate getter/setter
        refactor         Make private [edit]
        refactor         Make protected [edit]
        refactor         Promote constructor parameter"#]]
    .assert_eq(&out);
}

/// A property's own default value (`= 3`) must carry over onto the promoted
/// parameter, not be silently dropped — the property declaration line is
/// deleted entirely, so its default only survives if explicitly copied onto
/// the parameter.
#[tokio::test]
async fn promote_action_carries_over_property_default_value() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_action_apply(
            r#"<?php
class Config {
    private int $retries$0 = 3;
    public function __construct(int $retries) {
        $this->retries = $retries;
    }
}
"#,
            "Promote constructor parameter",
        )
        .await;
    expect![[r#"
        <?php
        class Config {
            public function __construct(private int $retries = 3) {
            }
        }
    "#]]
    .assert_eq(&out);
}

/// When the constructor parameter already declares its own default, that
/// default wins — the property's default (if different) is not appended,
/// matching the existing type-hint precedence (param's own annotation wins).
#[tokio::test]
async fn promote_action_param_own_default_wins_over_property_default() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_action_apply(
            r#"<?php
class Config {
    private int $retries$0 = 3;
    public function __construct(int $retries = 5) {
        $this->retries = $retries;
    }
}
"#,
            "Promote constructor parameter",
        )
        .await;
    expect![[r#"
        <?php
        class Config {
            public function __construct(private int $retries = 5) {
            }
        }
    "#]]
    .assert_eq(&out);
}

/// Properties without trailing newline before constructor should work.
/// Regression: whole_line_range logic handles files without trailing newlines.
#[tokio::test]
async fn promote_action_resolve_no_trailing_newline() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Foo {
    public string $name$0;
    public function __construct(string $name) {
        $this->name = $name;
    }
}"#,
        )
        .await;
    expect![[r#"
        refactor         Generate getter/setter
        refactor         Promote constructor parameter"#]]
    .assert_eq(&out);
}

/// Readonly properties with complex types (nullable, mixed, etc.) should be promotable.
#[tokio::test]
async fn promote_action_readonly_with_nullable_type() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Config {
    public readonly ?string $va$0lue;
    public function __construct(?string $value = null) {
        $this->value = $value;
    }
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate getter
        refactor         Make private [edit]
        refactor         Make protected [edit]
        refactor         Promote constructor parameter"#]]
    .assert_eq(&out);
}

// --- Documented Limitations ---

/// **LIMITATION**: Unbraced namespace context is not tracked by promote_action.
/// In unbraced namespace declarations (namespace Foo;), classes defined at the
/// top level still work. However, this test documents the behavior.
#[tokio::test]
async fn promote_action_limitation_unbraced_namespace_context() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
namespace App;

$0class Foo {
    public string $name;
    public function __construct(string $name) {
        $this->name = $name;
    }
}$0
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate PHPDoc
        refactor         Generate getter/setter
        refactor         Promote constructor parameter"#]]
    .assert_eq(&out);
}

/// **LIMITATION**: Static properties are explicitly not promoted because
/// they cannot be promoted to constructor parameters (constructor parameters
/// can only handle instance properties).
#[tokio::test]
async fn promote_action_limitation_static_properties_not_promotable() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Config {
    public static string $database$0;
    public function __construct(string $database) {
        self::$database = $database;
    }
}
"#,
        )
        .await;
    expect!["<no actions>"].assert_eq(&out);
}

/// **LIMITATION**: Complex assignments are not promotable.
/// Only simple assignments like `$this->prop = $param;` are supported.
/// Other patterns like conditional assignment, function call assignment, etc. are skipped.
#[tokio::test]
async fn promote_action_limitation_complex_assignments() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Foo {
    public int $value$0;
    public function __construct(int $value) {
        if ($value > 0) {
            $this->value = $value;
        }
    }
}
"#,
        )
        .await;
    expect!["refactor         Generate getter/setter"].assert_eq(&out);
}

/// **LIMITATION**: Assignment to wrong variable name prevents promotion.
/// If parameter is `$name` but assignment is `$this->name = $different;`,
/// the property won't be promoted because names don't match.
#[tokio::test]
async fn promote_action_limitation_mismatched_parameter_name() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class User {
    public string $email$0;
    public function __construct(string $address) {
        $this->email = $address;
    }
}
"#,
        )
        .await;
    expect!["refactor         Generate getter/setter"].assert_eq(&out);
}

/// **LIMITATION**: Multiple assignment targets are not promoted.
/// Promotion requires a simple one-to-one mapping between property and parameter.
#[tokio::test]
async fn promote_action_limitation_multiple_assignments() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Logger {
    public string $file$0;
    public string $level;
    public function __construct(string $file, string $level) {
        $this->file = $file;
        $this->level = $level;
    }
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate 2 getters/setters
        refactor         Promote 2 constructor parameters"#]]
    .assert_eq(&out);
}

// ── property type hint propagation ───────────────────────────────────────────

/// Promoting a typed property preserves the type hint on the generated parameter.
#[tokio::test]
async fn promote_action_with_property_type_hint() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class User {
    private string $name$0;
    public function __construct(string $name) {
        $this->name = $name;
    }
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate getter/setter
        refactor         Promote constructor parameter"#]]
    .assert_eq(&out);
}

/// Edge case: property with nullable type hint should promote.
#[tokio::test]
async fn promote_action_with_nullable_type_hint() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Config {
    private ?string $value$0;
    public function __construct(?string $value) {
        $this->value = $value;
    }
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate getter/setter
        refactor         Promote constructor parameter"#]]
    .assert_eq(&out);
}

/// Edge case: property with union type hint should promote.
#[tokio::test]
async fn promote_action_with_union_type_hint() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Parser {
    private int|string $data$0;
    public function __construct(int|string $data) {
        $this->data = $data;
    }
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate getter/setter
        refactor         Promote constructor parameter"#]]
    .assert_eq(&out);
}

/// Edge case: readonly property should promote.
#[tokio::test]
async fn promote_action_with_readonly_property() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Config {
    private readonly string $key$0;
    public function __construct(string $key) {
        $this->key = $key;
    }
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate getter
        refactor         Promote constructor parameter"#]]
    .assert_eq(&out);
}

/// Edge case: property with mixed type should promote.
#[tokio::test]
async fn promote_action_with_mixed_type() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_actions(
            r#"<?php
class Flexible {
    private mixed $value$0;
    public function __construct(mixed $value) {
        $this->value = $value;
    }
}
"#,
        )
        .await;
    expect![[r#"
        refactor         Generate getter/setter
        refactor         Promote constructor parameter"#]]
    .assert_eq(&out);
}

/// Implement missing methods when the interface is defined in a separate file
/// (background-indexed, not open). The action must resolve across the whole
/// workspace, not just open files.
#[tokio::test]
async fn code_action_resolve_implement_cross_file_interface() {
    let mut server = TestServer::new().await;
    server
        .open(
            "Printable.php",
            "<?php\ninterface Printable { public function print(): void; public function getLabel(): string; }\n",
        )
        .await;
    server
        .open(
            "Report.php",
            "<?php\nclass Report implements Printable {}\n",
        )
        .await;

    let resp = server.code_action("Report.php", 1, 0, 1, 37).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.starts_with("Implement"))
                .unwrap_or(false)
        })
        .cloned()
        .expect("implement missing methods action should be offered for cross-file interface");

    assert!(action["edit"].is_null(), "action must be deferred");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // Report.php
        1:35-1:35 → "\n    public function print(): void\n    {\n        throw new \\RuntimeException('Not implemented');\n    }\n\n    public function getLabel(): string\n    {\n        throw new \\RuntimeException('Not implemented');\n    }\n\n""#]]
    .assert_eq(&out);
}

/// `codeAction/resolve`'s "implement" tag re-scans every tracked workspace
/// file's cached text (an Aho-Corasick search over `docs_for_scan_mentioning`)
/// on the async task unless it's deferred to `spawn_blocking`, unlike its
/// sibling `textDocument/codeAction`, which already defers the same work.
/// A large workspace makes that scan non-trivial, so this pins the handler
/// staying off the request loop the same way the single-document handlers'
/// responsiveness tests do (see `feature_server.rs`).
#[tokio::test]
async fn code_action_resolve_implement_stays_responsive_on_large_workspace() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(
        workspace.path().join("Printable.php"),
        "<?php\ninterface Printable { public function print(): void; }\n",
    )
    .unwrap();
    // The Aho-Corasick pass scans every tracked file's cached text
    // regardless of whether it matches, so plenty of sizeable unrelated
    // files make the scan itself measurably slow.
    for i in 0..300 {
        std::fs::write(
            workspace.path().join(format!("Noise{i}.php")),
            crate::common::fixture::large_php_source(5),
        )
        .unwrap();
    }

    let mut server = TestServer::with_root(workspace.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "Report.php",
            "<?php\nclass Report implements Printable {}\n",
        )
        .await;

    let resp = server.code_action("Report.php", 1, 0, 1, 37).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.starts_with("Implement"))
                .unwrap_or(false)
        })
        .cloned()
        .expect("implement missing methods action should be offered");

    server
        .assert_stays_responsive("codeAction/resolve", action)
        .await;
}

/// Implement missing methods when the interface is in a namespaced file and
/// the class uses a `use` import. Namespace resolution must be FQN-aware.
#[tokio::test]
async fn code_action_resolve_implement_namespaced_cross_file_interface() {
    let mut server = TestServer::new().await;
    server
        .open(
            "Contracts/Serializable.php",
            "<?php\nnamespace App\\Contracts;\ninterface Serializable { public function serialize(): string; }\n",
        )
        .await;
    server
        .open(
            "Models/User.php",
            "<?php\nnamespace App\\Models;\nuse App\\Contracts\\Serializable;\nclass User implements Serializable {}\n",
        )
        .await;

    let resp = server.code_action("Models/User.php", 3, 0, 3, 34).await;
    let action = resp["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.starts_with("Implement"))
                .unwrap_or(false)
        })
        .cloned()
        .expect("implement missing methods should work with namespaced cross-file interface");

    assert!(action["edit"].is_null(), "action must be deferred");

    let resolved = server.code_action_resolve(action).await;
    assert!(resolved["error"].is_null());
    let out = canonicalize_workspace_edit(&resolved["result"]["edit"], &server.uri(""));
    expect![[r#"
        // Models/User.php
        3:36-3:36 → "\n    public function serialize(): string\n    {\n        throw new \\RuntimeException('Not implemented');\n    }\n\n""#]]
    .assert_eq(&out);
}

// --- Quick-fix: UndefinedFunction → use function FQN; ---

#[tokio::test]
async fn code_action_quickfix_undefined_function_cross_file() {
    let mut server = TestServer::new().await;
    server
        .open(
            "helpers.php",
            "<?php\nnamespace App\\Helpers;\nfunction tap(mixed $value): mixed { return $value; }\n",
        )
        .await;
    // open() waits for publishDiagnostics — analysis is complete before code_action
    server
        .open("main.php", "<?php\nnamespace App;\ntap($x);\n")
        .await;

    let resp = server.code_action("main.php", 2, 0, 2, 10).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let action = actions
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.starts_with("Add use function"))
                .unwrap_or(false)
        })
        .cloned();

    assert!(
        action.is_some(),
        "expected 'Add use function' quick-fix, got: {resp:#}"
    );
    let a = action.unwrap();
    let out = canonicalize_workspace_edit(&a["edit"], &server.uri(""));
    expect![[r#"
        // main.php
        2:0-2:0 → "use function App\\Helpers\\tap;\n""#]]
    .assert_eq(&out);
}

// --- Quick-fix: UndefinedClass → use FQN; ---

/// REGRESSION: the "Add use" quick-fix must find classes anywhere in the
/// workspace index, not just among currently-open editor buffers. Widget.php
/// is written to disk and indexed via the workspace scan but never opened —
/// this reproduces the common case of importing a class whose file isn't
/// already open in the editor.
#[tokio::test]
async fn code_action_quickfix_undefined_class_not_open_in_editor() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    server.write_file(
        "src/Service/Widget.php",
        "<?php\nnamespace App\\Service;\n\nclass Widget {}\n",
    );
    let uri = server.uri("src/Service/Widget.php");
    server.did_change_watched_files(vec![(uri, 1)]).await;
    server
        .wait_until_symbol_present("Widget", std::time::Duration::from_secs(3))
        .await;

    server
        .open("src/main.php", "<?php\nnamespace App;\nnew Widget();\n")
        .await;

    let resp = server.code_action("src/main.php", 2, 4, 2, 10).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let action = actions
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.starts_with("Add use"))
                .unwrap_or(false)
        })
        .cloned();

    assert!(
        action.is_some(),
        "expected 'Add use' quick-fix for a class indexed but not open in the editor, got: {resp:#}"
    );
    let a = action.unwrap();
    let out = canonicalize_workspace_edit(&a["edit"], &server.uri(""));
    expect![[r#"
        // src/main.php
        2:0-2:0 → "use App\\Service\\Widget;\n""#]]
    .assert_eq(&out);
}

/// REGRESSION: mir reports `UndefinedClass` with the namespace-resolved
/// attempt (e.g. `App\Widget` for a bare `Widget` reference inside
/// `namespace App;`), not the bare token the developer wrote. The quick-fix
/// must strip that prefix before looking the short name up in the workspace
/// index, or it silently never fires for any namespaced consumer file.
#[tokio::test]
async fn code_action_quickfix_undefined_class_in_namespaced_file() {
    let mut server = TestServer::new().await;
    server
        .open(
            "Service/Widget.php",
            "<?php\nnamespace App\\Service;\n\nclass Widget {}\n",
        )
        .await;
    server
        .open("main.php", "<?php\nnamespace App;\nnew Widget();\n")
        .await;

    let resp = server.code_action("main.php", 2, 4, 2, 10).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let action = actions
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.starts_with("Add use"))
                .unwrap_or(false)
        })
        .cloned();

    assert!(
        action.is_some(),
        "expected 'Add use' quick-fix in a namespaced consumer file, got: {resp:#}"
    );
    let a = action.unwrap();
    let out = canonicalize_workspace_edit(&a["edit"], &server.uri(""));
    expect![[r#"
        // main.php
        2:0-2:0 → "use App\\Service\\Widget;\n""#]]
    .assert_eq(&out);
}

// ============================================================================
// `context.only` FILTERING
// ============================================================================
//
// `organize_imports_action` doesn't depend on the request range, so a file
// with an unsorted `use` block plus an undefined-class reference always
// offers both a `quickfix` and a `source.organizeImports` action for the
// same request — a solid fixture for asserting `context.only` filtering.

/// Sanity check that both actions actually show up unfiltered, so the
/// filtering tests below are pinning real behavior, not an empty response.
#[tokio::test]
async fn code_action_only_absent_returns_both_kinds() {
    let mut server = TestServer::new().await;
    server
        .open(
            "Service/Widget.php",
            "<?php\nnamespace App\\Service;\n\nclass Widget {}\n",
        )
        .await;
    server
        .open(
            "main.php",
            "<?php\nnamespace App;\n\nuse App\\Zeta;\nuse App\\Alpha;\n\nnew Widget();\n",
        )
        .await;

    let resp = server.code_action("main.php", 6, 4, 6, 10).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let kinds: Vec<&str> = actions
        .iter()
        .map(|a| a["kind"].as_str().unwrap_or(""))
        .collect();
    assert!(
        kinds.contains(&"quickfix"),
        "expected an 'Add use' quickfix, got {kinds:?}"
    );
    assert!(
        kinds.contains(&"source.organizeImports"),
        "expected an organize-imports action, got {kinds:?}"
    );
}

#[tokio::test]
async fn code_action_only_quickfix_excludes_organize_imports() {
    let mut server = TestServer::new().await;
    server
        .open(
            "Service/Widget.php",
            "<?php\nnamespace App\\Service;\n\nclass Widget {}\n",
        )
        .await;
    server
        .open(
            "main.php",
            "<?php\nnamespace App;\n\nuse App\\Zeta;\nuse App\\Alpha;\n\nnew Widget();\n",
        )
        .await;

    let resp = server
        .code_action_only("main.php", 6, 4, 6, 10, &["quickfix"])
        .await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let kinds: Vec<&str> = actions
        .iter()
        .map(|a| a["kind"].as_str().unwrap_or(""))
        .collect();
    assert!(
        !kinds.is_empty(),
        "expected the Add-use quickfix to survive filtering"
    );
    assert!(
        kinds.iter().all(|k| *k == "quickfix"),
        "only=[quickfix] should filter out non-quickfix actions, got {kinds:?}"
    );
}

#[tokio::test]
async fn code_action_only_organize_imports_excludes_quickfix() {
    let mut server = TestServer::new().await;
    server
        .open(
            "Service/Widget.php",
            "<?php\nnamespace App\\Service;\n\nclass Widget {}\n",
        )
        .await;
    server
        .open(
            "main.php",
            "<?php\nnamespace App;\n\nuse App\\Zeta;\nuse App\\Alpha;\n\nnew Widget();\n",
        )
        .await;

    let resp = server
        .code_action_only("main.php", 6, 4, 6, 10, &["source.organizeImports"])
        .await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let kinds: Vec<&str> = actions
        .iter()
        .map(|a| a["kind"].as_str().unwrap_or(""))
        .collect();
    assert!(
        !kinds.is_empty(),
        "expected the organize-imports action to survive filtering"
    );
    assert!(
        kinds.iter().all(|k| *k == "source.organizeImports"),
        "only=[source.organizeImports] should filter out the quickfix, got {kinds:?}"
    );
}

/// `only: ["refactor"]` must also match the more specific descendant kind
/// `refactor.extract` — the LSP code-action-kind hierarchy is prefix-based.
#[tokio::test]
async fn code_action_only_refactor_includes_extract_descendant() {
    let mut server = TestServer::new().await;
    server.validate_syntax(false);
    server
        .open(
            "main.php",
            "<?php\nfunction f(): int {\n    return 1 + 2;\n}\n",
        )
        .await;

    let resp = server
        .code_action_only("main.php", 2, 11, 2, 16, &["refactor"])
        .await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let kinds: Vec<&str> = actions
        .iter()
        .map(|a| a["kind"].as_str().unwrap_or(""))
        .collect();
    assert!(
        kinds.contains(&"refactor.extract"),
        "only=[refactor] should include the refactor.extract descendant, got {kinds:?}"
    );
}
