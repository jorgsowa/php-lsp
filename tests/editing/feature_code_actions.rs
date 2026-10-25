//! Code action smoke coverage. Each scenario uses a two-`$0` selection to
//! name the range the action acts on.

use super::*;

use expect_test::expect;
use serde_json::Value;

fn deferred_action(resp: &Value, title: &str, kind: &str, resolve_tag: &str) -> Value {
    assert!(
        resp["error"].is_null(),
        "code action request failed: {resp:#}"
    );
    let actions = resp["result"]
        .as_array()
        .expect("code action response must contain an action array");
    let action = actions
        .iter()
        .find(|action| action["title"].as_str() == Some(title))
        .unwrap_or_else(|| panic!("missing {title:?} action in response: {resp:#}"));

    assert_eq!(action["kind"].as_str(), Some(kind));
    assert!(action["edit"].is_null(), "{title:?} must defer its edit");
    assert_eq!(
        action["data"]["php_lsp_resolve"].as_str(),
        Some(resolve_tag),
        "{title:?} must carry its resolver tag"
    );
    action.clone()
}

fn resolved_workspace_edit(resp: &Value) -> &Value {
    assert!(
        resp["error"].is_null(),
        "code action resolve failed: {resp:#}"
    );
    let edit = &resp["result"]["edit"];
    assert!(
        edit.is_object(),
        "resolved action must contain an edit: {resp:#}"
    );
    edit
}

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

// Deferred actions must return their edits through `codeAction/resolve`.

#[tokio::test]
async fn code_action_resolve_return_type() {
    let mut server = TestServer::new().await;
    server
        .open("rt.php", "<?php\nfunction noReturn() { return 42; }\n")
        .await;

    let resp = server.code_action("rt.php", 1, 0, 1, 30).await;
    let action = deferred_action(
        &resp,
        "Add return type `: mixed`",
        "refactor",
        "return_type",
    );

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
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
    let action = deferred_action(&resp, "Generate PHPDoc", "refactor", "phpdoc");

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
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
    let action = deferred_action(&resp, "Generate constructor", "refactor", "constructor");

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
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
    let action = deferred_action(
        &resp,
        "Generate 2 getters/setters",
        "refactor",
        "getters_setters",
    );

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
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
    let action = deferred_action(&resp, "Implement missing method", "quickfix", "implement");
    assert_eq!(action["isPreferred"].as_bool(), Some(true));

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
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
    let action = deferred_action(
        &resp,
        "Promote 2 constructor parameters",
        "refactor",
        "promote",
    );

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
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
    let action = deferred_action(
        &resp,
        "Promote constructor parameter",
        "refactor",
        "promote",
    );

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
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
    let action = deferred_action(
        &resp,
        "Promote constructor parameter",
        "refactor",
        "promote",
    );

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
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
    let action = deferred_action(
        &resp,
        "Promote 2 constructor parameters",
        "refactor",
        "promote",
    );

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
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
    assert_eq!(resolved["result"]["kind"].as_str(), Some("refactor"));
    assert!(
        resolved["result"]["data"].is_null(),
        "data must remain absent"
    );
    assert!(
        resolved["result"]["edit"].is_null(),
        "no edit should be added for data-less actions"
    );
}

// Promotion must be unavailable when no safe property-to-parameter mapping exists.

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

