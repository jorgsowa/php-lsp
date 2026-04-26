//! Completion coverage across trigger characters and contexts.
//!
//! Each test asserts on the presence of specific labels rather than full
//! snapshots — completion lists contain many built-ins/keywords whose ordering
//! is driven by ranking heuristics.

mod common;

use common::TestServer;
use expect_test::expect;

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

#[tokio::test]
async fn completion_arrow_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_completion(
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
    expect![[r#"
        Method      bye
        Method      hello"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_arrow_property() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
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
    assert!(labels.iter().any(|l| l == "name" || l == "$name"));
}

#[tokio::test]
async fn completion_double_colon_static_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_completion(
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
        Variable    $GLOBALS
        Variable    $_COOKIE
        Variable    $_ENV
        Variable    $_FILES
        Variable    $_GET
        Variable    $_POST
        Variable    $_REQUEST
        Variable    $_SERVER
        Variable    $_SESSION
        Class       Reg
        Constant    __CLASS__
        Constant    __DIR__
        Constant    __FILE__
        Constant    __FUNCTION__
        Constant    __LINE__
        Constant    __METHOD__
        Constant    __NAMESPACE__
        Constant    __TRAIT__
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
        Method      get
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
        Function    mkdir
        Function    mktime
        Function    mt_rand
        Keyword     namespace
        Keyword     new
        Function    nl2br
        Keyword     null
        Function    number_format
        Function    ob_end_clean
        Function    ob_get_clean
        Function    ob_start
        Function    opendir
        Keyword     or
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
        Method      set
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
        Function    vsprintf
        Keyword     while
        Keyword     xor
        Keyword     yield"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_namespace_prefix() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
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
    assert!(
        labels.iter().any(|l| l == "Greeter"),
        "expected Greeter in namespace-prefix completions: {labels:?}"
    );
}

#[tokio::test]
async fn completion_keyword_in_top_level() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
func$0
"#,
    )
    .await;
    assert!(labels.iter().any(|l| l == "function"));
}

#[tokio::test]
async fn completion_variable_in_scope() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
function f(string $name, int $count): void {
    $na$0
}
"#,
    )
    .await;
    assert!(
        labels.iter().any(|l| l == "$name"),
        "expected $name: {labels:?}"
    );
}

#[tokio::test]
async fn completion_method_does_not_leak_to_unrelated_classes() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
class A { public function foo(): void {} }
class B { public function bar(): void {} }
$a = new A();
$a->$0
"#,
    )
    .await;
    assert!(labels.iter().any(|l| l == "foo"));
    assert!(
        !labels.iter().any(|l| l == "bar"),
        "B::bar should not appear in A completion: {labels:?}"
    );
}

/// `Status::$0` on a PHP 8.1 enum must offer the declared cases. The server
/// returns them as fully-qualified labels (`Status::Active`, `Status::Inactive`)
/// alongside the global completion list. Both labels must be present.
#[tokio::test]
async fn completion_enum_case_access() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
enum Status { case Active; case Inactive; }
Status::$0
"#,
    )
    .await;
    assert!(
        labels.iter().any(|l| l == "Status::Active"),
        "expected Status::Active in enum case completions: {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "Status::Inactive"),
        "expected Status::Inactive in enum case completions: {labels:?}"
    );
}

/// `new $0` must include class names so users can pick from defined classes.
#[tokio::test]
async fn completion_after_new_offers_class_names() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
class Widget {}
class Gadget {}
$x = new $0
"#,
    )
    .await;
    assert!(
        labels.iter().any(|l| l == "Widget"),
        "expected Widget in `new` completions: {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "Gadget"),
        "expected Gadget in `new` completions: {labels:?}"
    );
}

/// Verify that `completionItem/resolve` is wired up end-to-end: request a
/// completion list, pick an item, resolve it, and check the `detail` field is
/// populated.
#[tokio::test]
async fn completion_resolve_returns_item() {
    let mut server = TestServer::new().await;
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
    assert!(
        !items.is_empty(),
        "expected completions for 'resolveM' prefix: {:?}",
        comp["result"]
    );

    let resolve_me = items
        .iter()
        .find(|i| i["label"].as_str() == Some("resolveMe"))
        .cloned()
        .expect("resolveMe must appear in completions for its own prefix");

    let resp = server.completion_resolve(resolve_me).await;

    assert!(
        resp["error"].is_null(),
        "completionItem/resolve error: {resp:?}"
    );
    assert!(resp["result"].is_object(), "expected resolved item object");
    let detail = resp["result"]["detail"].as_str().unwrap_or("");
    assert!(
        detail.contains("resolveMe"),
        "resolved item must have detail populated with the function signature: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn completion_this_arrow_includes_trait_methods() {
    let mut s = TestServer::new().await;
    let out = s
        .check_completion(
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
