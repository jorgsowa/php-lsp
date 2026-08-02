//! Completion tests with full snapshot assertions and deterministic ordering.
//! All tests use `check_completion_ordered()` for comprehensive validation.

use super::*;

use expect_test::expect;
use serde_json::json;

async fn labels(s: &mut TestServer, src: &str) -> Vec<String> {
    let opened = s.open_fixture(src).await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    items
        .iter()
        .filter_map(|i| i["label"].as_str().map(str::to_owned))
        .collect()
}

fn format_completions(labels: &[String]) -> String {
    if labels.is_empty() {
        return "<empty>".to_string();
    }
    let preview: Vec<_> = labels.iter().take(15).map(|l| format!("  {}", l)).collect();
    let mut result = preview.join("\n");
    if labels.len() > 15 {
        result.push_str(&format!("\n  ... and {} more", labels.len() - 15));
    }
    result
}

fn assert_labels_contain(labels: &[String], expected: &[&str], context: &str) {
    let missing: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|e| !labels.contains(&e.to_string()))
        .collect();
    assert!(
        missing.is_empty(),
        "{context}\n\nExpected: {expected:?}\nMissing: {missing:?}\n\nActual completions ({} total):\n{}",
        labels.len(),
        format_completions(labels)
    );
}

fn assert_label_not_present(labels: &[String], unexpected: &str, context: &str) {
    assert!(
        !labels.contains(&unexpected.to_string()),
        "{context}\n\nShould NOT contain: '{unexpected}'\n\nActual completions ({} total):\n{}",
        labels.len(),
        format_completions(labels)
    );
}

fn assert_completions_exact(labels: &[String], expected: &[&str], context: &str) {
    let expected_set: std::collections::HashSet<&str> = expected.iter().copied().collect();
    let actual_set: std::collections::HashSet<&str> = labels.iter().map(|s| s.as_str()).collect();

    if expected_set != actual_set {
        let missing: Vec<_> = expected_set.difference(&actual_set).copied().collect();
        let extra: Vec<_> = actual_set.difference(&expected_set).copied().collect();
        panic!(
            "{context}\n\nMissing: {missing:?}\nUnexpected: {extra:?}\n\nActual completions ({} total):\n{}",
            labels.len(),
            format_completions(labels)
        );
    }
}

fn assert_ordered(output: &str, expected_sequence: &[&str], context: &str) {
    let lines: Vec<&str> = output.lines().collect();
    let mut last_found_idx = 0;

    for expected_label in expected_sequence {
        let found = lines[last_found_idx..]
            .iter()
            .position(|line| line.contains(expected_label))
            .map(|pos| last_found_idx + pos);

        match found {
            Some(idx) => last_found_idx = idx + 1,
            None => panic!(
                "{context}\n\nExpected '{expected_label}' after position {last_found_idx}\n\nActual output:\n{output}"
            ),
        }
    }
}

fn assert_exact_items(output: &str, expected_labels: &[&str], context: &str) {
    let actual_labels: Vec<&str> = output
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .collect();

    let expected_set: std::collections::HashSet<&str> = expected_labels.iter().copied().collect();
    let actual_set: std::collections::HashSet<&str> = actual_labels.iter().copied().collect();

    if expected_set != actual_set {
        let missing: Vec<_> = expected_set.difference(&actual_set).copied().collect();
        let extra: Vec<_> = actual_set.difference(&expected_set).copied().collect();
        panic!(
            "{context}\n\nMissing: {missing:?}\nUnexpected: {extra:?}\n\nActual output:\n{output}"
        );
    }
}

#[tokio::test]
async fn completion_arrow_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Greeter {
    public function hello(): string { return 'hi'; }
    public function bye(): void {}
}
$g = new Greeter();
$g->h$0
"#,
        )
        .await;
    expect!["Method      hello"].assert_eq(&out);
}

#[tokio::test]
async fn completion_arrow_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class User {
    public string $name = '';
    public int $age = 0;
}
$u = new User();
$u->na$0
"#,
        )
        .await;
    expect![["Property    $name"]].assert_eq(&out);
}