#[tokio::test]
async fn promote_action_resolve_no_trailing_newline() {
    let mut server = TestServer::new().await;
    let out = server
        .check_code_action_apply(
            r#"<?php
class Foo {
    public string $name$0;
    public function __construct(string $name) {
        $this->name = $name;
    }
}"#,
            "Promote constructor parameter",
        )
        .await;
    expect![[r#"
        <?php
        class Foo {
            public function __construct(public string $name) {
            }
        }"#]]
    .assert_eq(&out);
}

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

#[tokio::test]
async fn promote_action_supports_unbraced_namespace() {
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

#[tokio::test]
async fn promote_action_not_offered_for_static_assignment() {
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

#[tokio::test]
async fn promote_action_not_offered_for_conditional_assignment() {
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

#[tokio::test]
async fn promote_action_not_offered_for_mismatched_parameter() {
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

#[tokio::test]
async fn promote_action_supports_multiple_properties() {
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
    let action = deferred_action(
        &resp,
        "Implement 2 missing methods",
        "quickfix",
        "implement",
    );

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
    expect![[r#"
        // Report.php
        1:35-1:35 → "\n    public function print(): void\n    {\n        throw new \\RuntimeException('Not implemented');\n    }\n\n    public function getLabel(): string\n    {\n        throw new \\RuntimeException('Not implemented');\n    }\n\n""#]]
    .assert_eq(&out);
}

// Resolving an implementation action must not block the request loop.
#[tokio::test]
async fn code_action_resolve_implement_stays_responsive_on_large_workspace() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(
        workspace.path().join("Printable.php"),
        "<?php\ninterface Printable { public function print(): void; }\n",
    )
    .unwrap();
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
        .assert_request_stays_responsive_via_gate(
            "codeAction/resolve",
            action,
            php_lsp::backend::debug_gate::GATE_CODE_ACTION_RESOLVE,
        )
        .await;
}

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
    let action = deferred_action(&resp, "Implement missing method", "quickfix", "implement");

    let resolved = server.code_action_resolve(action).await;
    let out = canonicalize_workspace_edit(resolved_workspace_edit(&resolved), &server.uri(""));
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
    assert!(
        resp["error"].is_null(),
        "code action request failed: {resp:#}"
    );
    let actions = resp["result"]
        .as_array()
        .expect("code action response must contain an action array");
    let action = actions
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .map(|t| t.starts_with("Add use function App\\Helpers\\tap"))
                .unwrap_or(false)
        })
        .expect("expected an Add use function quick-fix");

    assert_eq!(action["kind"].as_str(), Some("quickfix"));
    assert!(action["edit"].is_object(), "quick-fix must include an edit");
    let out = canonicalize_workspace_edit(&action["edit"], &server.uri(""));
    expect![[r#"
        // main.php
        2:0-2:0 → "use function App\\Helpers\\tap;\n""#]]
    .assert_eq(&out);
}

// --- Quick-fix: UndefinedClass → use FQN; ---

/// A unique workspace class should be offered as an import even when its file
/// is indexed but not open in the editor.
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
    expect![[r#"
        quickfix         Add use App\Service\Widget [edit]
        refactor.extract Extract variable [edit]"#]]
    .assert_eq(&render_code_actions(&resp));
    let action = resp["result"].as_array().and_then(|actions| {
        actions
            .iter()
            .find(|a| a["title"] == "Add use App\\Service\\Widget")
    });
    let action = action.expect("expected an Add use quick-fix");
    let out = canonicalize_workspace_edit(&action["edit"], &server.uri(""));
    expect![[r#"
        // src/main.php
        2:0-2:0 → "use App\\Service\\Widget;\n""#]]
    .assert_eq(&out);
}

/// The namespace-resolved diagnostic FQN is reduced to its short name only
/// after exact resolution has failed, so a unique imported class is offered.
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
    expect![[r#"
        quickfix         Add use App\Service\Widget [edit]
        refactor.extract Extract variable [edit]"#]]
    .assert_eq(&render_code_actions(&resp));
    let action = resp["result"].as_array().and_then(|actions| {
        actions
            .iter()
            .find(|a| a["title"] == "Add use App\\Service\\Widget")
    });
    let action = action.expect("expected an Add use quick-fix");
    let out = canonicalize_workspace_edit(&action["edit"], &server.uri(""));
    expect![[r#"
        // main.php
        2:0-2:0 → "use App\\Service\\Widget;\n""#]]
    .assert_eq(&out);
}

/// The import quick-fix goes after `declare(strict_types=1)`, not before it.
#[tokio::test]
async fn code_action_quickfix_undefined_class_inserts_after_declare() {
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
            "<?php\ndeclare(strict_types=1);\n\nnamespace App;\n\nclass User {\n    use HasFactory;\n    function f() { return new Widget(); }\n}\n",
        )
        .await;

    let resp = server.code_action("main.php", 7, 30, 7, 36).await;
    let action = resp["result"]
        .as_array()
        .and_then(|actions| {
            actions
                .iter()
                .find(|a| a["title"] == "Add use App\\Service\\Widget")
        })
        .unwrap_or_else(|| panic!("expected an Add use quick-fix: {resp:#}"));
    let out = canonicalize_workspace_edit(&action["edit"], &server.uri(""));
    expect![[r#"
        // main.php
        4:0-4:0 → "use App\\Service\\Widget;\n""#]]
    .assert_eq(&out);
}

/// Several classes with the same short name are ambiguous, so an import
/// quick-fix must not guess between them.
#[tokio::test]
async fn code_action_quickfix_undefined_class_ambiguous_candidates_not_offered() {
    let mut server = TestServer::new().await;
    server
        .open("A/Widget.php", "<?php\nnamespace A;\n\nclass Widget {}\n")
        .await;
    server
        .open("B/Widget.php", "<?php\nnamespace B;\n\nclass Widget {}\n")
        .await;
    server
        .open("main.php", "<?php\nnamespace App;\nnew Widget();\n")
        .await;

    let resp = server.code_action("main.php", 2, 4, 2, 10).await;
    expect!["refactor.extract Extract variable [edit]"].assert_eq(&render_code_actions(&resp));
}

// `context.only` filtering

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
    expect![[r#"
        quickfix         Add use App\Service\Widget [edit]
        refactor.extract Extract variable [edit]
        source.organizeImports Organize imports [edit]"#]]
    .assert_eq(&render_code_actions(&resp));
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
    expect!["quickfix         Add use App\\Service\\Widget [edit]"]
        .assert_eq(&render_code_actions(&resp));
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
    expect!["source.organizeImports Organize imports [edit]"]
        .assert_eq(&render_code_actions(&resp));
}

#[tokio::test]
async fn code_action_only_source_includes_organize_imports_descendant() {
    let mut server = TestServer::new().await;
    server
        .open(
            "main.php",
            "<?php\nnamespace App;\n\nuse App\\Zeta;\nuse App\\Alpha;\n\nnew Missing;\n",
        )
        .await;

    let resp = server
        .code_action_only("main.php", 6, 4, 6, 11, &["source"])
        .await;
    expect!["source.organizeImports Organize imports [edit]"]
        .assert_eq(&render_code_actions(&resp));
}

#[tokio::test]
async fn code_action_only_multiple_kinds_returns_their_actions() {
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
        .code_action_only("main.php", 6, 4, 6, 10, &["quickfix", "source"])
        .await;
    expect![[r#"
        quickfix         Add use App\Service\Widget [edit]
        source.organizeImports Organize imports [edit]"#]]
    .assert_eq(&render_code_actions(&resp));
}

#[tokio::test]
async fn code_action_only_returns_no_actions_when_no_kind_matches() {
    let mut server = TestServer::new().await;
    server.validate_syntax(false);
    server
        .open(
            "main.php",
            "<?php\nfunction f(): int {\n    return 1 + 2;\n}\n",
        )
        .await;

    let resp = server
        .code_action_only("main.php", 2, 11, 2, 16, &["quickfix"])
        .await;
    expect!["<no actions>"].assert_eq(&render_code_actions(&resp));
}

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
    expect!["refactor.extract Extract variable [edit]"].assert_eq(&render_code_actions(&resp));
}
