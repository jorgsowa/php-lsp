//! Document + workspace symbol coverage.

use super::*;

use expect_test::expect;
use serde_json::json;

#[tokio::test]
async fn document_symbols_outline() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_document_symbols(
            r#"<?php
class Greeter {
    public function hello(): string { return 'hi'; }
    public function bye(): void {}
}
function top_level(): void {}
"#,
        )
        .await;
    expect![[r#"
        Class Greeter @L1
          Method hello @L2
          Method bye @L3
        Function top_level @L5"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_symbols_nested_enum() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_document_symbols(
            r#"<?php
enum Status {
    case Active;
    case Inactive;
}
"#,
        )
        .await;
    expect![[r#"
        Enum Status @L1
          EnumMember Active @L2
          EnumMember Inactive @L3"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_symbols_interface() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_document_symbols(
            r#"<?php
interface Writable {
    public function write(): void;
}
"#,
        )
        .await;
    expect![[r#"
        Interface Writable @L1
          Method write @L2"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn workspace_symbols_finds_class_by_query() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_workspace_symbols(
            r#"<?php
class MagicRegistry {}
function abracadabra(): void {}
"#,
            "MagicReg",
        )
        .await;
    expect!["Class       MagicRegistry @ main.php:1"].assert_eq(&out);
}

/// Workspace symbol search must find `User` by short name even though the FQN
/// is `App\Model\User`.
#[tokio::test]
async fn workspace_symbol_finds_class_by_short_name() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;
    let out = server
        .check_workspace_symbols(
            r#"<?php
        // This file won't be used; we're searching the fixture
        "#,
            "User",
        )
        .await;
    expect![[r#"
        Class       User @ src/Model/User.php:4
        Property    $users @ src/Service/Registry.php:9"#]]
    .assert_eq(&out);
}

/// `workspace/symbol` must find class properties, not just methods/classes —
/// `workspace_symbols_from_index` reads `FileIndex` directly and previously
/// never iterated `cls.properties` at all.
#[tokio::test]
async fn workspace_symbols_finds_class_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_workspace_symbols(
            r#"<?php
class Config {
    public string $apiKey = '';
}
"#,
            "apiKey",
        )
        .await;
    expect!["Property    $apiKey @ main.php:2"].assert_eq(&out);
}

/// `workspace/symbol` must find class constants — same gap as properties.
#[tokio::test]
async fn workspace_symbols_finds_class_constant() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_workspace_symbols(
            r#"<?php
class Config {
    const MAX_RETRIES = 3;
}
"#,
            "MAX_RETRIES",
        )
        .await;
    expect!["Constant    MAX_RETRIES @ main.php:1"].assert_eq(&out);
}

/// workspace/symbol with no matches returns `[]`, not `null`.
#[tokio::test]
async fn workspace_symbols_returns_empty_array_not_null_on_no_match() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open("main.php", "<?php\nclass Foo {}\n").await;
    let out = s
        .snapshot_workspace_symbols("ThisQueryMatchesNothing")
        .await;
    expect!["<no symbols>"].assert_eq(&out);
}

// --- workspaceSymbol/resolve ---