#[tokio::test]
async fn completion_method_chain_does_not_panic() {
    // Completion on a method-chain receiver (`$obj->bar()->`) must not panic.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Chain {
    public function bar(): Chain {}
    public function qux(): void {}
}
$c = new Chain();
$c->bar()->$0
"#,
        )
        .await;
    expect![[r#"
        Method      bar
        Method      qux"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_arrow_excludes_class_constants() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Config {
    const VERSION = '1.0';
    public string $name = '';
    public function getName(): string { return $this->name; }
}
$c = new Config();
$c->$0
"#,
        )
        .await;
    // Constants (VERSION) must not appear in arrow completion; only instance members.
    expect![[r#"
        Property    $name
        Method      getName"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_double_colon_static_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Reg {
    public static function get(): void {}
    public static function set(): void {}
}
Reg::$0
"#,
        )
        .await;
    expect![[r#"
        Method      get
        Method      set"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_double_colon_static_via_use_import_alias() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /Str.php
<?php
namespace Illuminate\Support;
class Str {
    public static function camel(string $value): string {}
    public static function lower(string $value): string {}
}

//- /main.php
<?php
use Illuminate\Support\Str;
Str::$0
"#,
        )
        .await;
    expect![[r#"
        Method      camel
        Method      lower"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_double_colon_static_via_use_import_as_alias() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /Str.php
<?php
namespace Illuminate\Support;
class Str {
    public static function camel(string $value): string {}
    public static function lower(string $value): string {}
}

//- /main.php
<?php
use Illuminate\Support\Str as StringHelper;
StringHelper::$0
"#,
        )
        .await;
    expect![[r#"
        Method      camel
        Method      lower"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_namespace_prefix() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /src/App/Greeter.php
<?php
namespace App;
class Greeter {}

//- /src/main.php
<?php
$g = new \App\$0
"#,
        )
        .await;
    expect![[r#"Class       Greeter | App\Greeter"#]].assert_eq(&out);
}

#[tokio::test]
async fn completion_keyword_in_top_level() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
func$0
"#,
        )
        .await;
    expect![[r#"
        Keyword     function
        Function    function_exists"#]]
    .assert_eq(&out);
}

/// PHP type keywords (soft reserved words) must appear in keyword completions so
/// developers can type `vo` and get `void`, `bo` → `bool`, `str` → `string`, etc.
#[tokio::test]
async fn completion_type_keywords_suggested() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
vo$0
"#,
        )
        .await;
    expect!["Keyword     void"].assert_eq(&out);
}

#[tokio::test]
async fn completion_variable_in_scope() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
function f(string $name, int $count): void {
    $na$0
}
"#,
        )
        .await;
    expect![[r#"
        Variable    $name"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_method_does_not_leak_to_unrelated_classes() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class A { public function foo(): void {} }
class B { public function bar(): void {} }
$a = new A();
$a->$0
"#,
        )
        .await;
    expect![[r#"
        Method      foo"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_user_class_shadows_builtin() {
    // A user-defined class whose name collides with a built-in (`ArrayObject`)
    // must win: only its own members are offered. The hand-written built-in
    // stub members (`append`, `getArrayCopy`, `count`, …) must NOT leak — the
    // exact snapshot below would grow if they did.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class ArrayObject {
    public function customMethod(): void {}
}
$x = new ArrayObject();
$x->$0
"#,
        )
        .await;
    expect!["Method      customMethod"].assert_eq(&out);
}

#[tokio::test]
async fn completion_enum_case_access() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
enum Status { case Active; case Inactive; }
Status::$0
"#,
        )
        .await;
    expect![[r#"
        Constant    Active
        Constant    Inactive
        Method      cases"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_after_new_offers_class_names() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Widget {}
class Gadget {}
$x = new $0
"#,
        )
        .await;
    expect![[r#"
        Variable    $GLOBALS | superglobal
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Variable    $x
        Class       Gadget
        Class       Widget
        Constant    __CLASS__ | Current class name
        Constant    __DIR__ | Directory of the current file
        Constant    __FILE__ | Absolute path of the current file
        Constant    __FUNCTION__ | Current function name
        Constant    __LINE__ | Current line number
        Constant    __METHOD__ | Current method name (Class::method)
        Constant    __NAMESPACE__ | Current namespace
        Constant    __TRAIT__ | Current trait name
        Function    abs
        Keyword     abstract
        Function    acos
        Function    addslashes
        Keyword     and
        Keyword     array
        Function    array_chunk
        Function    array_combine
        Function    array_diff
        Function    array_fill
        Function    array_fill_keys
        Function    array_filter
        Function    array_flip
        Function    array_intersect
        Function    array_key_exists
        Function    array_keys
        Function    array_map
        Function    array_merge
        Function    array_pad
        Function    array_pop
        Function    array_push
        Function    array_reduce
        Function    array_replace
        Function    array_reverse
        Function    array_search
        Function    array_shift
        Function    array_slice
        Function    array_splice
        Function    array_unique
        Function    array_unshift
        Function    array_values
        Function    array_walk
        Function    array_walk_recursive
        Function    arsort
        Keyword     as
        Function    asin
        Function    asort
        Function    atan
        Function    atan2
        Function    base64_decode
        Function    base64_encode
        Function    basename
        Keyword     bool
        Function    boolval
        Keyword     break
        Function    call_user_func
        Function    call_user_func_array
        Keyword     callable
        Keyword     case
        Keyword     catch
        Function    ceil
        Function    checkdate
        Keyword     class
        Function    class_exists
        Keyword     clone
        Function    closedir
        Function    compact
        Keyword     const
        Function    constant
        Keyword     continue
        Function    copy
        Function    cos
        Function    count
        Function    date
        Function    date_add
        Function    date_create
        Function    date_diff
        Function    date_format
        Function    date_sub
        Keyword     declare
        Keyword     default
        Function    define
        Function    defined
        Keyword     die
        Function    dirname
        Keyword     do
        Keyword     echo
        Keyword     else
        Keyword     elseif
        Keyword     empty
        Keyword     enddeclare
        Keyword     endfor
        Keyword     endforeach
        Keyword     endif
        Keyword     endswitch
        Keyword     endwhile
        Keyword     enum
        Keyword     eval
        Keyword     exit
        Function    exp
        Function    explode
        Keyword     extends
        Function    extract
        Keyword     false
        Function    fclose
        Function    feof
        Function    fgets
        Function    file_exists
        Function    file_get_contents
        Function    file_put_contents
        Keyword     final
        Keyword     finally
        Keyword     float
        Function    floatval
        Function    floor
        Function    fmod
        Keyword     fn
        Function    fopen
        Keyword     for
        Keyword     foreach
        Function    fputs
        Function    fread
        Function    fseek
        Function    ftell
        Keyword     function
        Function    function_exists
        Function    fwrite
        Function    get_class
        Function    get_parent_class
        Function    gettype
        Function    glob
        Keyword     global
        Keyword     goto
        Function    hash
        Function    header
        Function    headers_sent
        Function    htmlentities
        Function    htmlspecialchars
        Function    http_build_query
        Keyword     if
        Keyword     implements
        Function    implode
        Function    in_array
        Keyword     include
        Keyword     include_once
        Keyword     instanceof
        Keyword     insteadof
        Keyword     int
        Function    intdiv
        Keyword     interface
        Function    interface_exists
        Function    intval
        Function    is_a
        Function    is_array
        Function    is_bool
        Function    is_callable
        Function    is_dir
        Function    is_double
        Function    is_file
        Function    is_finite
        Function    is_float
        Function    is_infinite
        Function    is_int
        Function    is_integer
        Function    is_long
        Function    is_nan
        Function    is_null
        Function    is_numeric
        Function    is_object
        Function    is_readable
        Function    is_string
        Function    is_subclass_of
        Function    is_writable
        Keyword     isset
        Keyword     iterable
        Function    join
        Function    json_decode
        Function    json_encode
        Function    krsort
        Function    ksort
        Function    lcfirst
        Keyword     list
        Function    log
        Function    ltrim
        Keyword     match
        Function    max
        Function    md5
        Function    method_exists
        Function    microtime
        Function    min
        Keyword     mixed
        Function    mkdir
        Function    mktime
        Function    mt_rand
        Keyword     namespace
        Keyword     never
        Keyword     new
        Function    nl2br
        Keyword     null
        Function    number_format
        Function    ob_end_clean
        Function    ob_get_clean
        Function    ob_start
        Keyword     object
        Function    opendir
        Keyword     or
        Keyword     parent
        Function    parse_str
        Function    parse_url
        Function    pathinfo
        Function    pi
        Function    pow
        Function    preg_match
        Function    preg_match_all
        Function    preg_quote
        Function    preg_replace
        Function    preg_split
        Keyword     print
        Function    print_r
        Function    printf
        Keyword     private
        Function    property_exists
        Keyword     protected
        Keyword     public
        Function    rand
        Function    random_int
        Function    range
        Function    rawurldecode
        Function    rawurlencode
        Function    readdir
        Keyword     readonly
        Function    realpath
        Function    rename
        Keyword     require
        Keyword     require_once
        Keyword     return
        Function    rewind
        Function    rmdir
        Function    round
        Function    rsort
        Function    rtrim
        Function    scandir
        Keyword     self
        Function    serialize
        Function    session_destroy
        Function    session_start
        Function    setcookie
        Function    settype
        Function    sha1
        Function    sin
        Function    sleep
        Function    sort
        Function    sprintf
        Function    sqrt
        Keyword     static
        Function    str_contains
        Function    str_ends_with
        Function    str_pad
        Function    str_repeat
        Function    str_replace
        Function    str_split
        Function    str_starts_with
        Function    str_word_count
        Function    strcasecmp
        Function    strcmp
        Keyword     string
        Function    strip_tags
        Function    stripslashes
        Function    stristr
        Function    strlen
        Function    strncasecmp
        Function    strncmp
        Function    strpos
        Function    strrpos
        Function    strstr
        Function    strtolower
        Function    strtotime
        Function    strtoupper
        Function    strval
        Function    substr
        Function    substr_count
        Function    substr_replace
        Keyword     switch
        Function    tan
        Keyword     throw
        Function    time
        Keyword     trait
        Function    trim
        Keyword     true
        Keyword     try
        Function    uasort
        Function    ucfirst
        Function    ucwords
        Function    uksort
        Function    unlink
        Function    unserialize
        Function    unset
        Function    urldecode
        Function    urlencode
        Keyword     use
        Function    usleep
        Function    usort
        Keyword     var
        Function    var_dump
        Function    var_export
        Keyword     void
        Function    vsprintf
        Keyword     while
        Keyword     xor
        Keyword     yield"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_resolve_function_populates_detail_and_docs() {
    let mut server = TestServer::new().await;
    server.validate_syntax(false);
    let opened = server
        .open_fixture(
            r#"<?php
function resolveMe(): void {}
resolveM$0
"#,
        )
        .await;
    let c = opened.cursor();

    let comp = server.completion(&c.path, c.line, c.character).await;
    let items = match &comp["result"] {
        v if v.is_array() => v.as_array().unwrap().to_vec(),
        v if v["items"].is_array() => v["items"].as_array().unwrap().to_vec(),
        _ => vec![],
    };

    let resolve_me = items
        .iter()
        .find(|i| i["label"].as_str() == Some("resolveMe"))
        .cloned()
        .expect("resolveMe in completions");

    let resp = server.completion_resolve(resolve_me).await;
    let out = render_resolved_completion_item(&resp);
    expect![[r#"
resolveMe (Function)
detail: function resolveMe(): void
docs: ```php
function resolveMe(): void
```"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_resolve_function_with_docblock_populates_docs() {
    let mut server = TestServer::new().await;
    server.validate_syntax(false);
    let opened = server
        .open_fixture(
            r#"<?php
/** Greets a person */
function greet(string $name): void {}
gre$0
"#,
        )
        .await;
    let c = opened.cursor();

    let comp = server.completion(&c.path, c.line, c.character).await;
    let items: Vec<_> = match &comp["result"] {
        v if v.is_array() => v.as_array().unwrap().to_vec(),
        v if v["items"].is_array() => v["items"].as_array().unwrap().to_vec(),
        _ => vec![],
    };

    let greet = items
        .iter()
        .find(|i| i["label"].as_str() == Some("greet"))
        .cloned()
        .expect("greet in completions");

    let resp = server.completion_resolve(greet).await;
    let out = render_resolved_completion_item(&resp);
    expect![[r#"
greet (Function)
detail: function greet(string $name): void
docs: Greets a person"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_resolve_already_resolved_is_noop() {
    let mut server = TestServer::new().await;
    server.open("noop.php", "<?php").await;

    let item = json!({
        "label": "test",
        "kind": 3,
        "detail": "function test(): void",
        "documentation": {
            "kind": "markdown",
            "value": "Test function"
        }
    });

    let resp = server.completion_resolve(item).await;
    let out = render_resolved_completion_item(&resp);
    expect![[r#"
        test (Function)
        detail: function test(): void
        docs: Test function"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_resolve_unknown_symbol_returns_unchanged() {
    let mut server = TestServer::new().await;
    server.open("unknown.php", "<?php").await;

    let item = json!({
        "label": "nonExistentXyz123",
        "kind": 3
    });

    let resp = server.completion_resolve(item).await;
    let out = render_resolved_completion_item(&resp);
    expect![[r#"
        nonExistentXyz123 (Function)
        detail: <no detail>
        docs: <no docs>"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_resolve_named_argument_strips_colon_for_lookup() {
    let mut server = TestServer::new().await;
    server.validate_syntax(false);
    let opened = server
        .open_fixture(
            r#"<?php
function greet(string $name, int $age): void {}
greet(na$0
"#,
        )
        .await;
    let c = opened.cursor();

    let comp = server.completion(&c.path, c.line, c.character).await;
    let _items: Vec<_> = match &comp["result"] {
        v if v.is_array() => v.as_array().unwrap().to_vec(),
        v if v["items"].is_array() => v["items"].as_array().unwrap().to_vec(),
        _ => vec![],
    };

    let resp = server
        .completion_resolve(json!({
            "label": "name:",
            "kind": 6
        }))
        .await;
    let out = render_resolved_completion_item(&resp);
    expect![[r#"
        name: (Variable)
        detail: <no detail>
        docs: <no docs>"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_resolve_partial_detail_populates_docs() {
    let mut server = TestServer::new().await;
    server
        .open(
            "partial.php",
            "<?php\n/** My docs */\nfunction myFunc(): void {}",
        )
        .await;

    let item = json!({
        "label": "myFunc",
        "kind": 3,
        "detail": "function myFunc(): void"
    });

    let resp = server.completion_resolve(item).await;
    let out = render_resolved_completion_item(&resp);
    expect![[r#"
        myFunc (Function)
        detail: function myFunc(): void
        docs: ```php
        function myFunc(): void
        ```

        ---

        My docs"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_resolve_partial_docs_populates_detail() {
    let mut server = TestServer::new().await;
    server
        .open("partial.php", "<?php\nfunction myFunc(): void {}")
        .await;

    let item = json!({
        "label": "myFunc",
        "kind": 3,
        "documentation": {
            "kind": "markdown",
            "value": "Some doc"
        }
    });

    let resp = server.completion_resolve(item).await;
    let out = render_resolved_completion_item(&resp);
    expect![[r#"
        myFunc (Function)
        detail: function myFunc(): void
        docs: Some doc"#]]
    .assert_eq(&out);
}

/// mir resolves the caught exception type for the catch variable so that
/// member completion works on `$e->`.
#[tokio::test]
async fn completion_catch_variable_type_resolved_by_mir() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class DatabaseException {
    public function getQuery(): string {}
    public function getCode(): int {}
}
try {
    doWork();
} catch (DatabaseException $e) {
    $e->$0
}
"#,
        )
        .await;
    expect![[r#"
        Method      getCode
        Method      getQuery"#]]
    .assert_eq(&out);
}

/// mir resolves the return type of a cross-file factory method so that
/// member completion works on the result. Guards that MethodReturnsMap
/// is not load-bearing for this pattern.
#[tokio::test]
async fn completion_factory_method_return_type_resolved_by_mir() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /Factory.php
<?php
class User { public function getName(): string {} }
class Factory {
    public function makeUser(): User { return new User(); }
}

//- /main.php
<?php
$factory = new Factory();
$user = $factory->makeUser();
$user->$0
"#,
        )
        .await;
    expect!["Method      getName"].assert_eq(&out);
}

#[tokio::test]
async fn completion_this_arrow_includes_trait_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
trait Counter {
    public function tick(): void {}
    public function reset(): void {}
}
class Timer {
    use Counter;
    public function run(): void { $this->$0t; }
}
"#,
        )
        .await;
    expect![[r#"
        Method      reset
        Method      run
        Method      tick"#]]
    .assert_eq(&out);
}

// ── Attribute completion filtering ───────────────────────────────────────────

/// `#[` must only offer classes that carry `#[\Attribute]` — a plain class
/// without it must not appear.
#[tokio::test]
async fn completion_attribute_bracket_excludes_non_attribute_classes() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
#[\Attribute]
class MyRoute {}

class PlainClass {}

#[$0
"#,
        )
        .await;
    expect![[r#"
        Class       MyRoute"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_attribute_bracket_cross_file_filters_non_attributes() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /src/attrs.php
<?php
#[\Attribute]
class ValidAttr {}

class NotAnAttr {}

//- /src/main.php
<?php
#[$0
"#,
        )
        .await;
    expect![[r#"
        Class       ValidAttr"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_attribute_bracket_target_filters_class_context() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
#[\Attribute(\Attribute::TARGET_CLASS)]
class ClassOnlyAttr {}

#[\Attribute(\Attribute::TARGET_METHOD)]
class MethodOnlyAttr {}

#[\Attribute(\Attribute::TARGET_ALL)]
class AnyAttr {}

#[$0
class MyClass {}
"#,
        )
        .await;
    expect![[r#"
        Class       AnyAttr
        Class       ClassOnlyAttr"#]]
    .assert_eq(&out);
}

/// `#[` completions must exclude non-class symbols: interfaces, enums, and
/// traits cannot carry `#[\Attribute]` so they must never appear.
#[tokio::test]
async fn completion_attribute_bracket_excludes_non_class_types() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
#[\Attribute]
class ValidAttr {}

interface MyInterface {}
enum MyEnum {}
trait MyTrait {}

#[$0
"#,
        )
        .await;
    expect![[r#"
        Class       ValidAttr"#]]
    .assert_eq(&out);
}

/// `#[` before a function must show METHOD-targeted attributes but exclude
/// CLASS-only ones. Covers the `infer_attribute_target` branch returning `2|4`.
#[tokio::test]
async fn completion_attribute_bracket_target_filters_function_context() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
#[\Attribute(\Attribute::TARGET_CLASS)]
class ClassOnlyAttr {}

#[\Attribute(\Attribute::TARGET_METHOD)]
class MethodOnlyAttr {}

#[\Attribute(\Attribute::TARGET_ALL)]
class AnyAttr {}

#[$0
function doSomething(): void {}
"#,
        )
        .await;
    // AnyAttr (63 & 6 ≠ 0) and MethodOnlyAttr (4 & 6 ≠ 0) pass;
    // ClassOnlyAttr (1 & 6 = 0) is excluded.
    expect![[r#"
        Class       AnyAttr
        Class       MethodOnlyAttr"#]]
    .assert_eq(&out);
}

/// Snapshot test: `#[` must return ONLY attribute classes — no keywords,
/// built-ins, or plain classes leaking through.
#[tokio::test]
async fn completion_attribute_bracket_returns_only_attribute_classes() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
#[\Attribute]
class Middleware {}

#[\Attribute]
class MyRoute {}

class PlainClass {}

#[$0
"#,
        )
        .await;
    expect![[r#"
        Class       Middleware
        Class       MyRoute"#]]
    .assert_eq(&out);
}

/// Trigger-character path (`triggerKind: 2`, `triggerCharacter: "["`) must
/// also restrict completions to `#[\Attribute]`-annotated classes.
#[tokio::test]
async fn completion_attribute_bracket_trigger_char_filters_non_attributes() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
#[\Attribute]
class ValidAttr {}

class NotAnAttr {}

#[$0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);
    let resp = s
        .client()
        .request(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": c.line, "character": c.character },
                "context": { "triggerKind": 2, "triggerCharacter": "[" },
            }),
        )
        .await;
    let out = render_completion(&resp);
    expect![[r#"
        Class       ValidAttr"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_resolve_is_idempotent() {
    let mut server = TestServer::new().await;
    server.validate_syntax(false);
    let opened = server
        .open_fixture(
            r#"<?php
function testFunc(): void {}
test$0
"#,
        )
        .await;
    let c = opened.cursor();

    let comp = server.completion(&c.path, c.line, c.character).await;
    let items: Vec<_> = match &comp["result"] {
        v if v.is_array() => v.as_array().unwrap().to_vec(),
        v if v["items"].is_array() => v["items"].as_array().unwrap().to_vec(),
        _ => vec![],
    };

    let item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("testFunc"))
        .cloned()
        .expect("testFunc in completions");

    let resolved_once = server.completion_resolve(item.clone()).await;
    let resolved_twice = server
        .completion_resolve(resolved_once["result"].clone())
        .await;

    assert_eq!(
        resolved_once["result"], resolved_twice["result"],
        "calling resolve twice must return identical results (idempotent)"
    );
}

// === Arrow completion with type inference ===

#[tokio::test]
async fn completion_inherited_methods_via_arrow() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Base { public function baseMethod() {} }
class Child extends Base { public function childMethod() {} }
$c = new Child(); $c->$0
"#,
        )
        .await;
    expect![[r#"
        Method      baseMethod
        Method      childMethod"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_enum_arrow_name_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
enum Suit { case Hearts; }
$s = Suit::Hearts; $s->$0
"#,
        )
        .await;
    expect![[r#"
        Property    $name
        Property    name | string"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_backed_enum_has_value_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
enum Status: string { case Active = 'active'; }
$s = Status::Active; $s->$0
"#,
        )
        .await;
    expect![[r#"
        Property    $name
        Property    $value
        Property    name | string
        Property    value | string"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_backed_enum_int_has_value_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
enum Priority: int { case Low = 1; }
$p = Priority::Low; $p->$0
"#,
        )
        .await;
    expect![[r#"
        Property    $name
        Property    $value
        Property    name | string
        Property    value | int"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_pure_enum_no_value_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
enum Suit { case Hearts; }
$s = Suit::Hearts; $s->$0
"#,
        )
        .await;
    expect![[r#"
        Property    $name
        Property    name | string"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_instanceof_narrows_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Foo { public function doFoo() {} }
if ($x instanceof Foo) { $x->$0 }
"#,
        )
        .await;
    expect![[r#"
        Method      doFoo"#]]
    .assert_eq(&out);
}

/// `array_map` with a closure typed to return `Widget` — mir's opaque-callback
/// inference (mir 0.59) resolves the returned array's element type, so the
/// foreach value variable's members complete without any php-lsp-side
/// array_map handling.
#[tokio::test]
async fn completion_array_map_foreach_element_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Widget { public function render() {} }
$items = array_map(fn($x): Widget => $x, []);
foreach ($items as $item) { $item->$0 }
"#,
        )
        .await;
    expect!["Method      render"].assert_eq(&out);
}

/// `clone($obj, [...])` (PHP 8.5 clone-with) preserves the object's type —
/// mir resolves this directly, no php-lsp-side handling needed.
#[tokio::test]
async fn completion_clone_with_member() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Box { public function open() {} }
$b = new Box();
$c = clone($b, ['x' => 1]);
$c->$0
"#,
        )
        .await;
    expect!["Method      open"].assert_eq(&out);
}

/// A `use`-captured variable inside a closure body keeps the outer scope's
/// mir-resolved type.
#[tokio::test]
async fn completion_closure_use_var_member() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class PaymentService { public function process() {} }
$svc = new PaymentService();
$fn = function() use ($svc) { $svc->$0 };
"#,
        )
        .await;
    expect!["Method      process"].assert_eq(&out);
}

#[tokio::test]
async fn completion_constructor_chain_arrow() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Builder { public function build() {} public function reset() {} }
(new Builder())->$0
"#,
        )
        .await;
    expect![[r#"
        Method      build
        Method      reset"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_nullsafe_arrow() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Service { public function run() {} public string $status = ''; }
$s = new Service(); $s?->$0
"#,
        )
        .await;
    expect![[r#"
        Property    $status
        Method      run"#]]
    .assert_eq(&out);
}

// === Named argument completions ===

#[tokio::test]
async fn completion_named_argument_after_open_paren() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
function connect(string $host, int $port): void {}
connect($0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);
    let resp = s
        .client()
        .request(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": c.line, "character": c.character },
                "context": { "triggerKind": 2, "triggerCharacter": "(" },
            }),
        )
        .await;
    let out = render_completion(&resp);
    expect![[r#"
        Variable    host:
        Variable    port:"#]]
    .assert_eq(&out);
}

// === Insert-text format (snippet vs plain call) ===

#[tokio::test]
async fn completion_function_with_params_gets_snippet() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
function process(string $input): void {}
pro$0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let process_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("process"))
        .expect("process function not in completions");
    assert_eq!(
        process_item["insertTextFormat"].as_u64(),
        Some(2),
        "function with params must have SNIPPET format"
    );
    assert_eq!(
        process_item["insertText"].as_str(),
        Some("process($1)"),
        "snippet text must have placeholder"
    );
}

#[tokio::test]
async fn completion_function_without_params_plain_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
function doThing(): void {}
doT$0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("doThing"))
        .expect("doThing not in completions");
    assert_eq!(
        item["insertText"].as_str(),
        Some("doThing()"),
        "zero-param function must have plain call"
    );
    assert_ne!(
        item["insertTextFormat"].as_u64(),
        Some(2),
        "zero-param function must not be snippet"
    );
}

/// Regression test for a confirmed bug: cross-file member completion (routed
/// through the workspace index, unlike same-file completion) always
/// snippeted `($1)` regardless of whether the method actually took
/// arguments, because `ClassMembers` didn't track parameter presence at all
/// — the has_params bit passed to `callable_item` was hardcoded `true`.
#[tokio::test]
async fn completion_cross_file_static_method_snippet_matches_param_count() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Reg.php"),
        "<?php\nnamespace App;\nclass Reg {\n    public static function reset(): void {}\n    public static function set(string $key): void {}\n}\n",
    )
    .unwrap();
    let caller = "<?php\nnamespace App;\nReg::reset();\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "reset();", 0);
    let resp = s.completion("caller.php", line, ch).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let reset_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("reset"))
        .expect("reset not in completions");
    assert_eq!(
        reset_item["insertText"].as_str(),
        Some("reset()"),
        "zero-param cross-file method must have plain call, got {reset_item:?}"
    );
    let set_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("set"))
        .expect("set not in completions");
    assert_eq!(
        set_item["insertText"].as_str(),
        Some("set($1)"),
        "cross-file method with a param must have a snippet placeholder, got {set_item:?}"
    );
}

// === Use auto-import additionalTextEdits ===

#[tokio::test]
async fn completion_cross_file_class_adds_use_import() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"//- /main.php
<?php
namespace App;
$x = new $0

//- /lib/Mailer.php
<?php
namespace Lib;
class Mailer {}
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let mailer_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("Mailer"))
        .expect("Mailer class not in completions");
    let out = render_text_edits(&json!({ "result": mailer_item["additionalTextEdits"] }));
    expect![[r#"2:0-2:0 → "use Lib\\Mailer;\\n""#]].assert_eq(&out);
}

#[tokio::test]
async fn completion_same_namespace_no_use_import() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"//- /main.php
<?php
namespace Lib;
$x = new $0

//- /Mailer.php
<?php
namespace Lib;
class Mailer {}
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let mailer_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("Mailer"))
        .expect("Mailer must be in completions");
    let additional_edits = &mailer_item["additionalTextEdits"];
    assert!(
        additional_edits.is_null()
            || additional_edits
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true),
        "same-namespace class must not get use edit, got: {additional_edits}"
    );
}

// === Readonly property detail ===

#[tokio::test]
async fn completion_readonly_property_shows_detail() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
class Config { public readonly string $name = ''; }
$c = new Config(); $c->$0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let name_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("$name") || i["label"].as_str() == Some("name"))
        .expect("$name property must be in completions");
    assert_eq!(
        name_item["detail"].as_str(),
        Some("readonly"),
        "readonly property must have detail"
    );
}

/// A promoted constructor param that is NOT `readonly` but whose default
/// value's text happens to contain the word "readonly" must not be shown
/// as readonly — the check must use the param's actual `is_readonly` flag,
/// not a substring scan over its raw source span.
#[tokio::test]
async fn completion_non_readonly_property_with_readonly_in_default_value() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
class Config {
    public function __construct(public string $mode = 'readonly') {}
}
$c = new Config(); $c->$0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let mode_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("$mode") || i["label"].as_str() == Some("mode"))
        .expect("$mode property must be in completions");
    assert_eq!(
        mode_item["detail"].as_str(),
        None,
        "non-readonly property must not be labeled readonly just because its \
         default value contains that word"
    );
}

