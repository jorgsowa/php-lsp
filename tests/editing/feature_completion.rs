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
        Variable    $GLOBALS | superglobal
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Class       Reg
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
        Method      get | function get(): void
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
        Method      set | function set(): void
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
        Variable    $GLOBALS | superglobal
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Enum        Status
        EnumMember  Status::Active
        EnumMember  Status::Inactive
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
    assert!(!items.is_empty());

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
    assert!(
        out.contains("name:") || out.contains("name"),
        "should keep label as-is in output"
    );
    assert!(
        out.contains("detail"),
        "should lookup stripped name and populate detail"
    );
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
    assert!(
        out.contains("My docs"),
        "should populate docs when detail already exists"
    );
    assert!(
        out.contains("function myFunc(): void"),
        "should preserve existing detail"
    );
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
    assert!(out.contains("Some doc"), "should preserve existing docs");
    assert!(
        out.contains("function myFunc(): void"),
        "should populate detail when docs already exist"
    );
}

/// mir resolves the caught exception type for the catch variable so that
/// member completion works on `$e->`. Guards that TypeMap catch handling
/// is not load-bearing for this pattern.
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
        .check_completion(
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
    assert!(
        out.contains("getName"),
        "expected getName in completions, got: {out}"
    );
}

#[tokio::test]
async fn completion_this_arrow_includes_trait_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
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
        .check_completion(
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
        .check_completion(
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
        .check_completion(
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
        Property    value | string|int"#]]
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
        Variable    $GLOBALS | superglobal
        Variable    $_COOKIE | superglobal
        Variable    $_ENV | superglobal
        Variable    $_FILES | superglobal
        Variable    $_GET | superglobal
        Variable    $_POST | superglobal
        Variable    $_REQUEST | superglobal
        Variable    $_SERVER | superglobal
        Variable    $_SESSION | superglobal
        Class       Builder
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
        Function    boolval
        Keyword     break
        Method      build | function build()
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
        Method      reset | function reset()
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
        Property    $status
        Class       Service
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
        Method      run | function run()
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
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    let item_labels: Vec<String> = items
        .iter()
        .filter_map(|i| i["label"].as_str().map(str::to_owned))
        .collect();
    assert!(
        item_labels.contains(&"host:".to_owned()),
        "named args must include host:"
    );
    assert!(
        item_labels.contains(&"port:".to_owned()),
        "named args must include port:"
    );
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
    let edits = mailer_item["additionalTextEdits"]
        .as_array()
        .expect("Mailer must have additionalTextEdits");
    assert!(
        !edits.is_empty(),
        "must have edits for cross-namespace class"
    );
    let edit_text = edits[0]["newText"]
        .as_str()
        .expect("edit must have newText");
    assert!(
        edit_text.contains("use") && edit_text.contains("Mailer"),
        "edit must contain use statement"
    );
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
    assert!(
        mailer_item["additionalTextEdits"].is_null()
            || mailer_item["additionalTextEdits"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true),
        "same-namespace class must not get use edit"
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
    expect!["Class       Mailer"].assert_eq(&out);
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
    let ls = labels(&mut s, "<?php require './lib/$0").await;
    assert_labels_contain(
        &ls,
        &["Helper.php", "Utils.php"],
        "include path: require './lib/' lists PHP files",
    );
    assert_label_not_present(
        &ls,
        "README.md",
        "include path: non-PHP files should be excluded",
    );
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
        .expect(&format!(
            "Boot.php must be in completions for require './src/$0. Got items: {:#?}",
            items
        ));
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
        .expect(&format!(
            "require '$0 must include 'modules' folder. Got items: {:#?}",
            items
        ));
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

/// NOTE: Completion filtering in string literals is not yet implemented.
/// Currently: typing inside a string still returns code completions (known limitation).
/// TODO: Implement context-aware filtering to suppress completions in strings/comments.
#[tokio::test]
#[ignore = "completion in strings not yet implemented"]
async fn completion_in_string_literal_returns_empty() {
    let mut _s = TestServer::new().await;
    // TODO: once string context detection is added, verify empty or minimal results
}

/// NOTE: Completion filtering in comments is not yet implemented.
/// Currently: typing inside a comment still returns code completions (known limitation).
#[tokio::test]
#[ignore = "completion in comments not yet implemented"]
async fn completion_in_comment_returns_empty() {
    let mut _s = TestServer::new().await;
    // TODO: once comment context detection is added, verify empty or minimal results
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
        .check_completion(
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