#[tokio::test]
async fn symbol_resolve_fills_range_for_open_class() {
    let mut server = TestServer::new().await;
    server
        .open("resolve.php", "<?php\nclass Resolvable {}\n")
        .await;
    let uri = server.uri("resolve.php");

    let symbol = json!({
        "name": "Resolvable",
        "kind": 5,
        "location": { "uri": uri },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;
    let out = render_resolved_workspace_symbol(&resp, &server.uri(""));
    expect!["Resolvable (Class) @ resolve.php:1:6-1:16"].assert_eq(&out);
}

#[tokio::test]
async fn symbol_resolve_fills_range_for_open_function() {
    let mut server = TestServer::new().await;
    server
        .open("resolve.php", "<?php\nfunction myFunc() {}\n")
        .await;
    let uri = server.uri("resolve.php");

    let symbol = json!({
        "name": "myFunc",
        "kind": 12,
        "location": { "uri": uri },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;
    let out = render_resolved_workspace_symbol(&resp, &server.uri(""));
    expect!["myFunc (Function) @ resolve.php:1:9-1:15"].assert_eq(&out);
}

#[tokio::test]
async fn symbol_resolve_unchanged_for_closed_file() {
    let mut server = TestServer::new().await;

    let symbol = json!({
        "name": "ClosedClass",
        "kind": 5,
        "location": { "uri": "file:///nonexistent_closed.php" },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;
    let out = render_resolved_workspace_symbol(&resp, "file:///");
    expect!["ClosedClass (Class) @ nonexistent_closed.php [uri-only]"].assert_eq(&out);
}

#[tokio::test]
async fn symbol_resolve_passthrough_for_already_resolved_location() {
    let mut server = TestServer::new().await;
    server
        .open("passthrough.php", "<?php\nfunction alreadyResolved() {}\n")
        .await;
    let uri = server.uri("passthrough.php");

    let symbol = json!({
        "name": "alreadyResolved",
        "kind": 12,
        "location": {
            "uri": uri,
            "range": {
                "start": { "line": 1, "character": 9 },
                "end":   { "line": 1, "character": 24 },
            },
        },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;
    let out = render_resolved_workspace_symbol(&resp, &server.uri(""));
    expect!["alreadyResolved (Function) @ passthrough.php:1:9-1:24"].assert_eq(&out);
}

#[tokio::test]
async fn symbol_resolve_finds_first_occurrence_when_name_appears_multiple_times() {
    let mut server = TestServer::new().await;
    server
        .open(
            "multi.php",
            "<?php\nclass Duplicate {}\nfunction test() { $x = new Duplicate(); }\n",
        )
        .await;
    let uri = server.uri("multi.php");

    let symbol = json!({
        "name": "Duplicate",
        "kind": 5,
        "location": { "uri": uri },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;
    let out = render_resolved_workspace_symbol(&resp, &server.uri(""));
    expect!["Duplicate (Class) @ multi.php:1:6-1:15"].assert_eq(&out);
}

#[tokio::test]
async fn symbol_resolve_symbol_at_line_zero() {
    let mut server = TestServer::new().await;
    server.open("line0.php", "<?php class AtStart {}\n").await;
    let uri = server.uri("line0.php");

    let symbol = json!({
        "name": "AtStart",
        "kind": 5,
        "location": { "uri": uri },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;
    let out = render_resolved_workspace_symbol(&resp, &server.uri(""));
    expect!["AtStart (Class) @ line0.php:0:12-0:19"].assert_eq(&out);
}

#[tokio::test]
async fn symbol_resolve_nonexistent_symbol_in_source() {
    let mut server = TestServer::new().await;
    server
        .open("noexist.php", "<?php\nclass RealClass {}\n")
        .await;
    let uri = server.uri("noexist.php");

    let symbol = json!({
        "name": "NonExistentClass",
        "kind": 5,
        "location": { "uri": uri },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;
    let out = render_resolved_workspace_symbol(&resp, &server.uri(""));
    expect!["NonExistentClass (Class) @ noexist.php [uri-only]"].assert_eq(&out);
}

#[tokio::test]
async fn symbol_resolve_is_idempotent() {
    let mut server = TestServer::new().await;
    server
        .open("idempotent.php", "<?php\nclass TestClass {}\n")
        .await;
    let uri = server.uri("idempotent.php");

    let symbol = json!({
        "name": "TestClass",
        "kind": 5,
        "location": { "uri": uri },
    });

    let resolved_once = server.workspace_symbol_resolve(symbol.clone()).await;
    let resolved_twice = server
        .workspace_symbol_resolve(resolved_once["result"].clone())
        .await;

    assert_eq!(
        resolved_once["result"], resolved_twice["result"],
        "calling resolve twice must return identical results (idempotent)"
    );
    let out = render_resolved_workspace_symbol(&resolved_once, &server.uri(""));
    expect!["TestClass (Class) @ idempotent.php:1:6-1:15"].assert_eq(&out);
}

/// The symbol's `range.start` must be ≤ `selection_range.start`. This is an
/// LSP invariant that clients rely on for folding and breadcrumb behaviour.
#[tokio::test]
async fn symbols_range_start_lte_selection_range_start() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        "<?php\nfunction hello(string $x): int { return 0; }",
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    let syms = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(!syms.is_empty(), "expected at least one symbol for hello()");
    // Verify LSP invariant: range.start must be ≤ selectionRange.start for every symbol.
    for sym in &syms {
        let range_start_line = sym["range"]["start"]["line"].as_u64().unwrap_or(u64::MAX);
        let sel_start_line = sym["selectionRange"]["start"]["line"]
            .as_u64()
            .unwrap_or(u64::MAX);
        assert!(
            range_start_line <= sel_start_line,
            "range.start.line ({range_start_line}) must be ≤ selectionRange.start.line ({sel_start_line})"
        );
    }
}

/// A partial AST from a parse error must still return valid symbols for the
/// declarations that did parse successfully.
#[tokio::test]
async fn symbols_partial_ast_on_parse_error_returns_valid_symbols() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        r#"<?php
function valid() {}
class {
"#,
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    assert_document_symbol_containment(&resp);
    expect![[r#"
        Function valid @L1
        Class <error> @L2"#]]
    .assert_eq(&render_document_symbols(&resp));
}

/// The function symbol's `range.start.line` must be the line where the
/// `function` keyword appears, not the first line of the file.
#[tokio::test]
async fn symbols_function_range_starts_at_function_keyword_line() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open("test.php", "<?php\nfunction myFunc() {}").await;
    let resp = s.document_symbols("test.php").await;
    let syms = resp["result"].as_array().cloned().unwrap_or_default();
    let func = syms
        .iter()
        .find(|s| s["name"].as_str() == Some("myFunc"))
        .expect("myFunc symbol not found");
    let range_line = func["range"]["start"]["line"].as_u64().unwrap_or(0);
    assert_eq!(
        range_line, 1,
        "function range must start at line 1 (where 'function' keyword is)"
    );
    let sel_line = func["selectionRange"]["start"]["line"]
        .as_u64()
        .unwrap_or(0);
    assert_eq!(sel_line, 1, "selectionRange must also start at line 1");
}

#[tokio::test]
async fn document_symbols_trait() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_document_symbols(
            r#"<?php
trait Loggable {
    public function log(): void {}
}
"#,
        )
        .await;
    expect![[r#"
        Class Loggable @L1
          Method log @L2"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_symbols_namespace() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_document_symbols(
            r#"<?php
namespace App\Services;
class Mailer {
    public function send(): void {}
}
"#,
        )
        .await;
    expect![[r#"
        Class Mailer @L2
          Method send @L3"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_symbols_class_with_properties() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_document_symbols(
            r#"<?php
class User {
    public string $name = '';
    private int $age = 0;
}
"#,
        )
        .await;
    expect![[r#"
        Class User @L1
          Property $name @L2
          Property $age @L3"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_symbols_class_with_constants() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_document_symbols(
            r#"<?php
class Config {
    const VERSION = '1.0';
    const MAX_RETRIES = 3;
}
"#,
        )
        .await;
    expect![[r#"
        Class Config @L1
          Constant VERSION @L2
          Constant MAX_RETRIES @L3"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_symbols_trait_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_document_symbols(
            r#"<?php
trait Serializable {
    public function serialize(): string { return ''; }
    public function unserialize(string $data): void {}
}
"#,
        )
        .await;
    expect![[r#"
        Class Serializable @L1
          Method serialize @L2
          Method unserialize @L3"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_symbols_interface_with_constants() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_document_symbols(
            r#"<?php
interface Limits {
    const MAX_SIZE = 100;
    public function check(): bool;
}
"#,
        )
        .await;
    expect![[r#"
        Interface Limits @L1
          Constant MAX_SIZE @L2
          Method check @L3"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_symbols_deprecated_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        "<?php\n/** @deprecated */\nfunction oldApi(): void {}\n",
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    let syms = resp["result"].as_array().cloned().unwrap_or_default();
    let func = syms
        .iter()
        .find(|s| s["name"].as_str() == Some("oldApi"))
        .expect("oldApi symbol not found");
    assert!(
        func["deprecated"].as_bool().unwrap_or(false),
        "deprecated function must have deprecated=true, got: {func}"
    );
}

#[tokio::test]
async fn document_symbols_non_deprecated_function_has_no_deprecated_field() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open("test.php", "<?php\nfunction freshApi(): void {}\n")
        .await;
    let resp = s.document_symbols("test.php").await;
    let syms = resp["result"].as_array().cloned().unwrap_or_default();
    let func = syms
        .iter()
        .find(|s| s["name"].as_str() == Some("freshApi"))
        .expect("freshApi symbol not found");
    assert!(
        !func["deprecated"].as_bool().unwrap_or(false),
        "non-deprecated function must not have deprecated=true, got: {func}"
    );
}

#[tokio::test]
async fn document_symbols_deprecated_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        "<?php\n/** @deprecated */\nclass LegacyService {}\n",
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    let syms = resp["result"].as_array().cloned().unwrap_or_default();
    let cls = syms
        .iter()
        .find(|s| s["name"].as_str() == Some("LegacyService"))
        .expect("LegacyService symbol not found");
    assert!(
        cls["deprecated"].as_bool().unwrap_or(false),
        "deprecated class must have deprecated=true, got: {cls}"
    );
}

#[tokio::test]
async fn document_symbols_deprecated_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        "<?php\nclass Repo {\n    /** @deprecated */\n    public function findAll(): array { return []; }\n}\n",
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    let syms = resp["result"].as_array().cloned().unwrap_or_default();
    let cls = syms
        .iter()
        .find(|s| s["name"].as_str() == Some("Repo"))
        .expect("Repo class not found");
    let method = cls["children"]
        .as_array()
        .and_then(|ch| ch.iter().find(|m| m["name"].as_str() == Some("findAll")))
        .expect("findAll method not found");
    assert!(
        method["deprecated"].as_bool().unwrap_or(false),
        "deprecated method must have deprecated=true, got: {method}"
    );
}

/// When a class member name matches an earlier top-level function, `selectionRange`
/// falls inside the member's `fullRange`, not the function's range.
#[tokio::test]
async fn document_symbols_selection_range_class_member_name_matches_earlier_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        r#"<?php
function process(): void {}
class Pipeline {
    private array $process = [];
    public function process(): void {}
    const process = 'noop';
}
"#,
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    assert_document_symbol_containment(&resp);
    expect![[r#"
        Function process @L1
        Class Pipeline @L2
          Property $process @L3
          Method process @L4
          Constant process @L5"#]]
    .assert_eq(&render_document_symbols(&resp));
}

/// When two classes share a method name, each method's `selectionRange` points
/// into its own class, not the other.
#[tokio::test]
async fn document_symbols_selection_range_second_class_method_not_confused() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        r#"<?php
class Alpha {
    public function process(): void {}
}
class Beta {
    public function process(): void {}
}
"#,
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    assert_document_symbol_containment(&resp);
    expect![[r#"
        Class Alpha @L1
          Method process @L2
        Class Beta @L4
          Method process @L5"#]]
    .assert_eq(&render_document_symbols(&resp));
}

/// When an interface method name matches an earlier top-level function, `selectionRange`
/// falls inside the interface member's range.
#[tokio::test]
async fn document_symbols_selection_range_interface_method_matches_earlier_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        r#"<?php
function read(): string { return ''; }
interface Reader {
    public function read(): string;
}
"#,
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    assert_document_symbol_containment(&resp);
    expect![[r#"
        Function read @L1
        Interface Reader @L2
          Method read @L3"#]]
    .assert_eq(&render_document_symbols(&resp));
}

/// When a trait method name matches an earlier top-level function, `selectionRange`
/// falls inside the trait member's range.
#[tokio::test]
async fn document_symbols_selection_range_trait_method_matches_earlier_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        r#"<?php
function format(): string { return ''; }
trait Formatter {
    public function format(): string { return ''; }
}
"#,
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    assert_document_symbol_containment(&resp);
    expect![[r#"
        Function format @L1
        Class Formatter @L2
          Method format @L3"#]]
    .assert_eq(&render_document_symbols(&resp));
}

/// When an enum case name matches an earlier top-level function, `selectionRange`
/// falls inside the enum member's range.
#[tokio::test]
async fn document_symbols_selection_range_enum_case_matches_earlier_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        r#"<?php
function Active(): bool { return true; }
enum Status {
    case Active;
}
"#,
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    assert_document_symbol_containment(&resp);
    expect![[r#"
        Function Active @L1
        Enum Status @L2
          EnumMember Active @L3"#]]
    .assert_eq(&render_document_symbols(&resp));
}

/// When an enum method name matches an earlier top-level function, `selectionRange`
/// falls inside the enum member's range.
#[tokio::test]
async fn document_symbols_selection_range_enum_method_matches_earlier_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open(
        "test.php",
        r#"<?php
function label(): string { return ''; }
enum Priority {
    case Low;
    public function label(): string { return $this->name; }
}
"#,
    )
    .await;
    let resp = s.document_symbols("test.php").await;
    assert_document_symbol_containment(&resp);
    expect![[r#"
        Function label @L1
        Enum Priority @L2
          EnumMember Low @L3
          Method label @L4"#]]
    .assert_eq(&render_document_symbols(&resp));
}