/// A `readonly class` (PHP 8.2+) makes every property readonly even
/// without a per-property `readonly` keyword — completion must still show
/// the "readonly" detail for a plain property declared inside one.
#[tokio::test]
async fn completion_plain_property_in_readonly_class_shows_detail() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
readonly class Config { public string $name; }
$c = new Config(); $c->$0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let name_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("$name") || i["label"].as_str() == Some("name"))
        .expect("$name property must be in completions");
    assert_eq!(
        name_item["detail"].as_str(),
        Some("readonly"),
        "plain property in a readonly class must have readonly detail"
    );
}

// === Variable cursor-line scoping ===

#[tokio::test]
async fn completion_variable_after_cursor_excluded() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
$early = 1;
$0
$late = 2;
"#,
        )
        .await;
    expect![[r#"
        Variable    $GLOBALS | superglobal
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Variable    $early
        Constant    __CLASS__ | Current class name
        Constant    __DIR__ | Directory of the current file
        Constant    __FILE__ | Absolute path of the current file
        Constant    __FUNCTION__ | Current function name
        Constant    __LINE__ | Current line number
        Constant    __METHOD__ | Current method name (Class::method)
        Constant    __NAMESPACE__ | Current namespace
        Constant    __TRAIT__ | Current trait name
        Function    abs
        Keyword     abstract
        Function    acos
        Function    addslashes
        Keyword     and
        Keyword     array
        Function    array_chunk
        Function    array_combine
        Function    array_diff
        Function    array_fill
        Function    array_fill_keys
        Function    array_filter
        Function    array_flip
        Function    array_intersect
        Function    array_key_exists
        Function    array_keys
        Function    array_map
        Function    array_merge
        Function    array_pad
        Function    array_pop
        Function    array_push
        Function    array_reduce
        Function    array_replace
        Function    array_reverse
        Function    array_search
        Function    array_shift
        Function    array_slice
        Function    array_splice
        Function    array_unique
        Function    array_unshift
        Function    array_values
        Function    array_walk
        Function    array_walk_recursive
        Function    arsort
        Keyword     as
        Function    asin
        Function    asort
        Function    atan
        Function    atan2
        Function    base64_decode
        Function    base64_encode
        Function    basename
        Keyword     bool
        Function    boolval
        Keyword     break
        Function    call_user_func
        Function    call_user_func_array
        Keyword     callable
        Keyword     case
        Keyword     catch
        Function    ceil
        Function    checkdate
        Keyword     class
        Function    class_exists
        Keyword     clone
        Function    closedir
        Function    compact
        Keyword     const
        Function    constant
        Keyword     continue
        Function    copy
        Function    cos
        Function    count
        Function    date
        Function    date_add
        Function    date_create
        Function    date_diff
        Function    date_format
        Function    date_sub
        Keyword     declare
        Keyword     default
        Function    define
        Function    defined
        Keyword     die
        Function    dirname
        Keyword     do
        Keyword     echo
        Keyword     else
        Keyword     elseif
        Keyword     empty
        Keyword     enddeclare
        Keyword     endfor
        Keyword     endforeach
        Keyword     endif
        Keyword     endswitch
        Keyword     endwhile
        Keyword     enum
        Keyword     eval
        Keyword     exit
        Function    exp
        Function    explode
        Keyword     extends
        Function    extract
        Keyword     false
        Function    fclose
        Function    feof
        Function    fgets
        Function    file_exists
        Function    file_get_contents
        Function    file_put_contents
        Keyword     final
        Keyword     finally
        Keyword     float
        Function    floatval
        Function    floor
        Function    fmod
        Keyword     fn
        Function    fopen
        Keyword     for
        Keyword     foreach
        Function    fputs
        Function    fread
        Function    fseek
        Function    ftell
        Keyword     function
        Function    function_exists
        Function    fwrite
        Function    get_class
        Function    get_parent_class
        Function    gettype
        Function    glob
        Keyword     global
        Keyword     goto
        Function    hash
        Function    header
        Function    headers_sent
        Function    htmlentities
        Function    htmlspecialchars
        Function    http_build_query
        Keyword     if
        Keyword     implements
        Function    implode
        Function    in_array
        Keyword     include
        Keyword     include_once
        Keyword     instanceof
        Keyword     insteadof
        Keyword     int
        Function    intdiv
        Keyword     interface
        Function    interface_exists
        Function    intval
        Function    is_a
        Function    is_array
        Function    is_bool
        Function    is_callable
        Function    is_dir
        Function    is_double
        Function    is_file
        Function    is_finite
        Function    is_float
        Function    is_infinite
        Function    is_int
        Function    is_integer
        Function    is_long
        Function    is_nan
        Function    is_null
        Function    is_numeric
        Function    is_object
        Function    is_readable
        Function    is_string
        Function    is_subclass_of
        Function    is_writable
        Keyword     isset
        Keyword     iterable
        Function    join
        Function    json_decode
        Function    json_encode
        Function    krsort
        Function    ksort
        Function    lcfirst
        Keyword     list
        Function    log
        Function    ltrim
        Keyword     match
        Function    max
        Function    md5
        Function    method_exists
        Function    microtime
        Function    min
        Keyword     mixed
        Function    mkdir
        Function    mktime
        Function    mt_rand
        Keyword     namespace
        Keyword     never
        Keyword     new
        Function    nl2br
        Keyword     null
        Function    number_format
        Function    ob_end_clean
        Function    ob_get_clean
        Function    ob_start
        Keyword     object
        Function    opendir
        Keyword     or
        Keyword     parent
        Function    parse_str
        Function    parse_url
        Function    pathinfo
        Function    pi
        Function    pow
        Function    preg_match
        Function    preg_match_all
        Function    preg_quote
        Function    preg_replace
        Function    preg_split
        Keyword     print
        Function    print_r
        Function    printf
        Keyword     private
        Function    property_exists
        Keyword     protected
        Keyword     public
        Function    rand
        Function    random_int
        Function    range
        Function    rawurldecode
        Function    rawurlencode
        Function    readdir
        Keyword     readonly
        Function    realpath
        Function    rename
        Keyword     require
        Keyword     require_once
        Keyword     return
        Function    rewind
        Function    rmdir
        Function    round
        Function    rsort
        Function    rtrim
        Function    scandir
        Keyword     self
        Function    serialize
        Function    session_destroy
        Function    session_start
        Function    setcookie
        Function    settype
        Function    sha1
        Function    sin
        Function    sleep
        Function    sort
        Function    sprintf
        Function    sqrt
        Keyword     static
        Function    str_contains
        Function    str_ends_with
        Function    str_pad
        Function    str_repeat
        Function    str_replace
        Function    str_split
        Function    str_starts_with
        Function    str_word_count
        Function    strcasecmp
        Function    strcmp
        Keyword     string
        Function    strip_tags
        Function    stripslashes
        Function    stristr
        Function    strlen
        Function    strncasecmp
        Function    strncmp
        Function    strpos
        Function    strrpos
        Function    strstr
        Function    strtolower
        Function    strtotime
        Function    strtoupper
        Function    strval
        Function    substr
        Function    substr_count
        Function    substr_replace
        Keyword     switch
        Function    tan
        Keyword     throw
        Function    time
        Keyword     trait
        Function    trim
        Keyword     true
        Keyword     try
        Function    uasort
        Function    ucfirst
        Function    ucwords
        Function    uksort
        Function    unlink
        Function    unserialize
        Function    unset
        Function    urldecode
        Function    urlencode
        Keyword     use
        Function    usleep
        Function    usort
        Keyword     var
        Function    var_dump
        Function    var_export
        Keyword     void
        Function    vsprintf
        Keyword     while
        Keyword     xor
        Keyword     yield"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_array_destructuring_variables_in_scope() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
[$first, $second] = ['a', 'b'];
$$0
"#,
        )
        .await;
    expect![[r#"
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Variable    $first
        Variable    $GLOBALS | superglobal
        Variable    $second"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_array_destructuring_after_cursor_excluded() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
$0
[$first] = ['a'];
"#,
        )
        .await;
    expect![[r#"
        Variable    $GLOBALS | superglobal
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Constant    __CLASS__ | Current class name
        Constant    __DIR__ | Directory of the current file
        Constant    __FILE__ | Absolute path of the current file
        Constant    __FUNCTION__ | Current function name
        Constant    __LINE__ | Current line number
        Constant    __METHOD__ | Current method name (Class::method)
        Constant    __NAMESPACE__ | Current namespace
        Constant    __TRAIT__ | Current trait name
        Function    abs
        Keyword     abstract
        Function    acos
        Function    addslashes
        Keyword     and
        Keyword     array
        Function    array_chunk
        Function    array_combine
        Function    array_diff
        Function    array_fill
        Function    array_fill_keys
        Function    array_filter
        Function    array_flip
        Function    array_intersect
        Function    array_key_exists
        Function    array_keys
        Function    array_map
        Function    array_merge
        Function    array_pad
        Function    array_pop
        Function    array_push
        Function    array_reduce
        Function    array_replace
        Function    array_reverse
        Function    array_search
        Function    array_shift
        Function    array_slice
        Function    array_splice
        Function    array_unique
        Function    array_unshift
        Function    array_values
        Function    array_walk
        Function    array_walk_recursive
        Function    arsort
        Keyword     as
        Function    asin
        Function    asort
        Function    atan
        Function    atan2
        Function    base64_decode
        Function    base64_encode
        Function    basename
        Keyword     bool
        Function    boolval
        Keyword     break
        Function    call_user_func
        Function    call_user_func_array
        Keyword     callable
        Keyword     case
        Keyword     catch
        Function    ceil
        Function    checkdate
        Keyword     class
        Function    class_exists
        Keyword     clone
        Function    closedir
        Function    compact
        Keyword     const
        Function    constant
        Keyword     continue
        Function    copy
        Function    cos
        Function    count
        Function    date
        Function    date_add
        Function    date_create
        Function    date_diff
        Function    date_format
        Function    date_sub
        Keyword     declare
        Keyword     default
        Function    define
        Function    defined
        Keyword     die
        Function    dirname
        Keyword     do
        Keyword     echo
        Keyword     else
        Keyword     elseif
        Keyword     empty
        Keyword     enddeclare
        Keyword     endfor
        Keyword     endforeach
        Keyword     endif
        Keyword     endswitch
        Keyword     endwhile
        Keyword     enum
        Keyword     eval
        Keyword     exit
        Function    exp
        Function    explode
        Keyword     extends
        Function    extract
        Keyword     false
        Function    fclose
        Function    feof
        Function    fgets
        Function    file_exists
        Function    file_get_contents
        Function    file_put_contents
        Keyword     final
        Keyword     finally
        Keyword     float
        Function    floatval
        Function    floor
        Function    fmod
        Keyword     fn
        Function    fopen
        Keyword     for
        Keyword     foreach
        Function    fputs
        Function    fread
        Function    fseek
        Function    ftell
        Keyword     function
        Function    function_exists
        Function    fwrite
        Function    get_class
        Function    get_parent_class
        Function    gettype
        Function    glob
        Keyword     global
        Keyword     goto
        Function    hash
        Function    header
        Function    headers_sent
        Function    htmlentities
        Function    htmlspecialchars
        Function    http_build_query
        Keyword     if
        Keyword     implements
        Function    implode
        Function    in_array
        Keyword     include
        Keyword     include_once
        Keyword     instanceof
        Keyword     insteadof
        Keyword     int
        Function    intdiv
        Keyword     interface
        Function    interface_exists
        Function    intval
        Function    is_a
        Function    is_array
        Function    is_bool
        Function    is_callable
        Function    is_dir
        Function    is_double
        Function    is_file
        Function    is_finite
        Function    is_float
        Function    is_infinite
        Function    is_int
        Function    is_integer
        Function    is_long
        Function    is_nan
        Function    is_null
        Function    is_numeric
        Function    is_object
        Function    is_readable
        Function    is_string
        Function    is_subclass_of
        Function    is_writable
        Keyword     isset
        Keyword     iterable
        Function    join
        Function    json_decode
        Function    json_encode
        Function    krsort
        Function    ksort
        Function    lcfirst
        Keyword     list
        Function    log
        Function    ltrim
        Keyword     match
        Function    max
        Function    md5
        Function    method_exists
        Function    microtime
        Function    min
        Keyword     mixed
        Function    mkdir
        Function    mktime
        Function    mt_rand
        Keyword     namespace
        Keyword     never
        Keyword     new
        Function    nl2br
        Keyword     null
        Function    number_format
        Function    ob_end_clean
        Function    ob_get_clean
        Function    ob_start
        Keyword     object
        Function    opendir
        Keyword     or
        Keyword     parent
        Function    parse_str
        Function    parse_url
        Function    pathinfo
        Function    pi
        Function    pow
        Function    preg_match
        Function    preg_match_all
        Function    preg_quote
        Function    preg_replace
        Function    preg_split
        Keyword     print
        Function    print_r
        Function    printf
        Keyword     private
        Function    property_exists
        Keyword     protected
        Keyword     public
        Function    rand
        Function    random_int
        Function    range
        Function    rawurldecode
        Function    rawurlencode
        Function    readdir
        Keyword     readonly
        Function    realpath
        Function    rename
        Keyword     require
        Keyword     require_once
        Keyword     return
        Function    rewind
        Function    rmdir
        Function    round
        Function    rsort
        Function    rtrim
        Function    scandir
        Keyword     self
        Function    serialize
        Function    session_destroy
        Function    session_start
        Function    setcookie
        Function    settype
        Function    sha1
        Function    sin
        Function    sleep
        Function    sort
        Function    sprintf
        Function    sqrt
        Keyword     static
        Function    str_contains
        Function    str_ends_with
        Function    str_pad
        Function    str_repeat
        Function    str_replace
        Function    str_split
        Function    str_starts_with
        Function    str_word_count
        Function    strcasecmp
        Function    strcmp
        Keyword     string
        Function    strip_tags
        Function    stripslashes
        Function    stristr
        Function    strlen
        Function    strncasecmp
        Function    strncmp
        Function    strpos
        Function    strrpos
        Function    strstr
        Function    strtolower
        Function    strtotime
        Function    strtoupper
        Function    strval
        Function    substr
        Function    substr_count
        Function    substr_replace
        Keyword     switch
        Function    tan
        Keyword     throw
        Function    time
        Keyword     trait
        Function    trim
        Keyword     true
        Keyword     try
        Function    uasort
        Function    ucfirst
        Function    ucwords
        Function    uksort
        Function    unlink
        Function    unserialize
        Function    unset
        Function    urldecode
        Function    urlencode
        Keyword     use
        Function    usleep
        Function    usort
        Keyword     var
        Function    var_dump
        Function    var_export
        Keyword     void
        Function    vsprintf
        Keyword     while
        Keyword     xor
        Keyword     yield"#]]
    .assert_eq(&out);
}

// === Match arm completions ===

#[tokio::test]
async fn completion_match_arm_suggests_enum_cases() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
enum Status { case Active; case Inactive; case Pending; }
$s = Status::Active;
match ($s) {
    $0
}
"#,
        )
        .await;
    expect![[r#"
        Variable    $GLOBALS | superglobal
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Variable    $s
        Enum        Status
        Constant    Status::Active
        Constant    Status::Inactive
        Constant    Status::Pending
        Constant    __CLASS__ | Current class name
        Constant    __DIR__ | Directory of the current file
        Constant    __FILE__ | Absolute path of the current file
        Constant    __FUNCTION__ | Current function name
        Constant    __LINE__ | Current line number
        Constant    __METHOD__ | Current method name (Class::method)
        Constant    __NAMESPACE__ | Current namespace
        Constant    __TRAIT__ | Current trait name
        Function    abs
        Keyword     abstract
        Function    acos
        Function    addslashes
        Keyword     and
        Keyword     array
        Function    array_chunk
        Function    array_combine
        Function    array_diff
        Function    array_fill
        Function    array_fill_keys
        Function    array_filter
        Function    array_flip
        Function    array_intersect
        Function    array_key_exists
        Function    array_keys
        Function    array_map
        Function    array_merge
        Function    array_pad
        Function    array_pop
        Function    array_push
        Function    array_reduce
        Function    array_replace
        Function    array_reverse
        Function    array_search
        Function    array_shift
        Function    array_slice
        Function    array_splice
        Function    array_unique
        Function    array_unshift
        Function    array_values
        Function    array_walk
        Function    array_walk_recursive
        Function    arsort
        Keyword     as
        Function    asin
        Function    asort
        Function    atan
        Function    atan2
        Function    base64_decode
        Function    base64_encode
        Function    basename
        Keyword     bool
        Function    boolval
        Keyword     break
        Function    call_user_func
        Function    call_user_func_array
        Keyword     callable
        Keyword     case
        Keyword     catch
        Function    ceil
        Function    checkdate
        Keyword     class
        Function    class_exists
        Keyword     clone
        Function    closedir
        Function    compact
        Keyword     const
        Function    constant
        Keyword     continue
        Function    copy
        Function    cos
        Function    count
        Function    date
        Function    date_add
        Function    date_create
        Function    date_diff
        Function    date_format
        Function    date_sub
        Keyword     declare
        Keyword     default
        Function    define
        Function    defined
        Keyword     die
        Function    dirname
        Keyword     do
        Keyword     echo
        Keyword     else
        Keyword     elseif
        Keyword     empty
        Keyword     enddeclare
        Keyword     endfor
        Keyword     endforeach
        Keyword     endif
        Keyword     endswitch
        Keyword     endwhile
        Keyword     enum
        Keyword     eval
        Keyword     exit
        Function    exp
        Function    explode
        Keyword     extends
        Function    extract
        Keyword     false
        Function    fclose
        Function    feof
        Function    fgets
        Function    file_exists
        Function    file_get_contents
        Function    file_put_contents
        Keyword     final
        Keyword     finally
        Keyword     float
        Function    floatval
        Function    floor
        Function    fmod
        Keyword     fn
        Function    fopen
        Keyword     for
        Keyword     foreach
        Function    fputs
        Function    fread
        Function    fseek
        Function    ftell
        Keyword     function
        Function    function_exists
        Function    fwrite
        Function    get_class
        Function    get_parent_class
        Function    gettype
        Function    glob
        Keyword     global
        Keyword     goto
        Function    hash
        Function    header
        Function    headers_sent
        Function    htmlentities
        Function    htmlspecialchars
        Function    http_build_query
        Keyword     if
        Keyword     implements
        Function    implode
        Function    in_array
        Keyword     include
        Keyword     include_once
        Keyword     instanceof
        Keyword     insteadof
        Keyword     int
        Function    intdiv
        Keyword     interface
        Function    interface_exists
        Function    intval
        Function    is_a
        Function    is_array
        Function    is_bool
        Function    is_callable
        Function    is_dir
        Function    is_double
        Function    is_file
        Function    is_finite
        Function    is_float
        Function    is_infinite
        Function    is_int
        Function    is_integer
        Function    is_long
        Function    is_nan
        Function    is_null
        Function    is_numeric
        Function    is_object
        Function    is_readable
        Function    is_string
        Function    is_subclass_of
        Function    is_writable
        Keyword     isset
        Keyword     iterable
        Function    join
        Function    json_decode
        Function    json_encode
        Function    krsort
        Function    ksort
        Function    lcfirst
        Keyword     list
        Function    log
        Function    ltrim
        Keyword     match
        Function    max
        Function    md5
        Function    method_exists
        Function    microtime
        Function    min
        Keyword     mixed
        Function    mkdir
        Function    mktime
        Function    mt_rand
        Keyword     namespace
        Keyword     never
        Keyword     new
        Function    nl2br
        Keyword     null
        Function    number_format
        Function    ob_end_clean
        Function    ob_get_clean
        Function    ob_start
        Keyword     object
        Function    opendir
        Keyword     or
        Keyword     parent
        Function    parse_str
        Function    parse_url
        Function    pathinfo
        Function    pi
        Function    pow
        Function    preg_match
        Function    preg_match_all
        Function    preg_quote
        Function    preg_replace
        Function    preg_split
        Keyword     print
        Function    print_r
        Function    printf
        Keyword     private
        Function    property_exists
        Keyword     protected
        Keyword     public
        Function    rand
        Function    random_int
        Function    range
        Function    rawurldecode
        Function    rawurlencode
        Function    readdir
        Keyword     readonly
        Function    realpath
        Function    rename
        Keyword     require
        Keyword     require_once
        Keyword     return
        Function    rewind
        Function    rmdir
        Function    round
        Function    rsort
        Function    rtrim
        Function    scandir
        Keyword     self
        Function    serialize
        Function    session_destroy
        Function    session_start
        Function    setcookie
        Function    settype
        Function    sha1
        Function    sin
        Function    sleep
        Function    sort
        Function    sprintf
        Function    sqrt
        Keyword     static
        Function    str_contains
        Function    str_ends_with
        Function    str_pad
        Function    str_repeat
        Function    str_replace
        Function    str_split
        Function    str_starts_with
        Function    str_word_count
        Function    strcasecmp
        Function    strcmp
        Keyword     string
        Function    strip_tags
        Function    stripslashes
        Function    stristr
        Function    strlen
        Function    strncasecmp
        Function    strncmp
        Function    strpos
        Function    strrpos
        Function    strstr
        Function    strtolower
        Function    strtotime
        Function    strtoupper
        Function    strval
        Function    substr
        Function    substr_count
        Function    substr_replace
        Keyword     switch
        Function    tan
        Keyword     throw
        Function    time
        Keyword     trait
        Function    trim
        Keyword     true
        Keyword     try
        Function    uasort
        Function    ucfirst
        Function    ucwords
        Function    uksort
        Function    unlink
        Function    unserialize
        Function    unset
        Function    urldecode
        Function    urlencode
        Keyword     use
        Function    usleep
        Function    usort
        Keyword     var
        Function    var_dump
        Function    var_export
        Keyword     void
        Function    vsprintf
        Keyword     while
        Keyword     xor
        Keyword     yield"#]]
    .assert_eq(&out);
}

// === Magic methods in class body ===

#[tokio::test]
async fn completion_magic_methods_in_class_body() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class App {
    $0
}
"#,
        )
        .await;
    expect![[r#"
        Variable    $GLOBALS | superglobal
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Class       App
        Constant    __CLASS__ | Current class name
        Constant    __DIR__ | Directory of the current file
        Constant    __FILE__ | Absolute path of the current file
        Constant    __FUNCTION__ | Current function name
        Constant    __LINE__ | Current line number
        Constant    __METHOD__ | Current method name (Class::method)
        Constant    __NAMESPACE__ | Current namespace
        Constant    __TRAIT__ | Current trait name
        Method      __call | magic method
        Method      __callStatic | magic method
        Method      __clone | magic method
        Method      __construct | magic method
        Method      __debugInfo | magic method
        Method      __destruct | magic method
        Method      __get | magic method
        Method      __invoke | magic method
        Method      __isset | magic method
        Method      __serialize | magic method
        Method      __set | magic method
        Method      __sleep | magic method
        Method      __toString | magic method
        Method      __unserialize | magic method
        Method      __unset | magic method
        Method      __wakeup | magic method
        Function    abs
        Keyword     abstract
        Function    acos
        Function    addslashes
        Keyword     and
        Keyword     array
        Function    array_chunk
        Function    array_combine
        Function    array_diff
        Function    array_fill
        Function    array_fill_keys
        Function    array_filter
        Function    array_flip
        Function    array_intersect
        Function    array_key_exists
        Function    array_keys
        Function    array_map
        Function    array_merge
        Function    array_pad
        Function    array_pop
        Function    array_push
        Function    array_reduce
        Function    array_replace
        Function    array_reverse
        Function    array_search
        Function    array_shift
        Function    array_slice
        Function    array_splice
        Function    array_unique
        Function    array_unshift
        Function    array_values
        Function    array_walk
        Function    array_walk_recursive
        Function    arsort
        Keyword     as
        Function    asin
        Function    asort
        Function    atan
        Function    atan2
        Function    base64_decode
        Function    base64_encode
        Function    basename
        Keyword     bool
        Function    boolval
        Keyword     break
        Function    call_user_func
        Function    call_user_func_array
        Keyword     callable
        Keyword     case
        Keyword     catch
        Function    ceil
        Function    checkdate
        Keyword     class
        Function    class_exists
        Keyword     clone
        Function    closedir
        Function    compact
        Keyword     const
        Function    constant
        Keyword     continue
        Function    copy
        Function    cos
        Function    count
        Function    date
        Function    date_add
        Function    date_create
        Function    date_diff
        Function    date_format
        Function    date_sub
        Keyword     declare
        Keyword     default
        Function    define
        Function    defined
        Keyword     die
        Function    dirname
        Keyword     do
        Keyword     echo
        Keyword     else
        Keyword     elseif
        Keyword     empty
        Keyword     enddeclare
        Keyword     endfor
        Keyword     endforeach
        Keyword     endif
        Keyword     endswitch
        Keyword     endwhile
        Keyword     enum
        Keyword     eval
        Keyword     exit
        Function    exp
        Function    explode
        Keyword     extends
        Function    extract
        Keyword     false
        Function    fclose
        Function    feof
        Function    fgets
        Function    file_exists
        Function    file_get_contents
        Function    file_put_contents
        Keyword     final
        Keyword     finally
        Keyword     float
        Function    floatval
        Function    floor
        Function    fmod
        Keyword     fn
        Function    fopen
        Keyword     for
        Keyword     foreach
        Function    fputs
        Function    fread
        Function    fseek
        Function    ftell
        Keyword     function
        Function    function_exists
        Function    fwrite
        Function    get_class
        Function    get_parent_class
        Function    gettype
        Function    glob
        Keyword     global
        Keyword     goto
        Function    hash
        Function    header
        Function    headers_sent
        Function    htmlentities
        Function    htmlspecialchars
        Function    http_build_query
        Keyword     if
        Keyword     implements
        Function    implode
        Function    in_array
        Keyword     include
        Keyword     include_once
        Keyword     instanceof
        Keyword     insteadof
        Keyword     int
        Function    intdiv
        Keyword     interface
        Function    interface_exists
        Function    intval
        Function    is_a
        Function    is_array
        Function    is_bool
        Function    is_callable
        Function    is_dir
        Function    is_double
        Function    is_file
        Function    is_finite
        Function    is_float
        Function    is_infinite
        Function    is_int
        Function    is_integer
        Function    is_long
        Function    is_nan
        Function    is_null
        Function    is_numeric
        Function    is_object
        Function    is_readable
        Function    is_string
        Function    is_subclass_of
        Function    is_writable
        Keyword     isset
        Keyword     iterable
        Function    join
        Function    json_decode
        Function    json_encode
        Function    krsort
        Function    ksort
        Function    lcfirst
        Keyword     list
        Function    log
        Function    ltrim
        Keyword     match
        Function    max
        Function    md5
        Function    method_exists
        Function    microtime
        Function    min
        Keyword     mixed
        Function    mkdir
        Function    mktime
        Function    mt_rand
        Keyword     namespace
        Keyword     never
        Keyword     new
        Function    nl2br
        Keyword     null
        Function    number_format
        Function    ob_end_clean
        Function    ob_get_clean
        Function    ob_start
        Keyword     object
        Function    opendir
        Keyword     or
        Keyword     parent
        Function    parse_str
        Function    parse_url
        Function    pathinfo
        Function    pi
        Function    pow
        Function    preg_match
        Function    preg_match_all
        Function    preg_quote
        Function    preg_replace
        Function    preg_split
        Keyword     print
        Function    print_r
        Function    printf
        Keyword     private
        Function    property_exists
        Keyword     protected
        Keyword     public
        Function    rand
        Function    random_int
        Function    range
        Function    rawurldecode
        Function    rawurlencode
        Function    readdir
        Keyword     readonly
        Function    realpath
        Function    rename
        Keyword     require
        Keyword     require_once
        Keyword     return
        Function    rewind
        Function    rmdir
        Function    round
        Function    rsort
        Function    rtrim
        Function    scandir
        Keyword     self
        Function    serialize
        Function    session_destroy
        Function    session_start
        Function    setcookie
        Function    settype
        Function    sha1
        Function    sin
        Function    sleep
        Function    sort
        Function    sprintf
        Function    sqrt
        Keyword     static
        Function    str_contains
        Function    str_ends_with
        Function    str_pad
        Function    str_repeat
        Function    str_replace
        Function    str_split
        Function    str_starts_with
        Function    str_word_count
        Function    strcasecmp
        Function    strcmp
        Keyword     string
        Function    strip_tags
        Function    stripslashes
        Function    stristr
        Function    strlen
        Function    strncasecmp
        Function    strncmp
        Function    strpos
        Function    strrpos
        Function    strstr
        Function    strtolower
        Function    strtotime
        Function    strtoupper
        Function    strval
        Function    substr
        Function    substr_count
        Function    substr_replace
        Keyword     switch
        Function    tan
        Keyword     throw
        Function    time
        Keyword     trait
        Function    trim
        Keyword     true
        Keyword     try
        Function    uasort
        Function    ucfirst
        Function    ucwords
        Function    uksort
        Function    unlink
        Function    unserialize
        Function    unset
        Function    urldecode
        Function    urlencode
        Keyword     use
        Function    usleep
        Function    usort
        Keyword     var
        Function    var_dump
        Function    var_export
        Keyword     void
        Function    vsprintf
        Keyword     while
        Keyword     xor
        Keyword     yield"#]]
    .assert_eq(&out);
}

// === Union type completions ===

#[tokio::test]
async fn completion_union_type_param_both_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Foo { public function fooMethod() {} }
class Bar { public function barMethod() {} }
function process(Foo|Bar $x): void { $x->$0 }
"#,
        )
        .await;
    expect![[r#"
        Method      barMethod
        Method      fooMethod"#]]
    .assert_eq(&out);
}

// === Use statement FQN completions ===

#[tokio::test]
async fn completion_use_statement_fqn_suggestions() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /main.php
<?php
use $0

//- /App/Services/Mailer.php
<?php
namespace App\Services;
class Mailer {}
"#,
        )
        .await;
    expect!["Class       App\\Services\\Mailer"].assert_eq(&out);
}

/// `use function` must suggest functions from other files — and must NOT
/// fall into the class-name path (a bare name-only substring match against
/// "function App\Helpers\format" would previously match nothing at all).
#[tokio::test]
async fn completion_use_function_statement_suggestions() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /main.php
<?php
use function $0

//- /App/Helpers.php
<?php
namespace App;
function formatName() {}
"#,
        )
        .await;
    expect!["Function    App\\formatName"].assert_eq(&out);
}

/// `use const` must suggest top-level constants from other files, scoped
/// to the const namespace (never classes or functions).
#[tokio::test]
async fn completion_use_const_statement_suggestions() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /main.php
<?php
use const $0

//- /App/Constants.php
<?php
namespace App;
const MAX_RETRIES = 3;
"#,
        )
        .await;
    expect!["Constant    App\\MAX_RETRIES"].assert_eq(&out);
}

// === Include/require path completions ===

#[tokio::test]
async fn completion_include_path_lists_php_files() {
    use std::fs;
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("lib")).unwrap();
    fs::write(tmp.path().join("lib/Helper.php"), "<?php").unwrap();
    fs::write(tmp.path().join("lib/Utils.php"), "<?php").unwrap();
    fs::write(tmp.path().join("lib/README.md"), "# readme").unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    let out = s.check_completion_ordered("<?php require './lib/$0").await;
    // Helper.php and Utils.php must appear; README.md must NOT
    expect![[r#"
        File        Helper.php
        File        Utils.php"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_include_path_insert_text_includes_prefix() {
    use std::fs;
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/Boot.php"), "<?php").unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    let opened = s.open_fixture("<?php require './src/$0").await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let boot_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("Boot.php"))
        .unwrap_or_else(|| {
            panic!(
                "Boot.php must be in completions for require './src/$0. Got items: {:#?}",
                items
            )
        });
    assert_eq!(
        boot_item["insertText"].as_str(),
        Some("./src/Boot.php"),
        "insertText for Boot.php must preserve path prefix './src/'. Got: {:?}",
        boot_item["insertText"]
    );
}

#[tokio::test]
async fn completion_include_path_nonexistent_dir_empty() {
    use std::fs;
    let tmp = tempfile::tempdir().unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    let out = s.check_completion("<?php require './no-such-dir/$0").await;
    assert_eq!(
        out, "<no completions>",
        "require './no-such-dir/$0 must return no completions (dir doesn't exist). Got: {out}"
    );
}

#[tokio::test]
async fn completion_include_path_folder_has_folder_kind() {
    use std::fs;
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("modules")).unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    let opened = s.open_fixture("<?php require '$0").await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let folder_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("modules") || i["label"].as_str() == Some("modules/"))
        .unwrap_or_else(|| {
            panic!(
                "require '$0 must include 'modules' folder. Got items: {:#?}",
                items
            )
        });
    assert_eq!(
        folder_item["kind"].as_u64(),
        Some(19),
        "modules folder must have kind FOLDER (19). Got kind: {:?}",
        folder_item["kind"]
    );
    let insert = folder_item["insertText"].as_str().unwrap_or("");
    assert!(
        insert.ends_with('/'),
        "modules folder insertText must end with '/'. Got: {insert:?}"
    );
}

/// Completion items must not contain duplicates — each label appears at most once.
#[tokio::test]
async fn completion_no_duplicates_in_list() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
enum Status { case Active; }
$s = Status::Active;
match ($s) {
    $0
}
"#,
        )
        .await;
    expect![[r#"
        Variable    $GLOBALS | superglobal
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Variable    $s
        Enum        Status
        Constant    Status::Active
        Constant    __CLASS__ | Current class name
        Constant    __DIR__ | Directory of the current file
        Constant    __FILE__ | Absolute path of the current file
        Constant    __FUNCTION__ | Current function name
        Constant    __LINE__ | Current line number
        Constant    __METHOD__ | Current method name (Class::method)
        Constant    __NAMESPACE__ | Current namespace
        Constant    __TRAIT__ | Current trait name
        Function    abs
        Keyword     abstract
        Function    acos
        Function    addslashes
        Keyword     and
        Keyword     array
        Function    array_chunk
        Function    array_combine
        Function    array_diff
        Function    array_fill
        Function    array_fill_keys
        Function    array_filter
        Function    array_flip
        Function    array_intersect
        Function    array_key_exists
        Function    array_keys
        Function    array_map
        Function    array_merge
        Function    array_pad
        Function    array_pop
        Function    array_push
        Function    array_reduce
        Function    array_replace
        Function    array_reverse
        Function    array_search
        Function    array_shift
        Function    array_slice
        Function    array_splice
        Function    array_unique
        Function    array_unshift
        Function    array_values
        Function    array_walk
        Function    array_walk_recursive
        Function    arsort
        Keyword     as
        Function    asin
        Function    asort
        Function    atan
        Function    atan2
        Function    base64_decode
        Function    base64_encode
        Function    basename
        Keyword     bool
        Function    boolval
        Keyword     break
        Function    call_user_func
        Function    call_user_func_array
        Keyword     callable
        Keyword     case
        Keyword     catch
        Function    ceil
        Function    checkdate
        Keyword     class
        Function    class_exists
        Keyword     clone
        Function    closedir
        Function    compact
        Keyword     const
        Function    constant
        Keyword     continue
        Function    copy
        Function    cos
        Function    count
        Function    date
        Function    date_add
        Function    date_create
        Function    date_diff
        Function    date_format
        Function    date_sub
        Keyword     declare
        Keyword     default
        Function    define
        Function    defined
        Keyword     die
        Function    dirname
        Keyword     do
        Keyword     echo
        Keyword     else
        Keyword     elseif
        Keyword     empty
        Keyword     enddeclare
        Keyword     endfor
        Keyword     endforeach
        Keyword     endif
        Keyword     endswitch
        Keyword     endwhile
        Keyword     enum
        Keyword     eval
        Keyword     exit
        Function    exp
        Function    explode
        Keyword     extends
        Function    extract
        Keyword     false
        Function    fclose
        Function    feof
        Function    fgets
        Function    file_exists
        Function    file_get_contents
        Function    file_put_contents
        Keyword     final
        Keyword     finally
        Keyword     float
        Function    floatval
        Function    floor
        Function    fmod
        Keyword     fn
        Function    fopen
        Keyword     for
        Keyword     foreach
        Function    fputs
        Function    fread
        Function    fseek
        Function    ftell
        Keyword     function
        Function    function_exists
        Function    fwrite
        Function    get_class
        Function    get_parent_class
        Function    gettype
        Function    glob
        Keyword     global
        Keyword     goto
        Function    hash
        Function    header
        Function    headers_sent
        Function    htmlentities
        Function    htmlspecialchars
        Function    http_build_query
        Keyword     if
        Keyword     implements
        Function    implode
        Function    in_array
        Keyword     include
        Keyword     include_once
        Keyword     instanceof
        Keyword     insteadof
        Keyword     int
        Function    intdiv
        Keyword     interface
        Function    interface_exists
        Function    intval
        Function    is_a
        Function    is_array
        Function    is_bool
        Function    is_callable
        Function    is_dir
        Function    is_double
        Function    is_file
        Function    is_finite
        Function    is_float
        Function    is_infinite
        Function    is_int
        Function    is_integer
        Function    is_long
        Function    is_nan
        Function    is_null
        Function    is_numeric
        Function    is_object
        Function    is_readable
        Function    is_string
        Function    is_subclass_of
        Function    is_writable
        Keyword     isset
        Keyword     iterable
        Function    join
        Function    json_decode
        Function    json_encode
        Function    krsort
        Function    ksort
        Function    lcfirst
        Keyword     list
        Function    log
        Function    ltrim
        Keyword     match
        Function    max
        Function    md5
        Function    method_exists
        Function    microtime
        Function    min
        Keyword     mixed
        Function    mkdir
        Function    mktime
        Function    mt_rand
        Keyword     namespace
        Keyword     never
        Keyword     new
        Function    nl2br
        Keyword     null
        Function    number_format
        Function    ob_end_clean
        Function    ob_get_clean
        Function    ob_start
        Keyword     object
        Function    opendir
        Keyword     or
        Keyword     parent
        Function    parse_str
        Function    parse_url
        Function    pathinfo
        Function    pi
        Function    pow
        Function    preg_match
        Function    preg_match_all
        Function    preg_quote
        Function    preg_replace
        Function    preg_split
        Keyword     print
        Function    print_r
        Function    printf
        Keyword     private
        Function    property_exists
        Keyword     protected
        Keyword     public
        Function    rand
        Function    random_int
        Function    range
        Function    rawurldecode
        Function    rawurlencode
        Function    readdir
        Keyword     readonly
        Function    realpath
        Function    rename
        Keyword     require
        Keyword     require_once
        Keyword     return
        Function    rewind
        Function    rmdir
        Function    round
        Function    rsort
        Function    rtrim
        Function    scandir
        Keyword     self
        Function    serialize
        Function    session_destroy
        Function    session_start
        Function    setcookie
        Function    settype
        Function    sha1
        Function    sin
        Function    sleep
        Function    sort
        Function    sprintf
        Function    sqrt
        Keyword     static
        Function    str_contains
        Function    str_ends_with
        Function    str_pad
        Function    str_repeat
        Function    str_replace
        Function    str_split
        Function    str_starts_with
        Function    str_word_count
        Function    strcasecmp
        Function    strcmp
        Keyword     string
        Function    strip_tags
        Function    stripslashes
        Function    stristr
        Function    strlen
        Function    strncasecmp
        Function    strncmp
        Function    strpos
        Function    strrpos
        Function    strstr
        Function    strtolower
        Function    strtotime
        Function    strtoupper
        Function    strval
        Function    substr
        Function    substr_count
        Function    substr_replace
        Keyword     switch
        Function    tan
        Keyword     throw
        Function    time
        Keyword     trait
        Function    trim
        Keyword     true
        Keyword     try
        Function    uasort
        Function    ucfirst
        Function    ucwords
        Function    uksort
        Function    unlink
        Function    unserialize
        Function    unset
        Function    urldecode
        Function    urlencode
        Keyword     use
        Function    usleep
        Function    usort
        Keyword     var
        Function    var_dump
        Function    var_export
        Keyword     void
        Function    vsprintf
        Keyword     while
        Keyword     xor
        Keyword     yield"#]]
    .assert_eq(&out);
}

/// Completions inside string literals must be suppressed — there are no PHP
/// identifier completions that make sense inside `"..."` or `'...'`.
#[tokio::test]
async fn completion_in_string_literal_returns_empty() {
    let mut s = TestServer::new().await;
    // cursor inside double-quoted string
    let out = s
        .check_completion(
            r#"<?php
$x = "hell$0";
"#,
        )
        .await;
    assert_eq!(
        out, "<no completions>",
        "expected no completions inside string, got:\n{out}"
    );

    // cursor inside single-quoted string
    let out = s
        .check_completion(
            r#"<?php
$x = 'hell$0';
"#,
        )
        .await;
    assert_eq!(
        out, "<no completions>",
        "expected no completions inside single-quoted string, got:\n{out}"
    );
}

/// Completions inside comments must be suppressed.
#[tokio::test]
async fn completion_in_comment_returns_empty() {
    let mut s = TestServer::new().await;
    // cursor inside line comment
    let out = s
        .check_completion(
            r#"<?php
// hell$0
$x = 1;
"#,
        )
        .await;
    assert_eq!(
        out, "<no completions>",
        "expected no completions inside // comment, got:\n{out}"
    );

    // cursor inside block comment
    let out = s
        .check_completion(
            r#"<?php
/* hell$0 */
$x = 1;
"#,
        )
        .await;
    assert_eq!(
        out, "<no completions>",
        "expected no completions inside /* comment, got:\n{out}"
    );

    // cursor inside # comment
    let out = s
        .check_completion(
            r#"<?php
# hell$0
$x = 1;
"#,
        )
        .await;
    assert_eq!(
        out, "<no completions>",
        "expected no completions inside # comment, got:\n{out}"
    );
}

/// Instance method completions are available for class instances.
#[tokio::test]
async fn completion_instance_methods_are_available() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Helper {
    public function publicMethod() {}
    public function anotherMethod() {}
}
$h = new Helper();
$h->$0
"#,
        )
        .await;
    expect![[r#"
        Method      anotherMethod
        Method      publicMethod"#]]
    .assert_eq(&out);
}

/// Instance methods are available in instance context.
#[tokio::test]
async fn completion_static_methods_excluded_in_instance_context() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Utils {
    public static function staticHelper() {}
    public function instanceMethod() {}
}
$u = new Utils();
$u->$0
"#,
        )
        .await;
    expect![["Method      instanceMethod"]].assert_eq(&out);
}

/// Union types show methods from all member types.
#[tokio::test]
async fn completion_union_type_shows_all_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Foo { public function fooOnly() {} }
class Bar { public function barOnly() {} }
function test(Foo|Bar $x): void { $x->$0 }
"#,
        )
        .await;
    expect![[r#"
        Method      barOnly
        Method      fooOnly"#]]
    .assert_eq(&out);
}

/// Variables after cursor are excluded from scope.
#[tokio::test]
async fn completion_after_cursor_variable_excluded() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
$early = 1;
$$0
$late = 2;
"#,
        )
        .await;
    expect![[r#"
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Variable    $early
        Variable    $GLOBALS | superglobal"#]]
    .assert_eq(&out);
}

/// Include path completion works with relative paths.
#[tokio::test]
async fn completion_include_path_relative_directory() {
    use std::fs;
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("lib")).unwrap();
    fs::write(tmp.path().join("lib").join("Helper.php"), "<?php").unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);

    let out = s
        .check_completion_ordered(
            r#"<?php
require './lib/$0
"#,
        )
        .await;

    expect![["File        Helper.php"]].assert_eq(&out);
}

/// Nested class methods are available in completions.
#[tokio::test]
async fn completion_nested_class_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Outer {
    class Inner {
        public function innerMethod() {}
    }
}
$i = new Outer\Inner();
$i->$0
"#,
        )
        .await;
    expect![[r#"
        Variable    $GLOBALS | superglobal
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Variable    $i
        Class       Outer
        Constant    __CLASS__ | Current class name
        Constant    __DIR__ | Directory of the current file
        Constant    __FILE__ | Absolute path of the current file
        Constant    __FUNCTION__ | Current function name
        Constant    __LINE__ | Current line number
        Constant    __METHOD__ | Current method name (Class::method)
        Constant    __NAMESPACE__ | Current namespace
        Constant    __TRAIT__ | Current trait name
        Function    abs
        Keyword     abstract
        Function    acos
        Function    addslashes
        Keyword     and
        Keyword     array
        Function    array_chunk
        Function    array_combine
        Function    array_diff
        Function    array_fill
        Function    array_fill_keys
        Function    array_filter
        Function    array_flip
        Function    array_intersect
        Function    array_key_exists
        Function    array_keys
        Function    array_map
        Function    array_merge
        Function    array_pad
        Function    array_pop
        Function    array_push
        Function    array_reduce
        Function    array_replace
        Function    array_reverse
        Function    array_search
        Function    array_shift
        Function    array_slice
        Function    array_splice
        Function    array_unique
        Function    array_unshift
        Function    array_values
        Function    array_walk
        Function    array_walk_recursive
        Function    arsort
        Keyword     as
        Function    asin
        Function    asort
        Function    atan
        Function    atan2
        Function    base64_decode
        Function    base64_encode
        Function    basename
        Keyword     bool
        Function    boolval
        Keyword     break
        Function    call_user_func
        Function    call_user_func_array
        Keyword     callable
        Keyword     case
        Keyword     catch
        Function    ceil
        Function    checkdate
        Keyword     class
        Function    class_exists
        Keyword     clone
        Function    closedir
        Function    compact
        Keyword     const
        Function    constant
        Keyword     continue
        Function    copy
        Function    cos
        Function    count
        Function    date
        Function    date_add
        Function    date_create
        Function    date_diff
        Function    date_format
        Function    date_sub
        Keyword     declare
        Keyword     default
        Function    define
        Function    defined
        Keyword     die
        Function    dirname
        Keyword     do
        Keyword     echo
        Keyword     else
        Keyword     elseif
        Keyword     empty
        Keyword     enddeclare
        Keyword     endfor
        Keyword     endforeach
        Keyword     endif
        Keyword     endswitch
        Keyword     endwhile
        Keyword     enum
        Keyword     eval
        Keyword     exit
        Function    exp
        Function    explode
        Keyword     extends
        Function    extract
        Keyword     false
        Function    fclose
        Function    feof
        Function    fgets
        Function    file_exists
        Function    file_get_contents
        Function    file_put_contents
        Keyword     final
        Keyword     finally
        Keyword     float
        Function    floatval
        Function    floor
        Function    fmod
        Keyword     fn
        Function    fopen
        Keyword     for
        Keyword     foreach
        Function    fputs
        Function    fread
        Function    fseek
        Function    ftell
        Keyword     function
        Function    function_exists
        Function    fwrite
        Function    get_class
        Function    get_parent_class
        Function    gettype
        Function    glob
        Keyword     global
        Keyword     goto
        Function    hash
        Function    header
        Function    headers_sent
        Function    htmlentities
        Function    htmlspecialchars
        Function    http_build_query
        Keyword     if
        Keyword     implements
        Function    implode
        Function    in_array
        Keyword     include
        Keyword     include_once
        Method      innerMethod | function innerMethod()
        Keyword     instanceof
        Keyword     insteadof
        Keyword     int
        Function    intdiv
        Keyword     interface
        Function    interface_exists
        Function    intval
        Function    is_a
        Function    is_array
        Function    is_bool
        Function    is_callable
        Function    is_dir
        Function    is_double
        Function    is_file
        Function    is_finite
        Function    is_float
        Function    is_infinite
        Function    is_int
        Function    is_integer
        Function    is_long
        Function    is_nan
        Function    is_null
        Function    is_numeric
        Function    is_object
        Function    is_readable
        Function    is_string
        Function    is_subclass_of
        Function    is_writable
        Keyword     isset
        Keyword     iterable
        Function    join
        Function    json_decode
        Function    json_encode
        Function    krsort
        Function    ksort
        Function    lcfirst
        Keyword     list
        Function    log
        Function    ltrim
        Keyword     match
        Function    max
        Function    md5
        Function    method_exists
        Function    microtime
        Function    min
        Keyword     mixed
        Function    mkdir
        Function    mktime
        Function    mt_rand
        Keyword     namespace
        Keyword     never
        Keyword     new
        Function    nl2br
        Keyword     null
        Function    number_format
        Function    ob_end_clean
        Function    ob_get_clean
        Function    ob_start
        Keyword     object
        Function    opendir
        Keyword     or
        Keyword     parent
        Function    parse_str
        Function    parse_url
        Function    pathinfo
        Function    pi
        Function    pow
        Function    preg_match
        Function    preg_match_all
        Function    preg_quote
        Function    preg_replace
        Function    preg_split
        Keyword     print
        Function    print_r
        Function    printf
        Keyword     private
        Function    property_exists
        Keyword     protected
        Keyword     public
        Function    rand
        Function    random_int
        Function    range
        Function    rawurldecode
        Function    rawurlencode
        Function    readdir
        Keyword     readonly
        Function    realpath
        Function    rename
        Keyword     require
        Keyword     require_once
        Keyword     return
        Function    rewind
        Function    rmdir
        Function    round
        Function    rsort
        Function    rtrim
        Function    scandir
        Keyword     self
        Function    serialize
        Function    session_destroy
        Function    session_start
        Function    setcookie
        Function    settype
        Function    sha1
        Function    sin
        Function    sleep
        Function    sort
        Function    sprintf
        Function    sqrt
        Keyword     static
        Function    str_contains
        Function    str_ends_with
        Function    str_pad
        Function    str_repeat
        Function    str_replace
        Function    str_split
        Function    str_starts_with
        Function    str_word_count
        Function    strcasecmp
        Function    strcmp
        Keyword     string
        Function    strip_tags
        Function    stripslashes
        Function    stristr
        Function    strlen
        Function    strncasecmp
        Function    strncmp
        Function    strpos
        Function    strrpos
        Function    strstr
        Function    strtolower
        Function    strtotime
        Function    strtoupper
        Function    strval
        Function    substr
        Function    substr_count
        Function    substr_replace
        Keyword     switch
        Function    tan
        Keyword     throw
        Function    time
        Keyword     trait
        Function    trim
        Keyword     true
        Keyword     try
        Function    uasort
        Function    ucfirst
        Function    ucwords
        Function    uksort
        Function    unlink
        Function    unserialize
        Function    unset
        Function    urldecode
        Function    urlencode
        Keyword     use
        Function    usleep
        Function    usort
        Keyword     var
        Function    var_dump
        Function    var_export
        Keyword     void
        Function    vsprintf
        Keyword     while
        Keyword     xor
        Keyword     yield"#]]
    .assert_eq(&out);
}

// === Instance property insertText (bug: was inserting "$prop" via "->" giving "$obj->$prop") ===

#[tokio::test]
async fn completion_arrow_property_insert_text_has_no_dollar() {
    // Instance property completion after `->` must set insertText=name (no "$")
    // so that clients do not produce the invalid `$obj->$prop`.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
class Box {
    public function __construct(
        public string $label = '',
        public int $count = 0,
    ) {}
}
$b = new Box();
$b->$0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items: Vec<_> = match &resp["result"] {
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };

    for prop in ["label", "count"] {
        let item = items
            .iter()
            .find(|i| i["label"].as_str() == Some(&format!("${prop}")))
            .unwrap_or_else(|| panic!("${prop} not found in completions"));
        let insert = item["insertText"].as_str().unwrap_or_else(|| {
            panic!("${prop}: insertText must be set for instance properties; got null")
        });
        assert_eq!(
            insert, prop,
            "${prop}: insertText should be '{prop}' (no $), got '{insert}'"
        );
    }
}

#[tokio::test]
async fn completion_static_property_insert_text_keeps_dollar() {
    // Static property completion after `::` must keep the `$` in insertText
    // (or omit insertText entirely) because `Class::$prop` is valid PHP.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
class Registry {
    public static string $instance = '';
}
Registry::$0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items: Vec<_> = match &resp["result"] {
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };

    let item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("$instance"))
        .expect("$instance not found in static completions");

    // insertText must either be absent (client uses label "$instance") or explicitly "$instance"
    let insert = item["insertText"].as_str();
    assert!(
        insert.is_none() || insert == Some("$instance"),
        "static $instance insertText must keep '$'; got {insert:?}"
    );
}

// Variables declared in another file must not appear in completions for the current file.
// === Invoked completion (no trigger char) member access ===

/// `(new Foo())->$0` invoked without a trigger char must return Foo's members,
/// not fall back to the global symbol list.
#[tokio::test]
async fn completion_constructor_chain_invoked_no_trigger() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Repo { public function find(int $id): void {} public function save(): void {} }
(new Repo())->$0
"#,
        )
        .await;
    expect![[r#"
        Method      find
        Method      save"#]]
    .assert_eq(&out);
}

/// `$obj?->$0` invoked without a trigger char must return receiver members,
/// not the global default list.
#[tokio::test]
async fn completion_nullsafe_invoked_no_trigger() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Cache { public function get(string $key): mixed {} public function set(): void {} }
$c = new Cache();
$c?->$0
"#,
        )
        .await;
    expect![[r#"
        Method      get
        Method      set"#]]
    .assert_eq(&out);
}

/// Nullable type-hint: `?Foo $x` — completing `$x->` must show Foo's members even
/// though the declared type includes null.
#[tokio::test]
async fn completion_nullable_type_hint_shows_class_members() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Logger { public function debug(string $msg): void {} public string $level = ''; }
function process(?Logger $log): void {
    $log->$0
}
"#,
        )
        .await;
    expect![[r#"
        Property    $level
        Method      debug"#]]
    .assert_eq(&out);
}

/// Method chain: `$obj->getUser()->$0` resolves to the call's return type.
#[tokio::test]
async fn completion_method_chain_resolves_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class User { public string $name = ''; public function greet(): void {} }
class Service { public function getUser(): User { return new User(); } }
$svc = new Service();
$svc->getUser()->$0
"#,
        )
        .await;
    expect![[r#"
        Property    $name
        Method      greet"#]]
    .assert_eq(&out);
}

/// PHP variables are file-scoped — they are never part of the cross-file symbol index.
#[tokio::test]
async fn completion_cross_file_variables_not_leaked() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /other.php
<?php
$remoteVar = 42;

//- /main.php
<?php
$remote$0
"#,
        )
        .await;
    // $remoteVar from other.php must not appear — variables are file-scoped.
    expect!["<no completions>"].assert_eq(&out);
}

/// Trigger-character `>` (after `->`) must return ONLY instance members of the
/// resolved receiver class, not the full keyword/builtin list.
#[tokio::test]
async fn completion_arrow_trigger_char_returns_instance_members_only() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
class Printer {
    public function print(): void {}
    public string $output = '';
}
$p = new Printer();
$p->$0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);
    let resp = s
        .client()
        .request(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": c.line, "character": c.character },
                "context": { "triggerKind": 2, "triggerCharacter": ">" },
            }),
        )
        .await;
    let out = render_completion(&resp);
    // Only the instance members of Printer must appear — no keywords or builtins.
    expect![[r#"
        Property    $output
        Method      print"#]]
    .assert_eq(&out);
}

/// Trigger-character `:` (after `::`) must return ONLY the static members of
/// the resolved class — no keywords or builtins leak through.
#[tokio::test]
async fn completion_static_trigger_char_returns_static_members_only() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
class Store {
    public static function save(): void {}
    public static int $count = 0;
    const VERSION = '1.0';
}
Store::$0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);
    let resp = s
        .client()
        .request(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": c.line, "character": c.character },
                "context": { "triggerKind": 2, "triggerCharacter": ":" },
            }),
        )
        .await;
    let out = render_completion_ordered(&resp);
    // Only static members of Store: static method, static property, constant.
    // Keywords and builtins must NOT appear.
    // render_completion_ordered sorts by label; `$count` < `VERSION` < `save` (ASCII order).
    expect![[r#"
        Property    $count
        Constant    VERSION
        Method      save"#]]
    .assert_eq(&out);
}

/// Trigger-character `$` must return ONLY superglobals and local variables —
/// not keywords or builtins.
#[tokio::test]
async fn completion_dollar_trigger_char_returns_variables_only() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
function process(string $input): void {
    $result = '';
    $$0
}
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);
    let resp = s
        .client()
        .request(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": c.line, "character": c.character },
                "context": { "triggerKind": 2, "triggerCharacter": "$" },
            }),
        )
        .await;
    let out = render_completion(&resp);
    // Must contain superglobals and function parameters, no keywords/builtins.
    // Note: body-local variables (e.g. `$result = ''`) are not scanned by the
    // trigger-$ path — only params and superglobals are returned.
    expect![[r#"
        Variable    $GLOBALS
        Variable    $_COOKIE
        Variable    $_ENV
        Variable    $_FILES
        Variable    $_GET
        Variable    $_POST
        Variable    $_REQUEST
        Variable    $_SERVER
        Variable    $_SESSION
        Variable    $input"#]]
    .assert_eq(&out);
}

/// Trigger-character `(` must return named-argument labels when cursor is
/// immediately after the opening paren of a function call.
#[tokio::test]
async fn completion_open_paren_trigger_char_returns_named_args() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
function createUser(string $name, int $age, bool $active): void {}
createUser($0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);
    let resp = s
        .client()
        .request(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": c.line, "character": c.character },
                "context": { "triggerKind": 2, "triggerCharacter": "(" },
            }),
        )
        .await;
    let out = render_completion(&resp);
    // All three parameter names must appear as named-argument completions.
    // PHP named args use `name:` syntax (no `$` prefix).
    // render_completion sorts by label (no sortText set), so alphabetical order.
    expect![[r#"
        Variable    active:
        Variable    age:
        Variable    name:"#]]
    .assert_eq(&out);
}

/// Named-argument completion after `$obj->method(` must be scoped to the
/// receiver's own class, not to whichever same-named method the naive
/// text scan finds first in the workspace. `Logger::send` and
/// `Mailer::send` have different parameter lists; completing on a
/// `Mailer`-typed receiver must offer `Mailer`'s params, never `Logger`'s.
#[tokio::test]
async fn completion_named_argument_scoped_to_receiver_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
class Logger {
    public function send(string $level, string $message): void {}
}
class Mailer {
    public function send(string $to, string $subject, string $body): void {}
}
function notify(Mailer $mailer): void {
    $mailer->send($0
}
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);
    let resp = s
        .client()
        .request(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": c.line, "character": c.character },
                "context": { "triggerKind": 2, "triggerCharacter": "(" },
            }),
        )
        .await;
    let out = render_completion(&resp);
    expect![[r#"
        Variable    body:
        Variable    subject:
        Variable    to:"#]]
    .assert_eq(&out);
}

/// Same as above but for a static call (`Class::method(`) — the receiver
/// class name comes from before `::`, not from mir's variable-type
/// tracking, but must still scope the lookup instead of matching whichever
/// same-named method is found first.
#[tokio::test]
async fn completion_named_argument_scoped_to_static_receiver_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
class Logger {
    public static function send(string $level, string $message): void {}
}
class Mailer {
    public static function send(string $to, string $subject, string $body): void {}
}
Mailer::send($0
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);
    let resp = s
        .client()
        .request(
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": c.line, "character": c.character },
                "context": { "triggerKind": 2, "triggerCharacter": "(" },
            }),
        )
        .await;
    let out = render_completion(&resp);
    expect![[r#"
        Variable    body:
        Variable    subject:
        Variable    to:"#]]
    .assert_eq(&out);
}

// ── member/static completion via the workspace index ─────────────────────────

/// Member completion when the class is index-only, global namespace.
#[tokio::test]
async fn completion_member_from_index_global_namespace() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Logger.php"),
        "<?php\nclass Logger {\n    public function debug(): void {}\n    public function info(): void {}\n}\n",
    )
    .unwrap();
    let caller = "<?php\n$log = new Logger();\n$log->debug();\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "debug();", 0);
    let resp = s.completion("caller.php", line, ch).await;
    let out = render_completion_ordered(&resp);
    expect![[r#"
        Method      debug
        Method      info"#]]
    .assert_eq(&out);
}

/// Member completion when the class is index-only inside `namespace App;`.
#[tokio::test]
async fn completion_member_from_index_namespaced() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Logger.php"),
        "<?php\nnamespace App;\nclass Logger {\n    public function debug(): void {}\n    public function info(): void {}\n}\n",
    )
    .unwrap();
    let caller = "<?php\nnamespace App;\n$log = new Logger();\n$log->debug();\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "debug();", 0);
    let resp = s.completion("caller.php", line, ch).await;
    let out = render_completion_ordered(&resp);
    expect![[r#"
        Method      debug
        Method      info"#]]
    .assert_eq(&out);
}

/// Static completion when the class is index-only inside `namespace App;`.
#[tokio::test]
async fn completion_static_from_index_namespaced() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Reg.php"),
        "<?php\nnamespace App;\nclass Reg {\n    public static function get(): void {}\n    public static function set(): void {}\n}\n",
    )
    .unwrap();
    let caller = "<?php\nnamespace App;\nReg::get();\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "get();", 0);
    let resp = s.completion("caller.php", line, ch).await;
    let out = render_completion_ordered(&resp);
    expect![[r#"
        Method      get
        Method      set"#]]
    .assert_eq(&out);
}

// ── index-only (unopened) classes referenced by bare short name ──────────────

/// Facade-style static member completion works when the class is index-only
/// (never opened), in a different namespace than the caller, and unimported
/// — methods are declared via `@method static` docblock tags, the pattern
/// Laravel facades use.
#[tokio::test]
async fn completion_static_facade_methods_from_unopened_cross_namespace_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Http.php"),
        "<?php\n\nnamespace Acme\\Support\\Facades;\n\n/**\n * @method static \\Acme\\Http\\Client\\Response get(string $url, $query = null)\n * @method static \\Acme\\Http\\Client\\Response post(string $url, array $data = [])\n */\nclass Http\n{\n}\n",
    )
    .unwrap();
    let caller = "<?php\n$r = Http::get('x');\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "get('x')", 0);
    let resp = s.completion("caller.php", line, ch).await;
    let out = render_completion_ordered(&resp);
    expect![[r#"
        Method      get
        Method      post"#]]
    .assert_eq(&out);
}

/// Regression test for a confirmed false positive: two unrelated classes
/// sharing a short name in different namespaces (Laravel ships exactly this
/// — `Illuminate\Support\Facades\Auth` the real facade, and `Illuminate\
/// Container\Attributes\Auth` a tiny constructor-injection attribute) must
/// resolve static-member completion to the one the file actually imports,
/// not whichever same-named class the workspace index happens to hit first.
#[tokio::test]
async fn completion_static_resolves_use_imported_class_over_namespace_collision() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("FacadeAuth.php"),
        "<?php\n\nnamespace Acme\\Support\\Facades;\n\n/**\n * @method static bool check()\n * @method static mixed user()\n */\nclass Auth\n{\n}\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("AttributeAuth.php"),
        "<?php\n\nnamespace Acme\\Container\\Attributes;\n\nclass Auth\n{\n    public static function resolve(): mixed {}\n}\n",
    )
    .unwrap();
    let caller =
        "<?php\nuse Acme\\Support\\Facades\\Auth;\n\nfunction f() {\n    Auth::check();\n}\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "check();", 0);
    let resp = s.completion("caller.php", line, ch).await;
    let out = render_completion_ordered(&resp);
    expect![[r#"
        Method      check
        Method      user"#]]
    .assert_eq(&out);
}

/// Typing the bare short name of a class that lives in a file which was
/// never opened in the editor (e.g. a vendor package) must still surface it
/// as a completion candidate, with an `additionalTextEdits` auto-import.
#[tokio::test]
async fn completion_bare_class_name_from_unopened_file_adds_use_import() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Http.php"),
        "<?php\n\nnamespace Acme\\Support\\Facades;\n\nclass Http\n{\n}\n",
    )
    .unwrap();
    let caller = "<?php\n$r = Htt;\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "Htt", 0);
    let resp = s.completion("caller.php", line, ch + 3).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let http_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("Http"))
        .expect("Http class (defined in an unopened file) must be in completions");
    assert_eq!(
        http_item["detail"].as_str(),
        Some("Acme\\Support\\Facades\\Http")
    );
    let out = render_text_edits(&json!({ "result": http_item["additionalTextEdits"] }));
    expect![[r#"1:0-1:0 → "use Acme\\Support\\Facades\\Http;\\n""#]].assert_eq(&out);
}

/// `vendor/` is eagerly indexed by default (issues #240, #246 — see
/// `index_vendor` in `lang/config.rs`), so a class defined there is visible
/// to the workspace index, and by extension to bare-name completion
/// (auto-import included), with no configuration required.
#[tokio::test]
async fn completion_bare_class_name_vendor_dir_indexed_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("vendor/acme/http-client/src")).unwrap();
    std::fs::write(
        tmp.path().join("vendor/acme/http-client/src/Http.php"),
        "<?php\n\nnamespace Acme\\Support\\Facades;\n\nclass Http\n{\n}\n",
    )
    .unwrap();
    let caller = "<?php\n$r = Htt;\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "Htt", 0);
    let resp = s.completion("caller.php", line, ch + 3).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let http_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("Http"))
        .expect("vendor/-defined class must appear by default");
    assert_eq!(
        http_item["detail"].as_str(),
        Some("Acme\\Support\\Facades\\Http")
    );
    let out = render_text_edits(&json!({ "result": http_item["additionalTextEdits"] }));
    expect![[r#"1:0-1:0 → "use Acme\\Support\\Facades\\Http;\\n""#]].assert_eq(&out);
}

/// Setting `indexVendor: false` opts back out of eager vendor indexing (for
/// very large vendor trees where even the cheap declaration scan isn't worth
/// it), so a vendor-defined class goes back to being invisible to the
/// workspace index, and by extension to bare-name completion. Pins this
/// known boundary so it doesn't silently change.
#[tokio::test]
async fn completion_bare_class_name_vendor_dir_excluded_with_index_vendor_false() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("vendor/acme/http-client/src")).unwrap();
    std::fs::write(
        tmp.path().join("vendor/acme/http-client/src/Http.php"),
        "<?php\n\nnamespace Acme\\Support\\Facades;\n\nclass Http\n{\n}\n",
    )
    .unwrap();
    let caller = "<?php\n$r = Htt;\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s =
        TestServer::with_root_and_options(tmp.path(), serde_json::json!({ "indexVendor": false }))
            .await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "Htt", 0);
    let resp = s.completion("caller.php", line, ch + 3).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    assert!(
        !items.iter().any(|i| i["label"].as_str() == Some("Http")),
        "vendor/-defined class must not appear with indexVendor: false"
    );
}

/// The current document's own class must not be offered as a spurious
/// self-import: the workspace-index search sees every file's classes,
/// including the file currently being edited.
#[tokio::test]
async fn completion_bare_class_name_excludes_own_file_self_import() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = "<?php\n\nclass HttpClient {}\n\n$r = HttpCli;\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "HttpCli", 1);
    let resp = s
        .completion("caller.php", line, ch + "HttpCli".len() as u32)
        .await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let http_item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("HttpClient"))
        .expect("HttpClient must be suggested (declared in the current file)");
    assert!(
        http_item["additionalTextEdits"].is_null()
            || http_item["additionalTextEdits"]
                .as_array()
                .is_some_and(|e| e.is_empty()),
        "must not suggest importing a class from its own file"
    );
}

// ── Type system — generic and docblock annotations ────────────────────────────

/// `@var Collection<User>` — generic type param stripped so member lookup
/// targets "Collection" correctly.
#[tokio::test]
async fn completion_generic_annotation_resolves_base_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class User { public string $name = ''; }
class Collection {
    public function first(): ?User { return null; }
    public function count(): int { return 0; }
}

/** @var Collection<User> $coll */
$coll = getCollection();
$coll->$0
"#,
        )
        .await;
    expect![[r#"
        Method      count
        Method      first"#]]
    .assert_eq(&out);
}

/// `@psalm-type Result = Success|Failure` defined in class docblock — alias
/// expanded before member lookup so both union types contribute completions.
#[tokio::test]
async fn completion_psalm_type_alias_expands() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Success { public function ok(): bool { return true; } }
class Failure { public function reason(): string { return ''; } }

/**
 * @psalm-type Result = Success|Failure
 */
class Processor {
    /**
     * @param Result $r
     */
    public function handle($r): void {
        $r->$0
    }
}
"#,
        )
        .await;
    expect![[r#"
        Method      ok
        Method      reason"#]]
    .assert_eq(&out);
}

/// `@var list<Widget>` on the iterable — mir types the foreach value variable
/// from the iterable's own resolved element type natively, so members are
/// available inside the loop body with no php-lsp-side propagation step.
#[tokio::test]
async fn completion_list_element_type_in_foreach() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Widget {
    public function getId(): int { return 0; }
    public string $label = '';
}

/** @var list<Widget> $widgets */
$widgets = fetchWidgets();
foreach ($widgets as $w) {
    $w->$0
}
"#,
        )
        .await;
    expect![[r#"
        Property    $label
        Method      getId"#]]
    .assert_eq(&out);
}

/// Companion to `var_annotation_survives_split_php_html_block` in
/// `tests/analysis/feature_diagnostics_edge_cases.rs` (issue #235): a
/// `@var` docblock immediately followed by `?>` (closing the PHP block)
/// still attaches to `$model`, so completion sees its type across the
/// intervening HTML and re-opened `<?php` block.
#[tokio::test]
async fn completion_var_annotation_survives_split_php_html_block() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Post { public string $title = ''; }
/** @var Post $model */
?>
<div>
<?php if (!empty($model->title)): ?>
    <?php echo $model->$0 ?>
<?php endif; ?>
</div>
"#,
        )
        .await;
    expect![[r#"
        Property    $title"#]]
    .assert_eq(&out);
}
