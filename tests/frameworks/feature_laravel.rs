//! Protocol-wired regression tests for all major LSP features against the real
//! Laravel framework corpus (~1 600 PHP files, ~2 900 total with types stubs).
//!
//! Each test exercises one feature area end-to-end through the wire protocol:
//! workspace scan → indexReady → open file → LSP request.  Running independently
//! (separate `TestServer`) prevents cross-test interference and timeouts from
//! cascading.
//!
//! # Setup
//!
//! ```bash
//! scripts/setup_laravel_fixture.sh
//! ```
//!
//! # Running
//!
//! ```bash
//! cargo test --test frameworks laravel -- --ignored --nocapture
//! ```
//!
//! The tests are `#[ignore]` so they don't run in CI by default — the Laravel
//! fixture is large and lives outside the normal test fixtures.

use super::*;
use expect_test::expect;

// ── Fixture constants ─────────────────────────────────────────────────────────

/// Root of the Laravel framework source tree (the `src/` subtree).
const LARAVEL_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/fixtures/laravel/src");

fn laravel_available() -> bool {
    std::path::Path::new(LARAVEL_SRC)
        .join("Illuminate/Auth/AuthManager.php")
        .exists()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(LARAVEL_SRC).join(rel))
        .unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
}

// ── Go to Definition ──────────────────────────────────────────────────────────

/// GoToDef for a class name on its own declaration line resolves to itself.
/// Guards against "definition on declaration returns null" regression.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_definition_class_declaration() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 17 (0-based) = `class AuthManager implements FactoryContract`
    // Character 6 = start of "AuthManager".
    let resp = s.definition("Illuminate/Auth/AuthManager.php", 17, 6).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["Illuminate/Auth/AuthManager.php:17:6-17:17"].assert_eq(&out);
}

/// GoToDef on a method declaration resolves to that method's own range.
/// Guards against same-file method definition returning null.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_definition_method_declaration() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 69 (0-based) = `    public function guard($name = null)`
    // Character 20 = start of "guard".
    let resp = s
        .definition("Illuminate/Auth/AuthManager.php", 69, 20)
        .await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["Illuminate/Auth/AuthManager.php:69:20-69:25"].assert_eq(&out);
}

/// GoToDef on a method call site (`$this->guard()`) resolves to the declaration.
/// Guards against same-class call-site definition returning null.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_definition_from_call_site() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 60 (0-based) = `        $this->userResolver = fn ($guard = null) => $this->guard($guard)->user();`
    // Character 59 = start of "guard" in the second `$this->guard(…)`.
    let resp = s
        .definition("Illuminate/Auth/AuthManager.php", 60, 59)
        .await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["Illuminate/Auth/AuthManager.php:69:20-69:25"].assert_eq(&out);
}

/// GoToDef on a static method call (`Str::camel`) navigates to `Str.php`.
/// Guards against static method cross-file definition returning null.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_definition_on_static_call() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/Access/Gate.php",
        &read("Illuminate/Auth/Access/Gate.php"),
    )
    .await;

    // Line 855 (0-based) = `        return str_contains($ability, '-') ? Str::camel($ability) : $ability;`
    // Character 50 = start of "camel" after `Str::`.
    let resp = s
        .definition("Illuminate/Auth/Access/Gate.php", 855, 50)
        .await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["Illuminate/Support/Str.php:225:27-225:32"].assert_eq(&out);
}

/// GoToDef on `new RequestGuard(…)` navigates to `RequestGuard.php`.
///
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_definition_on_new_expression() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 235 (0-based) = `            $guard = new RequestGuard($callback, …);`
    // Character 25 = start of "RequestGuard".
    let resp = s
        .definition("Illuminate/Auth/AuthManager.php", 235, 25)
        .await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["Illuminate/Auth/RequestGuard.php:9:6-9:18"].assert_eq(&out);
}

/// GoToDef on a trait in a `use` statement navigates to the trait file.
/// Guards against trait use definition returning null.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_definition_on_trait_use() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 19 (0-based) = `    use CreatesUserProviders, RebindsCallbacksToSelf;`
    // Character 8 = start of "CreatesUserProviders".
    let resp = s.definition("Illuminate/Auth/AuthManager.php", 19, 8).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["Illuminate/Auth/CreatesUserProviders.php:6:6-6:26"].assert_eq(&out);
}

/// GoToDef on a cross-file import (`use ... as`) resolves into the target file.
/// Guards against cross-file navigation returning null after a workspace scan.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_definition_cross_file_use_import() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 5 (0-based) = `use Illuminate\Contracts\Auth\Factory as FactoryContract;`
    // Character 30 = start of "Factory" in the qualified name.
    let resp = s.definition("Illuminate/Auth/AuthManager.php", 5, 30).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["Illuminate/Contracts/Auth/Factory.php:4:10-4:17"].assert_eq(&out);
}

/// GoToDef on an interface name in the `implements` clause navigates cross-file.
/// Guards against cross-file navigation on the implements clause returning null.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_definition_cross_file_implements() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 17 (0-based) = `class AuthManager implements FactoryContract`
    // Character 29 = start of "FactoryContract".
    let resp = s
        .definition("Illuminate/Auth/AuthManager.php", 17, 29)
        .await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["Illuminate/Contracts/Auth/Factory.php:4:10-4:17"].assert_eq(&out);
}

// ── Hover ─────────────────────────────────────────────────────────────────────

/// Hover on a class name shows the class signature and PHPDoc.
/// Guards against class hover returning empty after index.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_hover_class_declaration() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 17 (0-based), character 6 = "AuthManager".
    let resp = s.hover("Illuminate/Auth/AuthManager.php", 17, 6).await;
    let out = render_hover(&resp);
    expect![[r#"
        ```php
        class AuthManager implements FactoryContract
        ```

        ---

        **@mixin** `\Illuminate\Contracts\Auth\Guard`
        **@mixin** `\Illuminate\Contracts\Auth\StatefulGuard`"#]]
    .assert_eq(&out);
}

/// Hover on a method name shows its signature and PHPDoc summary.
/// Guards against method hover returning empty.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_hover_method_shows_signature_and_doc() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 69 (0-based) = `    public function guard($name = null)`
    // Character 20 = "guard".
    let resp = s.hover("Illuminate/Auth/AuthManager.php", 69, 20).await;
    let out = render_hover(&resp);
    expect![[r#"
        ```php
        public function guard($name = null)
        ```

        ---

        Attempt to get the guard from the local cache.

        **@return** `\Illuminate\Contracts\Auth\Guard|\Illuminate\Contracts\Auth\StatefulGuard`
        **@param** `\UnitEnum|string|null` `$name`"#]]
    .assert_eq(&out);
}

/// Hover on a property declaration shows its type and docblock.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_hover_property_declaration() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 40 (0-based) = `    protected $guards = [];`
    // Character 14 = "$guards".
    let resp = s.hover("Illuminate/Auth/AuthManager.php", 40, 14).await;
    let out = render_hover(&resp);
    expect![[r#"
        ```php
        (property) protected AuthManager::$guards
        ```

        ---

        The array of created "drivers".

        **@var** `array`"#]]
    .assert_eq(&out);
}

/// Hover on a method call site (`$this->guard()`) shows the guard() signature.
/// Guards against hover at a call site returning empty/null.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_hover_on_call_site() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 60 (0-based), character 59 = "guard" in `$this->guard($guard)`.
    let resp = s.hover("Illuminate/Auth/AuthManager.php", 60, 59).await;
    let out = render_hover(&resp);
    expect![[r#"
        ```php
        AuthManager::guard($name = null): Illuminate\Contracts\Auth\Guard|Illuminate\Contracts\Auth\StatefulGuard
        ```

        ---

        Attempt to get the guard from the local cache.

        **@return** `\Illuminate\Contracts\Auth\Guard|\Illuminate\Contracts\Auth\StatefulGuard`
        **@param** `\UnitEnum|string|null` `$name`"#]].assert_eq(&out);
}

/// Hover on a static method call (`Str::camel`) shows the camel() signature.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_hover_on_static_call() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/Access/Gate.php",
        &read("Illuminate/Auth/Access/Gate.php"),
    )
    .await;

    // Line 855 (0-based), character 50 = "camel" in `Str::camel($ability)`.
    let resp = s.hover("Illuminate/Auth/Access/Gate.php", 855, 50).await;
    let out = render_hover(&resp);
    expect![[r#"
        ```php
        Str::camel($value): string
        ```

        ---

        Convert a value to camel case.

        **@return** `($value is "" ? "" : string)` — is '' ? '' : string)
        **@param** `string` `$value`"#]]
    .assert_eq(&out);
}

/// Hover on an interface name at an `implements` clause shows the interface.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_hover_implements_interface() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 17, character 29 = "FactoryContract".
    let resp = s.hover("Illuminate/Auth/AuthManager.php", 17, 29).await;
    let out = render_hover(&resp);
    expect![[r#"
        ```php
        interface Factory
        ```"#]]
    .assert_eq(&out);
}

// ── Find References ───────────────────────────────────────────────────────────

/// `Str::lower` is called from many files in the framework; references must
/// include at least 8 call sites and contain the known caller
/// `QueriesRelationships.php`.
///
/// Guards against cross-file reference discovery breaking after index changes.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_references_static_method_cross_file() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Support/Str.php",
        &read("Illuminate/Support/Str.php"),
    )
    .await;

    // Line 755 (0-based) = `    public static function lower($value)`
    // Character 27 = "lower".
    let resp = s
        .references("Illuminate/Support/Str.php", 755, 27, false)
        .await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    let out = render_locations(&resp, &s.uri(""));
    // Must have ≥8 references spanning multiple files including QueriesRelationships.php
    expect![[r#"
        Illuminate/Database/Eloquent/Concerns/QueriesRelationships.php:869:47-869:52
        Illuminate/Foundation/Console/DocsCommand.php:239:25-239:30
        Illuminate/Foundation/Console/DocsCommand.php:241:58-241:63
        Illuminate/Foundation/Console/DocsCommand.php:241:78-241:83
        Illuminate/Foundation/Console/DocsCommand.php:247:61-247:66
        Illuminate/Foundation/Console/ViewMakeCommand.php:184:29-184:34
        Illuminate/Support/Str.php:1568:87-1568:92
        Illuminate/Support/Str.php:1594:29-1594:34
        Illuminate/Support/Str.php:1719:41-1719:46
        Illuminate/Support/Str.php:1860:23-1860:28
        Illuminate/Support/Stringable.php:486:31-486:36
        Illuminate/Validation/Concerns/ValidatesAttributes.php:1466:41-1466:46
        Illuminate/Validation/Concerns/ValidatesAttributes.php:2492:24-2492:29"#]]
    .assert_eq(&out);
}

/// References to the `guard()` method in AuthManager includes the declaration
/// and at least the intra-file self-calls.
/// Guards against method references returning empty.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_references_method_includes_declaration() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 69 (0-based) = `    public function guard($name = null)`
    // Character 20 = "guard".
    let resp = s
        .references("Illuminate/Auth/AuthManager.php", 69, 20, true)
        .await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    let out = render_locations(&resp, &s.uri(""));
    expect![[r#"
        Illuminate/Auth/AuthManager.php:211:58-211:63
        Illuminate/Auth/AuthManager.php:347:22-347:27
        Illuminate/Auth/AuthManager.php:60:59-60:64
        Illuminate/Auth/AuthManager.php:69:20-69:25
        Illuminate/Auth/Middleware/Authenticate.php:81:29-81:34
        Illuminate/Auth/Middleware/AuthenticateWithBasicAuth.php:53:21-53:26
        Illuminate/Contracts/Auth/Factory.php:12:20-12:25"#]]
    .assert_eq(&out);
}

// ── Completion ────────────────────────────────────────────────────────────────

/// Completing `$this->` inside AuthManager returns the class's own members.
/// Guards against member completion returning empty after a workspace scan.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_completion_this_members() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 59 (0-based) = `        $this->app = $app;`
    // Character 15 = immediately after `$this->`.
    let resp = s
        .completion("Illuminate/Auth/AuthManager.php", 59, 15)
        .await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    let out = render_completion(&resp);
    // Must include guard, $app, $guards, $customCreators, resolve(), etc. (≥5 members)
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
        Property    $app
        Property    $customCreators
        Property    $guards
        Property    $userResolver
        Class       AuthManager
        Constant    __CLASS__
        Constant    __DIR__
        Constant    __FILE__
        Constant    __FUNCTION__
        Constant    __LINE__
        Constant    __METHOD__
        Constant    __NAMESPACE__
        Constant    __TRAIT__
        Method      __call
        Method      __call(method:, parameters:)
        Method      __callStatic
        Method      __clone
        Method      __construct
        Method      __construct(app:)
        Method      __debugInfo
        Method      __destruct
        Method      __get
        Method      __invoke
        Method      __isset
        Method      __serialize
        Method      __set
        Method      __sleep
        Method      __toString
        Method      __unserialize
        Method      __unset
        Method      __wakeup
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
        Method      callCustomCreator
        Method      callCustomCreator(name:, config:)
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
        Method      createSessionDriver
        Method      createSessionDriver(name:, config:)
        Method      createTokenDriver
        Method      createTokenDriver(name:, config:)
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
        Method      extend
        Method      extend(driver:, callback:)
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
        Method      forgetGuards
        Function    fputs
        Function    fread
        Function    fseek
        Function    ftell
        Keyword     function
        Function    function_exists
        Function    fwrite
        Method      getConfig
        Method      getConfig(name:)
        Method      getDefaultDriver
        Function    get_class
        Function    get_parent_class
        Function    gettype
        Function    glob
        Keyword     global
        Keyword     goto
        Method      guard
        Method      guard(name:)
        Method      hasResolvedGuards
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
        Method      provider
        Method      provider(name:, callback:)
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
        Method      resolve
        Method      resolve(name:)
        Method      resolveUsersUsing
        Method      resolveUsersUsing(userResolver:)
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
        Method      setApplication
        Method      setApplication(app:)
        Method      setDefaultDriver
        Method      setDefaultDriver(name:)
        Function    setcookie
        Function    settype
        Function    sha1
        Method      shouldUse
        Method      shouldUse(name:)
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
        Method      userResolver
        Function    usleep
        Function    usort
        Keyword     var
        Function    var_dump
        Function    var_export
        Method      viaRequest
        Method      viaRequest(driver:, callback:)
        Keyword     void
        Function    vsprintf
        Keyword     while
        Keyword     xor
        Keyword     yield"#]]
    .assert_eq(&out);
}

/// `Str::` triggers static member completion with camel, lower, upper, etc.
///
/// Regression guard: static member completion resolves the class from a
/// `use`-import alias (`use Illuminate\Support\Str`) by expanding the alias
/// to its FQCN and extracting the short class name for the workspace index
/// lookup, rather than falling back to PHP keyword/global completions.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_completion_static_members() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;

    // Synthetic file with a `Str::` trigger to test static member completion.
    let src = "<?php\nuse Illuminate\\Support\\Str;\nStr::\n";
    s.open("__test_static_completion.php", src).await;

    // Line 2 (0-based), character 5 = immediately after `Str::` (S=0,t=1,r=2,:=3,:=4, cursor at 5).
    let resp = s.completion("__test_static_completion.php", 2, 5).await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    let out = render_completion(&resp);
    // Must include camel() and other Str:: static members
    expect![[r#"
        Property    $camelCache
        Property    $macros
        Property    $randomStringFactory
        Property    $snakeCache
        Property    $studlyCache
        Property    $ulidFactory
        Property    $uuidFactory
        Constant    INVISIBLE_CHARACTERS
        Method      __callStatic
        Method      after
        Method      afterLast
        Method      apa
        Method      ascii
        Method      before
        Method      beforeLast
        Method      between
        Method      betweenFirst
        Method      camel
        Method      charAt
        Method      chopEnd
        Method      chopStart
        Method      contains
        Method      containsAll
        Method      convertCase
        Method      createRandomStringsNormally
        Method      createRandomStringsUsing
        Method      createRandomStringsUsingSequence
        Method      createUlidsNormally
        Method      createUlidsUsing
        Method      createUlidsUsingSequence
        Method      createUuidsNormally
        Method      createUuidsUsing
        Method      createUuidsUsingSequence
        Method      deduplicate
        Method      doesntContain
        Method      doesntEndWith
        Method      doesntStartWith
        Method      endsWith
        Method      excerpt
        Method      finish
        Method      flushCache
        Method      flushMacros
        Method      freezeUlids
        Method      freezeUuids
        Method      fromBase64
        Method      hasMacro
        Method      headline
        Method      initials
        Method      inlineMarkdown
        Method      is
        Method      isAscii
        Method      isJson
        Method      isMatch
        Method      isUlid
        Method      isUrl
        Method      isUuid
        Method      kebab
        Method      lcfirst
        Method      length
        Method      limit
        Method      lower
        Method      ltrim
        Method      macro
        Method      markdown
        Method      mask
        Method      match
        Method      matchAll
        Method      mixin
        Method      numbers
        Method      of
        Method      orderedUuid
        Method      padBoth
        Method      padLeft
        Method      padRight
        Method      parseCallback
        Method      pascal
        Method      password
        Method      plural
        Method      pluralPascal
        Method      pluralStudly
        Method      position
        Method      random
        Method      remove
        Method      repeat
        Method      replace
        Method      replaceArray
        Method      replaceEnd
        Method      replaceFirst
        Method      replaceLast
        Method      replaceMatches
        Method      replaceStart
        Method      resetFactoryState
        Method      reverse
        Method      rtrim
        Method      singular
        Method      slug
        Method      snake
        Method      squish
        Method      start
        Method      startsWith
        Method      studly
        Method      substr
        Method      substrCount
        Method      substrReplace
        Method      swap
        Method      take
        Method      title
        Method      toBase64
        Method      toStringOr
        Method      transliterate
        Method      trim
        Method      ucfirst
        Method      ucsplit
        Method      ucwords
        Method      ulid
        Method      unwrap
        Method      upper
        Method      uuid
        Method      uuid7
        Method      wordCount
        Method      wordWrap
        Method      words
        Method      wrap"#]]
    .assert_eq(&out);
}

// ── Diagnostics ───────────────────────────────────────────────────────────────

/// Opening a clean Laravel framework file produces no diagnostics.
/// Guards against false-positive noise on real-world code.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_diagnostics_clean_file_no_noise() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    // Wait for the workspace index so trait declarations from other files are
    // known to the analyzer — without this, traits like RebindsCallbacksToSelf
    // produce false-positive "does not exist" errors.
    s.wait_for_index_ready_secs(60).await;
    let diag = s
        .open(
            "Illuminate/Auth/AuthManager.php",
            &read("Illuminate/Auth/AuthManager.php"),
        )
        .await;
    let empty = vec![];
    let all = diag["params"]["diagnostics"].as_array().unwrap_or(&empty);
    let errors: Vec<_> = all
        .iter()
        .filter(|d| d["severity"].as_u64() == Some(1))
        .collect();
    assert!(
        errors.is_empty(),
        "expected 0 unexpected errors in clean AuthManager.php, got: {errors:#?}"
    );
}

/// A file with a return-type mismatch produces an error diagnostic.
/// Guards against type-error diagnostics silently stopping after index.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_diagnostics_type_error_fires() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    // Inject a synthetic file with a known return-type violation.
    let bad = "<?php\nnamespace Illuminate\\Auth;\nfunction bad_func(): string { return 42; }\n";
    let diag = s.open("Illuminate/Auth/__test_diag.php", bad).await;
    let empty = vec![];
    let all = diag["params"]["diagnostics"].as_array().unwrap_or(&empty);
    let errors: Vec<_> = all
        .iter()
        .filter(|d| d["severity"].as_u64() == Some(1))
        .collect();
    assert!(
        !errors.is_empty(),
        "expected a type-error diagnostic for 'return 42' where string declared, got none"
    );
    let msg = errors[0]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("string") || msg.contains("Return") || msg.contains("compatible"),
        "unexpected error message: {msg}"
    );
}

/// `didChange` on an open file triggers a fresh `publishDiagnostics`.
/// Guards against diagnostic updates stalling after an edit.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_diagnostics_update_on_did_change() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    let clean = "<?php\nnamespace Illuminate\\Auth;\nfunction ok(): string { return 'hi'; }\n";
    s.open("Illuminate/Auth/__test_change.php", clean).await;

    // Introduce a return-type error via didChange.
    let bad = "<?php\nnamespace Illuminate\\Auth;\nfunction ok(): string { return 42; }\n";
    let diag = s.change("Illuminate/Auth/__test_change.php", 2, bad).await;
    let empty = vec![];
    let all = diag["params"]["diagnostics"].as_array().unwrap_or(&empty);
    let errors: Vec<_> = all
        .iter()
        .filter(|d| d["severity"].as_u64() == Some(1))
        .collect();
    assert!(
        !errors.is_empty(),
        "expected error after didChange introduced a type mismatch, got none"
    );
}

/// Opening `Eloquent/Model.php` produces no unexpected error diagnostics.
///
/// Regression guard: `tap()` and `class_uses_recursive()` are autoload-file
/// helpers declared in composer.json's `autoload.files` section; the
/// workspace scanner discovers and pre-ingests those files into the mir
/// session so they don't produce false `UndefinedFunction` diagnostics.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_diagnostics_no_noise_model() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    let diag = s
        .open(
            "Illuminate/Database/Eloquent/Model.php",
            &read("Illuminate/Database/Eloquent/Model.php"),
        )
        .await;
    let empty = vec![];
    let all = diag["params"]["diagnostics"].as_array().unwrap_or(&empty);
    // The test guards specifically against false UndefinedFunction / UndefinedClass
    // noise from autoload.files helpers (tap, class_uses_recursive, …).
    // Type-level issues in Model.php are a separate concern and excluded here.
    let undef_noise: Vec<_> = all
        .iter()
        .filter(|d| d["severity"].as_u64() == Some(1))
        .filter(|d| {
            let code = d["code"].as_str().unwrap_or("");
            matches!(
                code,
                "UndefinedFunction" | "UndefinedClass" | "UndefinedTrait"
            )
        })
        .collect();
    assert!(
        undef_noise.is_empty(),
        "expected no undefined-function/class noise in Eloquent/Model.php, got: {undef_noise:#?}"
    );
}

// ── Document Symbols ──────────────────────────────────────────────────────────

/// `documentSymbol` for AuthManager returns a hierarchical structure with the
/// class at the top and its methods and properties as children.
/// Guards against document symbols returning empty after index.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_document_symbols_hierarchical() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    let resp = s.document_symbols("Illuminate/Auth/AuthManager.php").await;
    let out = render_document_symbols(&resp);
    expect![[r#"
        Class AuthManager @L17
          Property $app @L26
          Property $customCreators @L33
          Property $guards @L40
          Property $userResolver @L49
          Method __construct @L56
            Variable $app @L56
          Method guard @L69
            Variable $name @L69
          Method resolve @L84
            Variable $name @L84
          Method callCustomCreator @L114
            Variable $name @L114
            Variable $config @L114
          Method createSessionDriver @L126
            Variable $name @L126
            Variable $config @L126
          Method createTokenDriver @L160
            Variable $name @L160
            Variable $config @L160
          Method getConfig @L184
            Variable $name @L184
          Method getDefaultDriver @L194
          Method shouldUse @L205
            Variable $name @L205
          Method setDefaultDriver @L220
            Variable $name @L220
          Method viaRequest @L232
            Variable $driver @L232
            Variable $callback @L232
          Method userResolver @L248
          Method resolveUsersUsing @L259
            Variable $userResolver @L259
          Method extend @L276
            Variable $driver @L276
            Variable $callback @L276
          Method provider @L296
            Variable $name @L296
            Variable $callback @L296
          Method hasResolvedGuards @L308
          Method forgetGuards @L318
          Method setApplication @L331
            Variable $app @L331
          Method __call @L345
            Variable $method @L345
            Variable $parameters @L345"#]]
    .assert_eq(&out);
}

/// `documentSymbol` for the Eloquent Model (a large file with ~200 members)
/// completes without timeout and returns all members.
/// Guards against large-file document symbol requests stalling.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_document_symbols_large_file() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Database/Eloquent/Model.php",
        &read("Illuminate/Database/Eloquent/Model.php"),
    )
    .await;

    let resp = s
        .document_symbols("Illuminate/Database/Eloquent/Model.php")
        .await;
    let out = render_document_symbols(&resp);
    expect![[r#"
        Class Model @L41
          Property $connection @L62
          Property $table @L69
          Property $primaryKey @L76
          Property $keyType @L83
          Property $incrementing @L90
          Property $with @L97
          Property $withCount @L104
          Property $preventsLazyLoading @L111
          Property $perPage @L118
          Property $exists @L125
          Property $wasRecentlyCreated @L132
          Property $escapeWhenCastingToString @L139
          Property $resolver @L146
          Property $dispatcher @L153
          Property $booting @L160
          Property $booted @L167
          Property $bootedCallbacks @L174
          Property $traitInitializers @L181
          Property $globalScopes @L188
          Property $ignoreOnTouch @L195
          Property $modelsShouldPreventLazyLoading @L202
          Property $modelsShouldAutomaticallyEagerLoadRelationships @L209
          Property $lazyLoadingViolationCallback @L216
          Property $modelsShouldPreventSilentlyDiscardingAttributes @L223
          Property $discardedAttributeViolationCallback @L230
          Property $modelsShouldPreventAccessingMissingAttributes @L237
          Property $missingAttributeViolationCallback @L244
          Property $isBroadcasting @L251
          Property $builder @L258
          Property $collectionClass @L265
          Property $isSoftDeletable @L272
          Property $isPrunable @L279
          Property $isMassPrunable @L286
          Property $classAttributes @L293
          Constant CREATED_AT @L300
          Constant UPDATED_AT @L307
          Method __construct @L314
            Variable $attributes @L314
          Method bootIfNotBooted @L330
          Method booting @L364
          Method boot @L374
          Method bootTraits @L384
          Method initializeTraits @L421
          Method initializeModelAttributes @L433
          Method booted @L470
          Method whenBooted @L481
            Variable $callback @L481
          Method clearBootedModels @L493
          Method withoutTouching @L507
            Variable $callback @L507
          Method withoutTouchingOn @L519
            Variable $models @L519
            Variable $callback @L519
          Method isIgnoringTouch @L536
            Variable $class @L536
          Method shouldBeStrict @L566
            Variable $shouldBeStrict @L566
          Method preventLazyLoading @L579
            Variable $value @L579
          Method automaticallyEagerLoadRelationships @L590
            Variable $value @L590
          Method handleLazyLoadingViolationUsing @L601
            Variable $callback @L601
          Method preventSilentlyDiscardingAttributes @L612
            Variable $value @L612
          Method handleDiscardedAttributeViolationUsing @L623
            Variable $callback @L623
          Method preventAccessingMissingAttributes @L634
            Variable $value @L634
          Method handleMissingAttributeViolationUsing @L645
            Variable $callback @L645
          Method withoutBroadcasting @L658
            Variable $callback @L658
          Method fill @L679
            Variable $attributes @L679
          Method forceFill @L727
            Variable $attributes @L727
          Method qualifyColumn @L738
            Variable $column @L738
          Method qualifyColumns @L753
            Variable $columns @L753
          Method newInstance @L767
            Variable $attributes @L767
            Variable $exists @L767
          Method newFromBuilder @L796
            Variable $attributes @L796
            Variable $connection @L796
          Method on @L815
            Variable $connection @L815
          Method onWriteConnection @L828
          Method all @L839
            Variable $columns @L839
          Method with @L852
            Variable $relations @L852
          Method load @L865
            Variable $relations @L865
          Method loadMorph @L883
            Variable $relation @L883
            Variable $relations @L883
          Method loadMissing @L902
            Variable $relations @L902
          Method loadAggregate @L919
            Variable $relations @L919
            Variable $column @L919
            Variable $function @L919
          Method loadCount @L932
            Variable $relations @L932
          Method loadMax @L946
            Variable $relations @L946
            Variable $column @L946
          Method loadMin @L958
            Variable $relations @L958
            Variable $column @L958
          Method loadSum @L970
            Variable $relations @L970
            Variable $column @L970
          Method loadAvg @L982
            Variable $relations @L982
            Variable $column @L982
          Method loadExists @L993
            Variable $relations @L993
          Method loadMorphAggregate @L1007
            Variable $relation @L1007
            Variable $relations @L1007
            Variable $column @L1007
            Variable $function @L1007
          Method loadMorphCount @L1027
            Variable $relation @L1027
            Variable $relations @L1027
          Method loadMorphMax @L1040
            Variable $relation @L1040
            Variable $relations @L1040
            Variable $column @L1040
          Method loadMorphMin @L1053
            Variable $relation @L1053
            Variable $relations @L1053
            Variable $column @L1053
          Method loadMorphSum @L1066
            Variable $relation @L1066
            Variable $relations @L1066
            Variable $column @L1066
          Method loadMorphAvg @L1079
            Variable $relation @L1079
            Variable $relations @L1079
            Variable $column @L1079
          Method increment @L1092
            Variable $column @L1092
            Variable $amount @L1092
            Variable $extra @L1092
          Method decrement @L1105
            Variable $column @L1105
            Variable $amount @L1105
            Variable $extra @L1105
          Method incrementOrDecrement @L1119
            Variable $column @L1119
            Variable $amount @L1119
            Variable $extra @L1119
            Variable $method @L1119
          Method update @L1155
            Variable $attributes @L1155
            Variable $options @L1155
          Method updateOrFail @L1173
            Variable $attributes @L1173
            Variable $options @L1173
          Method updateQuietly @L1189
            Variable $attributes @L1189
            Variable $options @L1189
          Method incrementQuietly @L1206
            Variable $column @L1206
            Variable $amount @L1206
            Variable $extra @L1206
          Method decrementQuietly @L1221
            Variable $column @L1221
            Variable $amount @L1221
            Variable $extra @L1221
          Method incrementEach @L1235
            Variable $columns @L1235
            Variable $extra @L1235
          Method decrementEach @L1247
            Variable $columns @L1247
            Variable $extra @L1247
          Method incrementOrDecrementEach @L1260
            Variable $columns @L1260
            Variable $extra @L1260
            Variable $method @L1260
          Method push @L1303
          Method pushQuietly @L1334
          Method saveQuietly @L1345
            Variable $options @L1345
          Method save @L1356
            Variable $options @L1356
          Method saveOrIgnore @L1406
            Variable $options @L1406
            Variable $uniqueBy @L1406
          Method saveOrFail @L1442
            Variable $options @L1442
          Method finishSave @L1453
            Variable $options @L1453
          Method performUpdate @L1470
            Variable $query @L1470
          Method setKeysForSelectQuery @L1508
            Variable $query @L1508
          Method getKeyForSelectQuery @L1520
          Method setKeysForSaveQuery @L1531
            Variable $query @L1531
          Method getKeyForSaveQuery @L1543
          Method performInsert @L1554
            Variable $query @L1554
          Method performInsertOrIgnore @L1610
            Variable $query @L1610
            Variable $uniqueBy @L1610
          Method insertAndSetId @L1659
            Variable $query @L1659
            Variable $attributes @L1659
          Method destroy @L1672
            Variable $ids @L1672
          Method delete @L1711
          Method deleteQuietly @L1750
          Method deleteOrFail @L1762
          Method forceDelete @L1778
          Method forceDestroy @L1791
            Variable $ids @L1791
          Method performDeleteOnModel @L1801
          Method query @L1813
          Method newQuery @L1823
          Method newModelQuery @L1833
          Method newQueryWithoutRelationships @L1845
          Method registerGlobalScopes @L1856
            Variable $builder @L1856
          Method newQueryWithoutScopes @L1870
          Method newQueryWithoutScope @L1883
            Variable $scope @L1883
          Method newQueryForRestoration @L1894
            Variable $ids @L1894
          Method newEloquentBuilder @L1905
            Variable $query @L1905
          Method resolveCustomBuilderClass @L1921
          Method newBaseQueryBuilder @L1936
          Method newPivot @L1951
            Variable $parent @L1951
            Variable $attributes @L1951
            Variable $table @L1951
            Variable $exists @L1951
            Variable $using @L1951
          Method hasNamedScope @L1963
            Variable $scope @L1963
          Method callNamedScope @L1976
            Variable $scope @L1976
            Variable $parameters @L1976
          Method isScopeMethodWithAttribute @L1991
            Variable $method @L1991
          Method toArray @L2007
          Method toJson @L2023
            Variable $options @L2023
          Method toPrettyJson @L2042
            Variable $options @L2042
          Method jsonSerialize @L2052
          Method fresh @L2063
            Variable $with @L2063
          Method refresh @L2080
          Method replicate @L2109
            Variable $except @L2109
          Method replicateQuietly @L2138
            Variable $except @L2138
          Method is @L2149
            Variable $model @L2149
          Method isNot @L2163
            Variable $model @L2163
          Method getConnection @L2173
          Method getConnectionName @L2183
          Method setConnection @L2194
            Variable $name @L2194
          Method resolveConnection @L2207
            Variable $connection @L2207
          Method getConnectionResolver @L2217
          Method setConnectionResolver @L2228
            Variable $resolver @L2228
          Method unsetConnectionResolver @L2238
          Method getTable @L2248
          Method setTable @L2259
            Variable $table @L2259
          Method getKeyName @L2271
          Method setKeyName @L2282
            Variable $key @L2282
          Method getQualifiedKeyName @L2294
          Method getKeyType @L2304
          Method setKeyType @L2315
            Variable $type @L2315
          Method getIncrementing @L2327
          Method setIncrementing @L2338
            Variable $value @L2338
          Method getKey @L2350
          Method getQueueableId @L2360
          Method getQueueableRelations @L2370
          Method getQueueableConnection @L2404
          Method getRouteKey @L2414
          Method getRouteKeyName @L2424
          Method resolveRouteBinding @L2436
            Variable $value @L2436
            Variable $field @L2436
          Method resolveSoftDeletableRouteBinding @L2448
            Variable $value @L2448
            Variable $field @L2448
          Method resolveChildRouteBinding @L2461
            Variable $childType @L2461
            Variable $value @L2461
            Variable $field @L2461
          Method resolveSoftDeletableChildRouteBinding @L2474
            Variable $childType @L2474
            Variable $value @L2474
            Variable $field @L2474
          Method resolveChildRouteBindingQuery @L2487
            Variable $childType @L2487
            Variable $value @L2487
            Variable $field @L2487
          Method childRouteBindingRelationshipName @L2509
            Variable $childType @L2509
          Method resolveRouteBindingQuery @L2522
            Variable $query @L2522
            Variable $value @L2522
            Variable $field @L2522
          Method getForeignKey @L2532
          Method getPerPage @L2542
          Method setPerPage @L2553
            Variable $perPage @L2553
          Method isSoftDeletable @L2563
          Method isPrunable @L2571
          Method isMassPrunable @L2579
          Method preventsLazyLoading @L2589
          Method isAutomaticallyEagerLoadingRelationships @L2599
          Method preventsSilentlyDiscardingAttributes @L2609
          Method preventsAccessingMissingAttributes @L2619
          Method broadcastChannelRoute @L2629
          Method broadcastChannel @L2639
          Method resolveClassAttribute @L2654
            Variable $attributeClass @L2654
            Variable $property @L2654
            Variable $class @L2654
          Method __get @L2689
            Variable $key @L2689
          Method __set @L2701
            Variable $key @L2701
            Variable $value @L2701
          Method offsetExists @L2712
            Variable $offset @L2712
          Method offsetGet @L2731
            Variable $offset @L2731
          Method offsetSet @L2743
            Variable $offset @L2743
            Variable $value @L2743
          Method offsetUnset @L2754
            Variable $offset @L2754
          Method __isset @L2770
            Variable $key @L2770
          Method __unset @L2781
            Variable $key @L2781
          Method __call @L2793
            Variable $method @L2793
            Variable $parameters @L2793
          Method __callStatic @L2818
            Variable $method @L2818
            Variable $parameters @L2818
          Method __toString @L2832
          Method escapeWhenCastingToString @L2845
            Variable $escape @L2845
          Method __sleep @L2857
          Method __wakeup @L2884"#]]
    .assert_eq(&out);
}

// ── Workspace Symbols ─────────────────────────────────────────────────────────

/// `workspace/symbol` for "AuthManager" resolves after the index completes.
/// Guards against workspace symbol returning empty on a real codebase.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_workspace_symbols_class_name() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;

    let resp = s.workspace_symbols("AuthManager").await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    let out = render_workspace_symbols(&resp, &s.uri(""));
    expect!["Class       AuthManager @ Illuminate/Auth/AuthManager.php:17"].assert_eq(&out);
}

/// `workspace/symbol` for "Guard" returns multiple guard-related symbols.
/// Guards against symbol search being too restrictive.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_workspace_symbols_partial_query() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;

    let resp = s.workspace_symbols("Guard").await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    let out = render_workspace_symbols(&resp, &s.uri(""));
    expect![[r#"
        Class       FrameGuard @ Illuminate/Http/Middleware/FrameGuard.php:6
        Class       GuardHelpers @ Illuminate/Auth/GuardHelpers.php:10
        Class       Guarded @ Illuminate/Database/Eloquent/Attributes/Guarded.php:7
        Class       GuardsAttributes @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:9
        Class       RequestGuard @ Illuminate/Auth/RequestGuard.php:9
        Class       SessionGuard @ Illuminate/Auth/SessionGuard.php:29
        Class       TokenGuard @ Illuminate/Auth/TokenGuard.php:9
        Class       Unguarded @ Illuminate/Database/Eloquent/Attributes/Unguarded.php:7
        Interface   Guard @ Illuminate/Contracts/Auth/Guard.php:4
        Interface   StatefulGuard @ Illuminate/Contracts/Auth/StatefulGuard.php:4
        Method      forgetGuards @ Illuminate/Auth/AuthManager.php:318
        Method      getGuarded @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:103
        Method      guard @ Illuminate/Auth/AuthManager.php:69
        Method      guard @ Illuminate/Contracts/Auth/Factory.php:12
        Method      guard @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:116
        Method      guard @ Illuminate/Session/Middleware/AuthenticateSession.php:142
        Method      guards @ Illuminate/Auth/AuthenticationException.php:50
        Method      hasResolvedGuards @ Illuminate/Auth/AuthManager.php:308
        Method      initializeGuardsAttributes @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:44
        Method      isGuardableColumn @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:244
        Method      isGuarded @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:227
        Method      isGuardedChannel @ Illuminate/Broadcasting/Broadcasters/AblyBroadcaster.php:161
        Method      isGuardedChannel @ Illuminate/Broadcasting/Broadcasters/UsePusherChannelConventions.php:14
        Method      isUnguarded @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:162
        Method      mergeGuarded @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:129
        Method      reguard @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:152
        Method      totallyGuarded @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:270
        Method      unguard @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:142
        Method      unguarded @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:175
        Property    $guard @ Illuminate/Auth/Events/Attempting.php:14
        Property    $guard @ Illuminate/Auth/Events/Authenticated.php:17
        Property    $guard @ Illuminate/Auth/Events/CurrentDeviceLogout.php:17
        Property    $guard @ Illuminate/Auth/Events/Failed.php:14
        Property    $guard @ Illuminate/Auth/Events/Login.php:18
        Property    $guard @ Illuminate/Auth/Events/Logout.php:17
        Property    $guard @ Illuminate/Auth/Events/OtherDeviceLogout.php:17
        Property    $guard @ Illuminate/Auth/Events/Validated.php:17
        Property    $guard @ Illuminate/Container/Attributes/Auth.php:15
        Property    $guard @ Illuminate/Container/Attributes/Authenticated.php:15
        Property    $guardableColumns @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:37
        Property    $guarded @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:23
        Property    $guarded @ Illuminate/Database/Eloquent/Relations/Pivot.php:23
        Property    $guarded @ Illuminate/Notifications/DatabaseNotification.php:39
        Property    $guards @ Illuminate/Auth/AuthManager.php:40
        Property    $guards @ Illuminate/Auth/AuthenticationException.php:14
        Property    $unguarded @ Illuminate/Database/Eloquent/Concerns/GuardsAttributes.php:30"#]].assert_eq(&out);
}

/// `workspace/symbol` for the `Str` class returns the class from Support.
/// Guards against workspace symbols missing commonly-used utility classes.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_workspace_symbols_str_class() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;

    let resp = s.workspace_symbols("Str").await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    let out = render_workspace_symbols(&resp, &s.uri(""));
    expect![[r#"
        Class       AbstractCursorPaginator @ Illuminate/Pagination/AbstractCursorPaginator.php:27
        Class       AbstractHasher @ Illuminate/Hashing/AbstractHasher.php:4
        Class       AbstractPaginator @ Illuminate/Pagination/AbstractPaginator.php:22
        Class       AbstractRouteCollection @ Illuminate/Routing/AbstractRouteCollection.php:17
        Class       AsHtmlString @ Illuminate/Database/Eloquent/Casts/AsHtmlString.php:8
        Class       AsStringable @ Illuminate/Database/Eloquent/Casts/AsStringable.php:8
        Class       AssertableJsonString @ Illuminate/Testing/AssertableJsonString.php:16
        Class       CompilesTranslations @ Illuminate/View/Compilers/Concerns/CompilesTranslations.php:4
        Class       ConvertEmptyStringsToNull @ Illuminate/Foundation/Http/Middleware/ConvertEmptyStringsToNull.php:6
        Class       CreatesPotentiallyTranslatedStrings @ Illuminate/Translation/CreatesPotentiallyTranslatedStrings.php:4
        Class       CreatesRegularExpressionRouteConstraints @ Illuminate/Routing/CreatesRegularExpressionRouteConstraints.php:8
        Class       EncodedHtmlString @ Illuminate/Support/EncodedHtmlString.php:8
        Class       HasUniqueStringIds @ Illuminate/Database/Eloquent/Concerns/HasUniqueStringIds.php:6
        Class       HtmlString @ Illuminate/Support/HtmlString.php:7
        Class       ManagesTransactions @ Illuminate/Database/Concerns/ManagesTransactions.php:12
        Class       ManagesTranslations @ Illuminate/View/Concerns/ManagesTranslations.php:4
        Class       PendingResourceRegistration @ Illuminate/Routing/PendingResourceRegistration.php:7
        Class       PendingSingletonResourceRegistration @ Illuminate/Routing/PendingSingletonResourceRegistration.php:7
        Class       PotentiallyTranslatedString @ Illuminate/Translation/PotentiallyTranslatedString.php:6
        Class       ResourceRegistrar @ Illuminate/Routing/ResourceRegistrar.php:6
        Class       RouteFileRegistrar @ Illuminate/Routing/RouteFileRegistrar.php:4
        Class       RouteRegistrar @ Illuminate/Routing/RouteRegistrar.php:35
        Class       SesTransport @ Illuminate/Mail/Transport/SesTransport.php:14
        Class       Str @ Illuminate/Support/Str.php:22
        Class       StrayRequestException @ Illuminate/Http/Client/StrayRequestException.php:6
        Class       StreamedEvent @ Illuminate/Http/StreamedEvent.php:4
        Class       StreamedResponseException @ Illuminate/Routing/Exceptions/StreamedResponseException.php:8
        Class       StringRule @ Illuminate/Validation/Rules/StringRule.php:8
        Class       StringType @ Illuminate/JsonSchema/Types/StringType.php:4
        Class       Stringable @ Illuminate/Support/Stringable.php:14
        Class       TestResponse @ Illuminate/Testing/TestResponse.php:36
        Class       TestResponseAssert @ Illuminate/Testing/TestResponseAssert.php:14
        Class       TrimStrings @ Illuminate/Foundation/Http/Middleware/TrimStrings.php:8
        Class       UniqueConstraintViolationException @ Illuminate/Database/UniqueConstraintViolationException.php:4
        Class       UriQueryString @ Illuminate/Support/UriQueryString.php:9
        Interface   BindingRegistrar @ Illuminate/Contracts/Routing/BindingRegistrar.php:4
        Interface   CanBeEscapedWhenCastToString @ Illuminate/Contracts/Support/CanBeEscapedWhenCastToString.php:4
        Interface   Registrar @ Illuminate/Contracts/Routing/Registrar.php:4
        Interface   StringEncrypter @ Illuminate/Contracts/Encryption/StringEncrypter.php:4
        Method      __construct @ Illuminate/Auth/Access/AuthorizationException.php:30
        Method      __construct @ Illuminate/Auth/Access/Events/GateEvaluated.php:42
        Method      __construct @ Illuminate/Auth/Access/Gate.php:98
        Method      __construct @ Illuminate/Auth/Access/Response.php:44
        Method      __construct @ Illuminate/Auth/AuthManager.php:56
        Method      __construct @ Illuminate/Auth/AuthenticationException.php:37
        Method      __construct @ Illuminate/Auth/DatabaseUserProvider.php:41
        Method      __construct @ Illuminate/Auth/EloquentUserProvider.php:39
        Method      __construct @ Illuminate/Auth/Events/Attempting.php:13
        Method      __construct @ Illuminate/Auth/Events/Authenticated.php:16
        Method      __construct @ Illuminate/Auth/Events/CurrentDeviceLogout.php:16
        Method      __construct @ Illuminate/Auth/Events/Failed.php:13
        Method      __construct @ Illuminate/Auth/Events/Lockout.php:20
        Method      __construct @ Illuminate/Auth/Events/Login.php:17
        Method      __construct @ Illuminate/Auth/Events/Logout.php:16
        Method      __construct @ Illuminate/Auth/Events/OtherDeviceLogout.php:16
        Method      __construct @ Illuminate/Auth/Events/PasswordReset.php:15
        Method      __construct @ Illuminate/Auth/Events/PasswordResetLinkSent.php:15
        Method      __construct @ Illuminate/Auth/Events/Registered.php:15
        Method      __construct @ Illuminate/Auth/Events/Validated.php:16
        Method      __construct @ Illuminate/Auth/Events/Verified.php:15
        Method      __construct @ Illuminate/Auth/GenericUser.php:20
        Method      __construct @ Illuminate/Auth/Middleware/Authenticate.php:31
        Method      __construct @ Illuminate/Auth/Middleware/AuthenticateWithBasicAuth.php:21
        Method      __construct @ Illuminate/Auth/Middleware/Authorize.php:25
        Method      __construct @ Illuminate/Auth/Middleware/RequirePassword.php:39
        Method      __construct @ Illuminate/Auth/Notifications/ResetPassword.php:36
        Method      __construct @ Illuminate/Auth/Passwords/CacheTokenRepository.php:20
        Method      __construct @ Illuminate/Auth/Passwords/DatabaseTokenRepository.php:18
        Method      __construct @ Illuminate/Auth/Passwords/PasswordBroker.php:60
        Method      __construct @ Illuminate/Auth/Passwords/PasswordBrokerManager.php:33
        Method      __construct @ Illuminate/Auth/Recaller.php:18
        Method      __construct @ Illuminate/Auth/RequestGuard.php:34
        Method      __construct @ Illuminate/Auth/SessionGuard.php:145
        Method      __construct @ Illuminate/Auth/TokenGuard.php:50
        Method      __construct @ Illuminate/Broadcasting/AnonymousEvent.php:42
        Method      __construct @ Illuminate/Broadcasting/BroadcastEvent.php:72
        Method      __construct @ Illuminate/Broadcasting/BroadcastManager.php:67
        Method      __construct @ Illuminate/Broadcasting/Broadcasters/AblyBroadcaster.php:29
        Method      __construct @ Illuminate/Broadcasting/Broadcasters/LogBroadcaster.php:20
        Method      __construct @ Illuminate/Broadcasting/Broadcasters/PusherBroadcaster.php:34
        Method      __construct @ Illuminate/Broadcasting/Broadcasters/RedisBroadcaster.php:47
        Method      __construct @ Illuminate/Broadcasting/Channel.php:21
        Method      __construct @ Illuminate/Broadcasting/EncryptedPrivateChannel.php:11
        Method      __construct @ Illuminate/Broadcasting/FakePendingBroadcast.php:9
        Method      __construct @ Illuminate/Broadcasting/PendingBroadcast.php:30
        Method      __construct @ Illuminate/Broadcasting/PresenceChannel.php:11
        Method      __construct @ Illuminate/Broadcasting/PrivateChannel.php:13
        Method      __construct @ Illuminate/Broadcasting/UniqueBroadcastEvent.php:29
        Method      __construct @ Illuminate/Bus/Batch.php:108
        Method      __construct @ Illuminate/Bus/BatchFactory.php:21
        Method      __construct @ Illuminate/Bus/ChainedBatch.php:42
        Method      __construct @ Illuminate/Bus/DatabaseBatchRepository.php:43
        Method      __construct @ Illuminate/Bus/DebounceLock.php:26
        Method      __construct @ Illuminate/Bus/Dispatcher.php:70
        Method      __construct @ Illuminate/Bus/DynamoBatchRepository.php:64
        Method      __construct @ Illuminate/Bus/Events/BatchCanceled.php:15
        Method      __construct @ Illuminate/Bus/Events/BatchDispatched.php:13
        Method      __construct @ Illuminate/Bus/Events/BatchFinished.php:13
        Method      __construct @ Illuminate/Bus/Events/BatchStarted.php:13
        Method      __construct @ Illuminate/Bus/PendingBatch.php:63
        Method      __construct @ Illuminate/Bus/UniqueLock.php:24
        Method      __construct @ Illuminate/Bus/UpdatedBatchJobCounts.php:26
        Method      __construct @ Illuminate/Cache/ApcStore.php:28
        Method      __construct @ Illuminate/Cache/ArrayLock.php:23
        Method      __construct @ Illuminate/Cache/ArrayStore.php:49
        Method      __construct @ Illuminate/Cache/CacheLock.php:21
        Method      __construct @ Illuminate/Cache/CacheManager.php:53
        Method      __construct @ Illuminate/Cache/Console/ClearCommand.php:52
        Method      __construct @ Illuminate/Cache/Console/ForgetCommand.php:37
        Method      __construct @ Illuminate/Cache/DatabaseLock.php:52
        Method      __construct @ Illuminate/Cache/DatabaseStore.php:90
        Method      __construct @ Illuminate/Cache/DynamoDbLock.php:21
        Method      __construct @ Illuminate/Cache/DynamoDbStore.php:44
        Method      __construct @ Illuminate/Cache/Events/CacheEvent.php:34
        Method      __construct @ Illuminate/Cache/Events/CacheFailedOver.php:14
        Method      __construct @ Illuminate/Cache/Events/CacheFlushFailed.php:26
        Method      __construct @ Illuminate/Cache/Events/CacheFlushed.php:26
        Method      __construct @ Illuminate/Cache/Events/CacheFlushing.php:26
        Method      __construct @ Illuminate/Cache/Events/CacheHit.php:21
        Method      __construct @ Illuminate/Cache/Events/CacheLocksFlushFailed.php:18
        Method      __construct @ Illuminate/Cache/Events/CacheLocksFlushed.php:18
        Method      __construct @ Illuminate/Cache/Events/CacheLocksFlushing.php:18
        Method      __construct @ Illuminate/Cache/Events/KeyWriteFailed.php:29
        Method      __construct @ Illuminate/Cache/Events/KeyWritten.php:29
        Method      __construct @ Illuminate/Cache/Events/RetrievingManyKeys.php:20
        Method      __construct @ Illuminate/Cache/Events/WritingKey.php:29
        Method      __construct @ Illuminate/Cache/Events/WritingManyKeys.php:36
        Method      __construct @ Illuminate/Cache/FailoverStore.php:25
        Method      __construct @ Illuminate/Cache/FileStore.php:61
        Method      __construct @ Illuminate/Cache/Limiters/ConcurrencyLimiter.php:46
        Method      __construct @ Illuminate/Cache/Limiters/ConcurrencyLimiterBuilder.php:58
        Method      __construct @ Illuminate/Cache/Lock.php:51
        Method      __construct @ Illuminate/Cache/MemcachedLock.php:21
        Method      __construct @ Illuminate/Cache/MemcachedStore.php:40
        Method      __construct @ Illuminate/Cache/MemoizedStore.php:24
        Method      __construct @ Illuminate/Cache/PhpRedisLock.php:16
        Method      __construct @ Illuminate/Cache/RateLimiter.php:35
        Method      __construct @ Illuminate/Cache/RateLimiting/GlobalLimit.php:12
        Method      __construct @ Illuminate/Cache/RateLimiting/Limit.php:48
        Method      __construct @ Illuminate/Cache/RateLimiting/Unlimited.php:9
        Method      __construct @ Illuminate/Cache/RedisLock.php:21
        Method      __construct @ Illuminate/Cache/RedisStore.php:65
        Method      __construct @ Illuminate/Cache/Repository.php:94
        Method      __construct @ Illuminate/Cache/SessionStore.php:32
        Method      __construct @ Illuminate/Cache/StorageStore.php:49
        Method      __construct @ Illuminate/Cache/TagSet.php:28
        Method      __construct @ Illuminate/Cache/TaggedCache.php:29
        Method      __construct @ Illuminate/Collections/Collection.php:42
        Method      __construct @ Illuminate/Collections/HigherOrderCollectionProxy.php:34
        Method      __construct @ Illuminate/Collections/LazyCollection.php:46
        Method      __construct @ Illuminate/Collections/MultipleItemsFoundException.php:22
        Method      __construct @ Illuminate/Concurrency/ProcessDriver.php:22
        Method      __construct @ Illuminate/Conditionable/HigherOrderWhenProxy.php:39
        Method      __construct @ Illuminate/Config/Repository.php:27
        Method      __construct @ Illuminate/Console/Application.php:68
        Method      __construct @ Illuminate/Console/Attributes/Aliases.php:14
        Method      __construct @ Illuminate/Console/Attributes/Description.php:14
        Method      __construct @ Illuminate/Console/Attributes/Help.php:14
        Method      __construct @ Illuminate/Console/Attributes/Hidden.php:12
        Method      __construct @ Illuminate/Console/Attributes/Signature.php:15
        Method      __construct @ Illuminate/Console/Attributes/Usage.php:14
        Method      __construct @ Illuminate/Console/CacheCommandMutex.php:33
        Method      __construct @ Illuminate/Console/Command.php:96
        Method      __construct @ Illuminate/Console/ContainerCommandLoader.php:31
        Method      __construct @ Illuminate/Console/Events/ArtisanStarting.php:13
        Method      __construct @ Illuminate/Console/Events/CommandFinished.php:17
        Method      __construct @ Illuminate/Console/Events/CommandStarting.php:16
        Method      __construct @ Illuminate/Console/Events/ScheduledBackgroundTaskFinished.php:13
        Method      __construct @ Illuminate/Console/Events/ScheduledTaskFailed.php:15
        Method      __construct @ Illuminate/Console/Events/ScheduledTaskFinished.php:14
        Method      __construct @ Illuminate/Console/Events/ScheduledTaskSkipped.php:13
        Method      __construct @ Illuminate/Console/Events/ScheduledTaskStarting.php:13
        Method      __construct @ Illuminate/Console/GeneratorCommand.php:128
        Method      __construct @ Illuminate/Console/MigrationGeneratorCommand.php:22
        Method      __construct @ Illuminate/Console/OutputStyle.php:43
        Method      __construct @ Illuminate/Console/Scheduling/CacheEventMutex.php:29
        Method      __construct @ Illuminate/Console/Scheduling/CacheSchedulingMutex.php:30
        Method      __construct @ Illuminate/Console/Scheduling/CallbackEvent.php:51
        Method      __construct @ Illuminate/Console/Scheduling/Event.php:107
        Method      __construct @ Illuminate/Console/Scheduling/PendingEventAttributes.php:50
        Method      __construct @ Illuminate/Console/Scheduling/Schedule.php:124
        Method      __construct @ Illuminate/Console/Scheduling/ScheduleInterruptCommand.php:38
        Method      __construct @ Illuminate/Console/Scheduling/ScheduleRunCommand.php:89
        Method      __construct @ Illuminate/Console/Signals.php:35
        Method      __construct @ Illuminate/Console/View/Components/Component.php:33
        Method      __construct @ Illuminate/Console/View/Components/Factory.php:36
        Method      __construct @ Illuminate/Container/Attributes/Auth.php:15
        Method      __construct @ Illuminate/Container/Attributes/Authenticated.php:15
        Method      __construct @ Illuminate/Container/Attributes/Bind.php:35
        Method      __construct @ Illuminate/Container/Attributes/Cache.php:15
        Method      __construct @ Illuminate/Container/Attributes/Config.php:14
        Method      __construct @ Illuminate/Container/Attributes/Context.php:15
        Method      __construct @ Illuminate/Container/Attributes/Database.php:15
        Method      __construct @ Illuminate/Container/Attributes/Give.php:17
        Method      __construct @ Illuminate/Container/Attributes/Log.php:20
        Method      __construct @ Illuminate/Container/Attributes/RouteParameter.php:15
        Method      __construct @ Illuminate/Container/Attributes/Storage.php:15
        Method      __construct @ Illuminate/Container/Attributes/Tag.php:13
        Method      __construct @ Illuminate/Container/ContextualBindingBuilder.php:36
        Method      __construct @ Illuminate/Container/RewindableGenerator.php:30
        Method      __construct @ Illuminate/Contracts/Database/ModelIdentifier.php:58
        Method      __construct @ Illuminate/Contracts/Queue/EntityNotFoundException.php:14
        Method      __construct @ Illuminate/Cookie/Middleware/AddQueuedCookiesToResponse.php:21
        Method      __construct @ Illuminate/Cookie/Middleware/EncryptCookies.php:48
        Method      __construct @ Illuminate/Database/Capsule/Manager.php:28
        Method      __construct @ Illuminate/Database/ClassMorphViolationException.php:20
        Method      __construct @ Illuminate/Database/Connection.php:241
        Method      __construct @ Illuminate/Database/ConnectionResolver.php:25
        Method      __construct @ Illuminate/Database/Connectors/ConnectionFactory.php:32
        Method      __construct @ Illuminate/Database/Console/Migrations/FreshCommand.php:45
        Method      __construct @ Illuminate/Database/Console/Migrations/InstallCommand.php:38
        Method      __construct @ Illuminate/Database/Console/Migrations/MigrateCommand.php:67
        Method      __construct @ Illuminate/Database/Console/Migrations/MigrateMakeCommand.php:54
        Method      __construct @ Illuminate/Database/Console/Migrations/ResetCommand.php:42
        Method      __construct @ Illuminate/Database/Console/Migrations/RollbackCommand.php:42
        Method      __construct @ Illuminate/Database/Console/Migrations/StatusCommand.php:39
        Method      __construct @ Illuminate/Database/Console/MonitorCommand.php:49
        Method      __construct @ Illuminate/Database/Console/Seeds/SeedCommand.php:44
        Method      __construct @ Illuminate/Database/DatabaseManager.php:75
        Method      __construct @ Illuminate/Database/DatabaseTransactionRecord.php:48
        Method      __construct @ Illuminate/Database/DatabaseTransactionsManager.php:32
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/Appends.php:19
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/CollectedBy.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/Connection.php:15
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/DateFormat.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/Fillable.php:19
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/Guarded.php:19
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/Hidden.php:19
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/ObservedBy.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/Scope.php:12
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/ScopedBy.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/Table.php:19
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/Touches.php:19
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/UseEloquentBuilder.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/UseFactory.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/UsePolicy.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/UseResource.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/UseResourceCollection.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/Visible.php:19
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/WithoutIncrementing.php:12
        Method      __construct @ Illuminate/Database/Eloquent/Attributes/WithoutTimestamps.php:12
        Method      __construct @ Illuminate/Database/Eloquent/BroadcastableModelEventOccurred.php:62
        Method      __construct @ Illuminate/Database/Eloquent/Builder.php:173
        Method      __construct @ Illuminate/Database/Eloquent/Casts/Attribute.php:40
        Method      __construct @ Illuminate/Database/Eloquent/Factories/Attributes/UseModel.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Factories/BelongsToManyRelationship.php:37
        Method      __construct @ Illuminate/Database/Eloquent/Factories/BelongsToRelationship.php:36
        Method      __construct @ Illuminate/Database/Eloquent/Factories/CrossJoinSequence.php:13
        Method      __construct @ Illuminate/Database/Eloquent/Factories/Factory.php:175
        Method      __construct @ Illuminate/Database/Eloquent/Factories/Relationship.php:31
        Method      __construct @ Illuminate/Database/Eloquent/Factories/Sequence.php:34
        Method      __construct @ Illuminate/Database/Eloquent/HigherOrderBuilderProxy.php:29
        Method      __construct @ Illuminate/Database/Eloquent/InvalidCastException.php:36
        Method      __construct @ Illuminate/Database/Eloquent/MissingAttributeException.php:14
        Method      __construct @ Illuminate/Database/Eloquent/Model.php:314
        Method      __construct @ Illuminate/Database/Eloquent/ModelInfo.php:31
        Method      __construct @ Illuminate/Database/Eloquent/ModelInspector.php:42
        Method      __construct @ Illuminate/Database/Eloquent/PendingHasThroughRelationship.php:37
        Method      __construct @ Illuminate/Database/Eloquent/Relations/BelongsTo.php:62
        Method      __construct @ Illuminate/Database/Eloquent/Relations/BelongsToMany.php:159
        Method      __construct @ Illuminate/Database/Eloquent/Relations/HasOneOrMany.php:46
        Method      __construct @ Illuminate/Database/Eloquent/Relations/HasOneOrManyThrough.php:80
        Method      __construct @ Illuminate/Database/Eloquent/Relations/MorphOneOrMany.php:40
        Method      __construct @ Illuminate/Database/Eloquent/Relations/MorphTo.php:87
        Method      __construct @ Illuminate/Database/Eloquent/Relations/MorphToMany.php:56
        Method      __construct @ Illuminate/Database/Eloquent/Relations/Relation.php:91
        Method      __construct @ Illuminate/Database/Events/ConnectionEvent.php:25
        Method      __construct @ Illuminate/Database/Events/DatabaseBusy.php:12
        Method      __construct @ Illuminate/Database/Events/DatabaseRefreshed.php:14
        Method      __construct @ Illuminate/Database/Events/MigrationEvent.php:37
        Method      __construct @ Illuminate/Database/Events/MigrationSkipped.php:13
        Method      __construct @ Illuminate/Database/Events/MigrationsEvent.php:14
        Method      __construct @ Illuminate/Database/Events/MigrationsPruned.php:35
        Method      __construct @ Illuminate/Database/Events/ModelPruningFinished.php:11
        Method      __construct @ Illuminate/Database/Events/ModelPruningStarting.php:11
        Method      __construct @ Illuminate/Database/Events/ModelsPruned.php:12
        Method      __construct @ Illuminate/Database/Events/NoPendingMigrations.php:13
        Method      __construct @ Illuminate/Database/Events/QueryExecuted.php:57
        Method      __construct @ Illuminate/Database/Events/SchemaDumped.php:33
        Method      __construct @ Illuminate/Database/Events/SchemaLoaded.php:33
        Method      __construct @ Illuminate/Database/Events/StatementPrepared.php:12
        Method      __construct @ Illuminate/Database/Grammar.php:25
        Method      __construct @ Illuminate/Database/LazyLoadingViolationException.php:28
        Method      __construct @ Illuminate/Database/Migrations/DatabaseMigrationRepository.php:35
        Method      __construct @ Illuminate/Database/Migrations/MigrationCreator.php:38
        Method      __construct @ Illuminate/Database/Migrations/Migrator.php:104
        Method      __construct @ Illuminate/Database/MultipleRecordsFoundException.php:22
        Method      __construct @ Illuminate/Database/Query/Builder.php:283
        Method      __construct @ Illuminate/Database/Query/Expression.php:17
        Method      __construct @ Illuminate/Database/Query/IndexHint.php:26
        Method      __construct @ Illuminate/Database/Query/JoinClause.php:57
        Method      __construct @ Illuminate/Database/QueryException.php:56
        Method      __construct @ Illuminate/Database/SQLiteDatabaseDoesNotExistException.php:20
        Method      __construct @ Illuminate/Database/Schema/Blueprint.php:102
        Method      __construct @ Illuminate/Database/Schema/BlueprintState.php:60
        Method      __construct @ Illuminate/Database/Schema/Builder.php:62
        Method      __construct @ Illuminate/Database/Schema/ForeignIdColumnDefinition.php:21
        Method      __construct @ Illuminate/Database/Schema/SchemaState.php:52
        Method      __construct @ Illuminate/Encryption/Encrypter.php:53
        Method      __construct @ Illuminate/Encryption/MissingAppKeyException.php:13
        Method      __construct @ Illuminate/Events/CallQueuedListener.php:119
        Method      __construct @ Illuminate/Events/Dispatcher.php:111
        Method      __construct @ Illuminate/Events/NullDispatcher.php:23
        Method      __construct @ Illuminate/Events/QueuedClosure.php:66
        Method      __construct @ Illuminate/Filesystem/AwsS3V3Adapter.php:28
        Method      __construct @ Illuminate/Filesystem/FilesystemAdapter.php:106
        Method      __construct @ Illuminate/Filesystem/FilesystemManager.php:62
        Method      __construct @ Illuminate/Filesystem/LockableFile.php:35
        Method      __construct @ Illuminate/Filesystem/ReceiveFile.php:14
        Method      __construct @ Illuminate/Filesystem/ServeFile.php:13
        Method      __construct @ Illuminate/Foundation/AliasLoader.php:39
        Method      __construct @ Illuminate/Foundation/Application.php:215
        Method      __construct @ Illuminate/Foundation/Bus/PendingChain.php:66
        Method      __construct @ Illuminate/Foundation/Bus/PendingDispatch.php:40
        Method      __construct @ Illuminate/Foundation/CacheBasedMaintenanceMode.php:38
        Method      __construct @ Illuminate/Foundation/Cloud/Events.php:20
        Method      __construct @ Illuminate/Foundation/Cloud/FailedJobProvider.php:34
        Method      __construct @ Illuminate/Foundation/Cloud/Queue.php:38
        Method      __construct @ Illuminate/Foundation/Cloud/QueueConnector.php:26
        Method      __construct @ Illuminate/Foundation/Configuration/ApplicationBuilder.php:50
        Method      __construct @ Illuminate/Foundation/Configuration/Exceptions.php:16
        Method      __construct @ Illuminate/Foundation/Console/AboutCommand.php:56
        Method      __construct @ Illuminate/Foundation/Console/CliDumper.php:51
        Method      __construct @ Illuminate/Foundation/Console/ClosureCommand.php:40
        Method      __construct @ Illuminate/Foundation/Console/ConfigCacheCommand.php:41
        Method      __construct @ Illuminate/Foundation/Console/ConfigClearCommand.php:37
        Method      __construct @ Illuminate/Foundation/Console/EnvironmentDecryptCommand.php:50
        Method      __construct @ Illuminate/Foundation/Console/EnvironmentEncryptCommand.php:50
        Method      __construct @ Illuminate/Foundation/Console/EventClearCommand.php:37
        Method      __construct @ Illuminate/Foundation/Console/Kernel.php:135
        Method      __construct @ Illuminate/Foundation/Console/QueuedCommand.php:25
        Method      __construct @ Illuminate/Foundation/Console/RouteCacheCommand.php:39
        Method      __construct @ Illuminate/Foundation/Console/RouteClearCommand.php:37
        Method      __construct @ Illuminate/Foundation/Console/RouteListCommand.php:79
        Method      __construct @ Illuminate/Foundation/Console/VendorPublishCommand.php:83
        Method      __construct @ Illuminate/Foundation/Console/ViewClearCommand.php:38
        Method      __construct @ Illuminate/Foundation/DevCommand.php:21
        Method      __construct @ Illuminate/Foundation/Events/LocaleUpdated.php:26
        Method      __construct @ Illuminate/Foundation/Events/PublishingStubs.php:20
        Method      __construct @ Illuminate/Foundation/Events/VendorTagPublished.php:26
        Method      __construct @ Illuminate/Foundation/Exceptions/Handler.php:214
        Method      __construct @ Illuminate/Foundation/Exceptions/Renderer/Exception.php:50
        Method      __construct @ Illuminate/Foundation/Exceptions/Renderer/Frame.php:64
        Method      __construct @ Illuminate/Foundation/Exceptions/Renderer/Mappers/BladeMapper.php:67
        Method      __construct @ Illuminate/Foundation/Exceptions/Renderer/Renderer.php:63
        Method      __construct @ Illuminate/Foundation/Exceptions/ReportableHandler.php:30
        Method      __construct @ Illuminate/Foundation/Http/Attributes/ErrorBag.php:14
        Method      __construct @ Illuminate/Foundation/Http/Attributes/FailOnUnknownFields.php:9
        Method      __construct @ Illuminate/Foundation/Http/Attributes/RedirectTo.php:14
        Method      __construct @ Illuminate/Foundation/Http/Attributes/RedirectToRoute.php:14
        Method      __construct @ Illuminate/Foundation/Http/Events/RequestHandled.php:26
        Method      __construct @ Illuminate/Foundation/Http/HtmlDumper.php:56
        Method      __construct @ Illuminate/Foundation/Http/Kernel.php:122
        Method      __construct @ Illuminate/Foundation/Http/Middleware/HandlePrecognitiveRequests.php:24
        Method      __construct @ Illuminate/Foundation/Http/Middleware/PreventRequestForgery.php:78
        Method      __construct @ Illuminate/Foundation/Http/Middleware/PreventRequestsDuringMaintenance.php:42
        Method      __construct @ Illuminate/Foundation/PackageManifest.php:53
        Method      __construct @ Illuminate/Foundation/ProviderRepository.php:38
        Method      __construct @ Illuminate/Foundation/Testing/Attributes/Seed.php:12
        Method      __construct @ Illuminate/Foundation/Testing/Attributes/Seeder.php:14
        Method      __construct @ Illuminate/Foundation/Testing/DatabaseTransactionsManager.php:18
        Method      __construct @ Illuminate/Foundation/Testing/Wormhole.php:20
        Method      __construct @ Illuminate/Hashing/ArgonHasher.php:43
        Method      __construct @ Illuminate/Hashing/BcryptHasher.php:37
        Method      __construct @ Illuminate/Http/Client/Batch.php:127
        Method      __construct @ Illuminate/Http/Client/BatchInProgressException.php:6
        Method      __construct @ Illuminate/Http/Client/Events/ConnectionFailed.php:29
        Method      __construct @ Illuminate/Http/Client/Events/RequestSending.php:20
        Method      __construct @ Illuminate/Http/Client/Events/ResponseReceived.php:29
        Method      __construct @ Illuminate/Http/Client/Factory.php:97
        Method      __construct @ Illuminate/Http/Client/PendingRequest.php:259
        Method      __construct @ Illuminate/Http/Client/Pool.php:37
        Method      __construct @ Illuminate/Http/Client/Promises/FluentPromise.php:19
        Method      __construct @ Illuminate/Http/Client/Promises/LazyPromise.php:29
        Method      __construct @ Illuminate/Http/Client/Request.php:41
        Method      __construct @ Illuminate/Http/Client/RequestException.php:42
        Method      __construct @ Illuminate/Http/Client/Response.php:83
        Method      __construct @ Illuminate/Http/Client/ResponseSequence.php:38
        Method      __construct @ Illuminate/Http/Client/StrayRequestException.php:8
        Method      __construct @ Illuminate/Http/Exceptions/HttpResponseException.php:23
        Method      __construct @ Illuminate/Http/Exceptions/MalformedUrlException.php:11
        Method      __construct @ Illuminate/Http/Exceptions/PostTooLargeException.php:17
        Method      __construct @ Illuminate/Http/Exceptions/ThrottleRequestsException.php:17
        Method      __construct @ Illuminate/Http/JsonResponse.php:26
        Method      __construct @ Illuminate/Http/Middleware/HandleCors.php:38
        Method      __construct @ Illuminate/Http/Middleware/TrustHosts.php:35
        Method      __construct @ Illuminate/Http/Resources/Attributes/Collects.php:14
        Method      __construct @ Illuminate/Http/Resources/Attributes/PreserveKeys.php:12
        Method      __construct @ Illuminate/Http/Resources/Json/AnonymousResourceCollection.php:26
        Method      __construct @ Illuminate/Http/Resources/Json/JsonResource.php:65
        Method      __construct @ Illuminate/Http/Resources/Json/ResourceCollection.php:48
        Method      __construct @ Illuminate/Http/Resources/Json/ResourceResponse.php:22
        Method      __construct @ Illuminate/Http/Resources/JsonApi/RelationResolver.php:32
        Method      __construct @ Illuminate/Http/Resources/MergeValue.php:21
        Method      __construct @ Illuminate/Http/Response.php:29
        Method      __construct @ Illuminate/Http/StreamedEvent.php:19
        Method      __construct @ Illuminate/Http/Testing/File.php:42
        Method      __construct @ Illuminate/JsonSchema/Deserializer.php:37
        Method      __construct @ Illuminate/JsonSchema/Types/AnyOfType.php:11
        Method      __construct @ Illuminate/JsonSchema/Types/ObjectType.php:16
        Method      __construct @ Illuminate/JsonSchema/Types/UnionType.php:29
        Method      __construct @ Illuminate/Log/Context/Events/ContextDehydrating.php:18
        Method      __construct @ Illuminate/Log/Context/Events/ContextHydrated.php:18
        Method      __construct @ Illuminate/Log/Context/Repository.php:52
        Method      __construct @ Illuminate/Log/Events/MessageLogged.php:13
        Method      __construct @ Illuminate/Log/LogManager.php:77
        Method      __construct @ Illuminate/Log/Logger.php:44
        Method      __construct @ Illuminate/Mail/Attachment.php:44
        Method      __construct @ Illuminate/Mail/Events/MessageSending.php:14
        Method      __construct @ Illuminate/Mail/Events/MessageSent.php:19
        Method      __construct @ Illuminate/Mail/MailManager.php:65
        Method      __construct @ Illuminate/Mail/Mailables/Address.php:28
        Method      __construct @ Illuminate/Mail/Mailables/Content.php:66
        Method      __construct @ Illuminate/Mail/Mailables/Envelope.php:91
        Method      __construct @ Illuminate/Mail/Mailables/Headers.php:42
        Method      __construct @ Illuminate/Mail/Mailer.php:98
        Method      __construct @ Illuminate/Mail/Markdown.php:57
        Method      __construct @ Illuminate/Mail/Message.php:41
        Method      __construct @ Illuminate/Mail/PendingMail.php:53
        Method      __construct @ Illuminate/Mail/SendQueuedMailable.php:62
        Method      __construct @ Illuminate/Mail/SentMessage.php:27
        Method      __construct @ Illuminate/Mail/TextMessage.php:25
        Method      __construct @ Illuminate/Mail/Transport/ArrayTransport.php:23
        Method      __construct @ Illuminate/Mail/Transport/CloudflareTransport.php:27
        Method      __construct @ Illuminate/Mail/Transport/LogTransport.php:26
        Method      __construct @ Illuminate/Mail/Transport/ResendTransport.php:43
        Method      __construct @ Illuminate/Mail/Transport/SesTransport.php:36
        Method      __construct @ Illuminate/Mail/Transport/SesV2Transport.php:36
        Method      __construct @ Illuminate/Notifications/Action.php:26
        Method      __construct @ Illuminate/Notifications/Channels/BroadcastChannel.php:24
        Method      __construct @ Illuminate/Notifications/Channels/MailChannel.php:39
        Method      __construct @ Illuminate/Notifications/Events/BroadcastNotificationCreated.php:23
        Method      __construct @ Illuminate/Notifications/Events/NotificationFailed.php:19
        Method      __construct @ Illuminate/Notifications/Events/NotificationSending.php:18
        Method      __construct @ Illuminate/Notifications/Events/NotificationSent.php:19
        Method      __construct @ Illuminate/Notifications/Messages/BroadcastMessage.php:22
        Method      __construct @ Illuminate/Notifications/Messages/DatabaseMessage.php:18
        Method      __construct @ Illuminate/Notifications/NotificationSender.php:69
        Method      __construct @ Illuminate/Notifications/SendQueuedNotifications.php:85
        Method      __construct @ Illuminate/Pagination/Cursor.php:31
        Method      __construct @ Illuminate/Pagination/CursorPaginator.php:42
        Method      __construct @ Illuminate/Pagination/LengthAwarePaginator.php:50
        Method      __construct @ Illuminate/Pagination/Paginator.php:42
        Method      __construct @ Illuminate/Pagination/UrlWindow.php:20
        Method      __construct @ Illuminate/Pipeline/Hub.php:29
        Method      __construct @ Illuminate/Pipeline/Pipeline.php:64
        Method      __construct @ Illuminate/Process/Exceptions/ProcessFailedException.php:21
        Method      __construct @ Illuminate/Process/Exceptions/ProcessTimedOutException.php:23
        Method      __construct @ Illuminate/Process/FakeInvokedProcess.php:63
        Method      __construct @ Illuminate/Process/FakeProcessResult.php:46
        Method      __construct @ Illuminate/Process/FakeProcessSequence.php:35
        Method      __construct @ Illuminate/Process/InvokedProcess.php:26
        Method      __construct @ Illuminate/Process/InvokedProcessPool.php:21
        Method      __construct @ Illuminate/Process/PendingProcess.php:101
        Method      __construct @ Illuminate/Process/Pipe.php:40
        Method      __construct @ Illuminate/Process/Pool.php:40
        Method      __construct @ Illuminate/Process/ProcessPoolResults.php:21
        Method      __construct @ Illuminate/Process/ProcessResult.php:22
        Method      __construct @ Illuminate/Queue/Attributes/Backoff.php:21
        Method      __construct @ Illuminate/Queue/Attributes/Connection.php:17
        Method      __construct @ Illuminate/Queue/Attributes/DebounceFor.php:15
        Method      __construct @ Illuminate/Queue/Attributes/Delay.php:14
        Method      __construct @ Illuminate/Queue/Attributes/MaxExceptions.php:14
        Method      __construct @ Illuminate/Queue/Attributes/Queue.php:17
        Method      __construct @ Illuminate/Queue/Attributes/Timeout.php:14
        Method      __construct @ Illuminate/Queue/Attributes/Tries.php:14
        Method      __construct @ Illuminate/Queue/Attributes/UniqueFor.php:14
        Method      __construct @ Illuminate/Queue/BeanstalkdQueue.php:52
        Method      __construct @ Illuminate/Queue/CallQueuedClosure.php:50
        Method      __construct @ Illuminate/Queue/CallQueuedHandler.php:53
        Method      __construct @ Illuminate/Queue/Capsule/Manager.php:29
        Method      __construct @ Illuminate/Queue/Connectors/DatabaseConnector.php:21
        Method      __construct @ Illuminate/Queue/Connectors/FailoverConnector.php:13
        Method      __construct @ Illuminate/Queue/Connectors/RedisConnector.php:29
        Method      __construct @ Illuminate/Queue/Console/ListenCommand.php:50
        Method      __construct @ Illuminate/Queue/Console/MonitorCommand.php:52
        Method      __construct @ Illuminate/Queue/Console/RestartCommand.php:40
        Method      __construct @ Illuminate/Queue/Console/WorkCommand.php:93
        Method      __construct @ Illuminate/Queue/DatabaseQueue.php:63
        Method      __construct @ Illuminate/Queue/Events/JobAttempted.php:13
        Method      __construct @ Illuminate/Queue/Events/JobDebounced.php:13
        Method      __construct @ Illuminate/Queue/Events/JobExceptionOccurred.php:13
        Method      __construct @ Illuminate/Queue/Events/JobFailed.php:13
        Method      __construct @ Illuminate/Queue/Events/JobPopped.php:12
        Method      __construct @ Illuminate/Queue/Events/JobPopping.php:12
        Method      __construct @ Illuminate/Queue/Events/JobProcessed.php:12
        Method      __construct @ Illuminate/Queue/Events/JobProcessing.php:12
        Method      __construct @ Illuminate/Queue/Events/JobQueued.php:16
        Method      __construct @ Illuminate/Queue/Events/JobQueueing.php:15
        Method      __construct @ Illuminate/Queue/Events/JobReleasedAfterException.php:13
        Method      __construct @ Illuminate/Queue/Events/JobRetryRequested.php:18
        Method      __construct @ Illuminate/Queue/Events/JobTimedOut.php:12
        Method      __construct @ Illuminate/Queue/Events/Looping.php:13
        Method      __construct @ Illuminate/Queue/Events/QueueBusy.php:13
        Method      __construct @ Illuminate/Queue/Events/QueueFailedOver.php:15
        Method      __construct @ Illuminate/Queue/Events/QueuePaused.php:13
        Method      __construct @ Illuminate/Queue/Events/QueueResumed.php:12
        Method      __construct @ Illuminate/Queue/Events/WorkerIdle.php:15
        Method      __construct @ Illuminate/Queue/Events/WorkerInterrupted.php:16
        Method      __construct @ Illuminate/Queue/Events/WorkerPausing.php:15
        Method      __construct @ Illuminate/Queue/Events/WorkerResuming.php:15
        Method      __construct @ Illuminate/Queue/Events/WorkerStarting.php:13
        Method      __construct @ Illuminate/Queue/Events/WorkerStopping.php:13
        Method      __construct @ Illuminate/Queue/Failed/DatabaseFailedJobProvider.php:38
        Method      __construct @ Illuminate/Queue/Failed/DatabaseUuidFailedJobProvider.php:38
        Method      __construct @ Illuminate/Queue/Failed/DynamoDbFailedJobProvider.php:41
        Method      __construct @ Illuminate/Queue/Failed/FileFailedJobProvider.php:39
        Method      __construct @ Illuminate/Queue/FailoverQueue.php:23
        Method      __construct @ Illuminate/Queue/InvalidPayloadException.php:21
        Method      __construct @ Illuminate/Queue/Jobs/BeanstalkdJob.php:34
        Method      __construct @ Illuminate/Queue/Jobs/DatabaseJob.php:33
        Method      __construct @ Illuminate/Queue/Jobs/DatabaseJobRecord.php:22
        Method      __construct @ Illuminate/Queue/Jobs/InspectedJob.php:18
        Method      __construct @ Illuminate/Queue/Jobs/RedisJob.php:48
        Method      __construct @ Illuminate/Queue/Jobs/SqsJob.php:49
        Method      __construct @ Illuminate/Queue/Jobs/SyncJob.php:31
        Method      __construct @ Illuminate/Queue/Listener.php:52
        Method      __construct @ Illuminate/Queue/ListenerOptions.php:26
        Method      __construct @ Illuminate/Queue/Middleware/FailOnException.php:21
        Method      __construct @ Illuminate/Queue/Middleware/RateLimited.php:46
        Method      __construct @ Illuminate/Queue/Middleware/RateLimitedWithRedis.php:32
        Method      __construct @ Illuminate/Queue/Middleware/Skip.php:11
        Method      __construct @ Illuminate/Queue/Middleware/ThrottlesExceptions.php:94
        Method      __construct @ Illuminate/Queue/Middleware/WithoutOverlapping.php:54
        Method      __construct @ Illuminate/Queue/QueueManager.php:45
        Method      __construct @ Illuminate/Queue/RedisQueue.php:88
        Method      __construct @ Illuminate/Queue/SqsQueue.php:73
        Method      __construct @ Illuminate/Queue/SyncQueue.php:25
        Method      __construct @ Illuminate/Queue/Worker.php:171
        Method      __construct @ Illuminate/Queue/WorkerOptions.php:106
        Method      __construct @ Illuminate/Redis/Connections/PhpRedisConnection.php:38
        Method      __construct @ Illuminate/Redis/Connections/PredisConnection.php:26
        Method      __construct @ Illuminate/Redis/Events/CommandExecuted.php:49
        Method      __construct @ Illuminate/Redis/Events/CommandFailed.php:51
        Method      __construct @ Illuminate/Redis/Limiters/ConcurrencyLimiter.php:55
        Method      __construct @ Illuminate/Redis/Limiters/ConcurrencyLimiterBuilder.php:59
        Method      __construct @ Illuminate/Redis/Limiters/DurationLimiter.php:59
        Method      __construct @ Illuminate/Redis/Limiters/DurationLimiterBuilder.php:59
        Method      __construct @ Illuminate/Redis/RedisManager.php:74
        Method      __construct @ Illuminate/Routing/Attributes/Controllers/Authorize.php:15
        Method      __construct @ Illuminate/Routing/Attributes/Controllers/Middleware.php:14
        Method      __construct @ Illuminate/Routing/CallableDispatcher.php:24
        Method      __construct @ Illuminate/Routing/CompiledRouteCollection.php:64
        Method      __construct @ Illuminate/Routing/ControllerDispatcher.php:24
        Method      __construct @ Illuminate/Routing/ControllerMiddlewareOptions.php:18
        Method      __construct @ Illuminate/Routing/Controllers/Middleware.php:19
        Method      __construct @ Illuminate/Routing/Events/PreparingResponse.php:12
        Method      __construct @ Illuminate/Routing/Events/ResponsePrepared.php:12
        Method      __construct @ Illuminate/Routing/Events/RouteMatched.php:12
        Method      __construct @ Illuminate/Routing/Events/Routing.php:11
        Method      __construct @ Illuminate/Routing/Exceptions/BackedEnumCaseNotFoundException.php:14
        Method      __construct @ Illuminate/Routing/Exceptions/InvalidSignatureException.php:11
        Method      __construct @ Illuminate/Routing/Exceptions/StreamedResponseException.php:22
        Method      __construct @ Illuminate/Routing/Middleware/SubstituteBindings.php:22
        Method      __construct @ Illuminate/Routing/Middleware/ThrottleRequests.php:40
        Method      __construct @ Illuminate/Routing/Middleware/ThrottleRequestsWithRedis.php:38
        Method      __construct @ Illuminate/Routing/PendingResourceRegistration.php:54
        Method      __construct @ Illuminate/Routing/PendingSingletonResourceRegistration.php:54
        Method      __construct @ Illuminate/Routing/Redirector.php:31
        Method      __construct @ Illuminate/Routing/ResourceRegistrar.php:65
        Method      __construct @ Illuminate/Routing/ResponseFactory.php:44
        Method      __construct @ Illuminate/Routing/Route.php:177
        Method      __construct @ Illuminate/Routing/RouteFileRegistrar.php:18
        Method      __construct @ Illuminate/Routing/RouteParameterBinder.php:20
        Method      __construct @ Illuminate/Routing/RouteRegistrar.php:104
        Method      __construct @ Illuminate/Routing/RouteUri.php:26
        Method      __construct @ Illuminate/Routing/RouteUrlGenerator.php:61
        Method      __construct @ Illuminate/Routing/Router.php:143
        Method      __construct @ Illuminate/Routing/SortedMiddleware.php:14
        Method      __construct @ Illuminate/Routing/UrlGenerator.php:127
        Method      __construct @ Illuminate/Routing/ViewController.php:20
        Method      __construct @ Illuminate/Session/ArraySessionHandler.php:30
        Method      __construct @ Illuminate/Session/CacheBasedSessionHandler.php:29
        Method      __construct @ Illuminate/Session/CookieSessionHandler.php:48
        Method      __construct @ Illuminate/Session/DatabaseSessionHandler.php:60
        Method      __construct @ Illuminate/Session/EncryptedStore.php:26
        Method      __construct @ Illuminate/Session/FileSessionHandler.php:39
        Method      __construct @ Illuminate/Session/Middleware/AuthenticateSession.php:32
        Method      __construct @ Illuminate/Session/Middleware/StartSession.php:36
        Method      __construct @ Illuminate/Session/Store.php:84
        Method      __construct @ Illuminate/Session/SymfonySessionDecorator.php:24
        Method      __construct @ Illuminate/Support/Composer.php:32
        Method      __construct @ Illuminate/Support/DefaultProviders.php:18
        Method      __construct @ Illuminate/Support/Defer/DeferredCallback.php:13
        Method      __construct @ Illuminate/Support/EncodedHtmlString.php:30
        Method      __construct @ Illuminate/Support/Fluent.php:40
        Method      __construct @ Illuminate/Support/HigherOrderTapProxy.php:18
        Method      __construct @ Illuminate/Support/HtmlString.php:21
        Method      __construct @ Illuminate/Support/Js.php:42
        Method      __construct @ Illuminate/Support/Lottery.php:51
        Method      __construct @ Illuminate/Support/Manager.php:47
        Method      __construct @ Illuminate/Support/MessageBag.php:32
        Method      __construct @ Illuminate/Support/MultipleInstanceManager.php:53
        Method      __construct @ Illuminate/Support/NodePackageManager.php:13
        Method      __construct @ Illuminate/Support/Once.php:27
        Method      __construct @ Illuminate/Support/Onceable.php:17
        Method      __construct @ Illuminate/Support/Optional.php:26
        Method      __construct @ Illuminate/Support/ServiceProvider.php:86
        Method      __construct @ Illuminate/Support/Sleep.php:83
        Method      __construct @ Illuminate/Support/Stringable.php:30
        Method      __construct @ Illuminate/Support/Testing/Fakes/BatchFake.php:41
        Method      __construct @ Illuminate/Support/Testing/Fakes/BusFake.php:90
        Method      __construct @ Illuminate/Support/Testing/Fakes/ChainedBatchTruthTest.php:20
        Method      __construct @ Illuminate/Support/Testing/Fakes/EventFake.php:54
        Method      __construct @ Illuminate/Support/Testing/Fakes/ExceptionHandlerFake.php:42
        Method      __construct @ Illuminate/Support/Testing/Fakes/MailFake.php:55
        Method      __construct @ Illuminate/Support/Testing/Fakes/PendingBatchFake.php:26
        Method      __construct @ Illuminate/Support/Testing/Fakes/PendingChainFake.php:24
        Method      __construct @ Illuminate/Support/Testing/Fakes/PendingMailFake.php:14
        Method      __construct @ Illuminate/Support/Testing/Fakes/QueueFake.php:84
        Method      __construct @ Illuminate/Support/Uri.php:36
        Method      __construct @ Illuminate/Support/UriQueryString.php:16
        Method      __construct @ Illuminate/Support/ValidatedInput.php:26
        Method      __construct @ Illuminate/Testing/AssertableJsonString.php:37
        Method      __construct @ Illuminate/Testing/Concerns/RunsInParallel.php:57
        Method      __construct @ Illuminate/Testing/Constraints/ArraySubset.php:28
        Method      __construct @ Illuminate/Testing/Constraints/CountInDatabase.php:37
        Method      __construct @ Illuminate/Testing/Constraints/HasInDatabase.php:37
        Method      __construct @ Illuminate/Testing/Constraints/NotSoftDeletedInDatabase.php:44
        Method      __construct @ Illuminate/Testing/Constraints/SeeInHtml.php:42
        Method      __construct @ Illuminate/Testing/Constraints/SeeInOrder.php:28
        Method      __construct @ Illuminate/Testing/Constraints/SoftDeletedInDatabase.php:44
        Method      __construct @ Illuminate/Testing/Fluent/AssertableJson.php:43
        Method      __construct @ Illuminate/Testing/ParallelConsoleOutput.php:32
        Method      __construct @ Illuminate/Testing/ParallelTesting.php:77
        Method      __construct @ Illuminate/Testing/PendingCommand.php:88
        Method      __construct @ Illuminate/Testing/TestComponent.php:37
        Method      __construct @ Illuminate/Testing/TestResponse.php:76
        Method      __construct @ Illuminate/Testing/TestResponseAssert.php:19
        Method      __construct @ Illuminate/Testing/TestView.php:38
        Method      __construct @ Illuminate/Translation/FileLoader.php:45
        Method      __construct @ Illuminate/Translation/PotentiallyTranslatedString.php:35
        Method      __construct @ Illuminate/Translation/Translator.php:89
        Method      __construct @ Illuminate/Validation/ClosureValidationRule.php:45
        Method      __construct @ Illuminate/Validation/Concerns/FilterEmailValidation.php:22
        Method      __construct @ Illuminate/Validation/ConditionalRules.php:36
        Method      __construct @ Illuminate/Validation/DatabasePresenceVerifier.php:28
        Method      __construct @ Illuminate/Validation/Factory.php:88
        Method      __construct @ Illuminate/Validation/InvokableValidationRule.php:56
        Method      __construct @ Illuminate/Validation/NestedRules.php:20
        Method      __construct @ Illuminate/Validation/NotPwnedVerifier.php:30
        Method      __construct @ Illuminate/Validation/Rules/AnyOf.php:33
        Method      __construct @ Illuminate/Validation/Rules/ArrayRule.php:23
        Method      __construct @ Illuminate/Validation/Rules/Can.php:37
        Method      __construct @ Illuminate/Validation/Rules/Contains.php:23
        Method      __construct @ Illuminate/Validation/Rules/DatabaseRule.php:47
        Method      __construct @ Illuminate/Validation/Rules/Dimensions.php:23
        Method      __construct @ Illuminate/Validation/Rules/DoesntContain.php:23
        Method      __construct @ Illuminate/Validation/Rules/Enum.php:51
        Method      __construct @ Illuminate/Validation/Rules/ExcludeIf.php:24
        Method      __construct @ Illuminate/Validation/Rules/ExcludeUnless.php:24
        Method      __construct @ Illuminate/Validation/Rules/ImageFile.php:11
        Method      __construct @ Illuminate/Validation/Rules/In.php:30
        Method      __construct @ Illuminate/Validation/Rules/NotIn.php:30
        Method      __construct @ Illuminate/Validation/Rules/Password.php:132
        Method      __construct @ Illuminate/Validation/Rules/ProhibitedIf.php:24
        Method      __construct @ Illuminate/Validation/Rules/ProhibitedUnless.php:24
        Method      __construct @ Illuminate/Validation/Rules/RequiredIf.php:24
        Method      __construct @ Illuminate/Validation/Rules/RequiredUnless.php:24
        Method      __construct @ Illuminate/Validation/ValidationException.php:52
        Method      __construct @ Illuminate/Validation/ValidationRuleParser.php:39
        Method      __construct @ Illuminate/Validation/Validator.php:339
        Method      __construct @ Illuminate/View/AnonymousComponent.php:26
        Method      __construct @ Illuminate/View/AppendableAttributeValue.php:20
        Method      __construct @ Illuminate/View/Compilers/Compiler.php:65
        Method      __construct @ Illuminate/View/Compilers/ComponentTagCompiler.php:57
        Method      __construct @ Illuminate/View/ComponentAttributeBag.php:36
        Method      __construct @ Illuminate/View/ComponentSlot.php:30
        Method      __construct @ Illuminate/View/DynamicComponent.php:40
        Method      __construct @ Illuminate/View/Engines/CompilerEngine.php:43
        Method      __construct @ Illuminate/View/Engines/FileEngine.php:21
        Method      __construct @ Illuminate/View/Engines/PhpEngine.php:22
        Method      __construct @ Illuminate/View/Factory.php:113
        Method      __construct @ Illuminate/View/FileViewFinder.php:51
        Method      __construct @ Illuminate/View/InvokableComponentVariable.php:26
        Method      __construct @ Illuminate/View/Middleware/ShareErrorsFromSession.php:22
        Method      __construct @ Illuminate/View/View.php:70
        Method      __destruct @ Illuminate/Broadcasting/FakePendingBroadcast.php:40
        Method      __destruct @ Illuminate/Broadcasting/PendingBroadcast.php:70
        Method      __destruct @ Illuminate/Foundation/Bus/PendingDispatch.php:282
        Method      __destruct @ Illuminate/Routing/PendingResourceRegistration.php:330
        Method      __destruct @ Illuminate/Routing/PendingSingletonResourceRegistration.php:302
        Method      __destruct @ Illuminate/Support/Sleep.php:296
        Method      __destruct @ Illuminate/Testing/PendingCommand.php:668
        Method      __toString @ Illuminate/Auth/Access/Response.php:210
        Method      __toString @ Illuminate/Broadcasting/Channel.php:31
        Method      __toString @ Illuminate/Collections/Enumerable.php:1333
        Method      __toString @ Illuminate/Collections/Traits/EnumeratesValues.php:1039
        Method      __toString @ Illuminate/Database/Eloquent/Model.php:2832
        Method      __toString @ Illuminate/Http/Client/Response.php:591
        Method      __toString @ Illuminate/JsonSchema/Types/Type.php:127
        Method      __toString @ Illuminate/Mail/Transport/ArrayTransport.php:61
        Method      __toString @ Illuminate/Mail/Transport/CloudflareTransport.php:181
        Method      __toString @ Illuminate/Mail/Transport/LogTransport.php:93
        Method      __toString @ Illuminate/Mail/Transport/ResendTransport.php:142
        Method      __toString @ Illuminate/Mail/Transport/SesTransport.php:148
        Method      __toString @ Illuminate/Mail/Transport/SesV2Transport.php:152
        Method      __toString @ Illuminate/Pagination/AbstractCursorPaginator.php:683
        Method      __toString @ Illuminate/Pagination/AbstractPaginator.php:810
        Method      __toString @ Illuminate/Support/HtmlString.php:61
        Method      __toString @ Illuminate/Support/Js.php:160
        Method      __toString @ Illuminate/Support/MessageBag.php:449
        Method      __toString @ Illuminate/Support/Stringable.php:1621
        Method      __toString @ Illuminate/Support/Uri.php:469
        Method      __toString @ Illuminate/Support/UriQueryString.php:91
        Method      __toString @ Illuminate/Support/ViewErrorBag.php:126
        Method      __toString @ Illuminate/Testing/TestComponent.php:194
        Method      __toString @ Illuminate/Testing/TestView.php:267
        Method      __toString @ Illuminate/Translation/PotentiallyTranslatedString.php:86
        Method      __toString @ Illuminate/Validation/Rules/ArrayRule.php:37
        Method      __toString @ Illuminate/Validation/Rules/Contains.php:37
        Method      __toString @ Illuminate/Validation/Rules/Date.php:169
        Method      __toString @ Illuminate/Validation/Rules/Dimensions.php:165
        Method      __toString @ Illuminate/Validation/Rules/DoesntContain.php:37
        Method      __toString @ Illuminate/Validation/Rules/Enum.php:155
        Method      __toString @ Illuminate/Validation/Rules/ExcludeIf.php:38
        Method      __toString @ Illuminate/Validation/Rules/ExcludeUnless.php:38
        Method      __toString @ Illuminate/Validation/Rules/Exists.php:16
        Method      __toString @ Illuminate/Validation/Rules/In.php:46
        Method      __toString @ Illuminate/Validation/Rules/NotIn.php:44
        Method      __toString @ Illuminate/Validation/Rules/Numeric.php:215
        Method      __toString @ Illuminate/Validation/Rules/ProhibitedIf.php:38
        Method      __toString @ Illuminate/Validation/Rules/ProhibitedUnless.php:38
        Method      __toString @ Illuminate/Validation/Rules/RequiredIf.php:42
        Method      __toString @ Illuminate/Validation/Rules/RequiredUnless.php:42
        Method      __toString @ Illuminate/Validation/Rules/StringRule.php:172
        Method      __toString @ Illuminate/Validation/Rules/Unique.php:65
        Method      __toString @ Illuminate/View/AppendableAttributeValue.php:30
        Method      __toString @ Illuminate/View/ComponentAttributeBag.php:484
        Method      __toString @ Illuminate/View/ComponentSlot.php:106
        Method      __toString @ Illuminate/View/InvokableComponentVariable.php:91
        Method      __toString @ Illuminate/View/View.php:502
        Method      addConstraints @ Illuminate/Database/Eloquent/Relations/BelongsTo.php:91
        Method      addConstraints @ Illuminate/Database/Eloquent/Relations/BelongsToMany.php:209
        Method      addConstraints @ Illuminate/Database/Eloquent/Relations/HasOneOrMany.php:90
        Method      addConstraints @ Illuminate/Database/Eloquent/Relations/HasOneOrManyThrough.php:97
        Method      addConstraints @ Illuminate/Database/Eloquent/Relations/MorphOneOrMany.php:54
        Method      addConstraints @ Illuminate/Database/Eloquent/Relations/Relation.php:129
        Method      addEagerConstraints @ Illuminate/Database/Eloquent/Relations/BelongsTo.php:104
        Method      addEagerConstraints @ Illuminate/Database/Eloquent/Relations/BelongsToMany.php:256
        Method      addEagerConstraints @ Illuminate/Database/Eloquent/Relations/HasOneOrMany.php:102
        Method      addEagerConstraints @ Illuminate/Database/Eloquent/Relations/HasOneOrManyThrough.php:164
        Method      addEagerConstraints @ Illuminate/Database/Eloquent/Relations/MorphOneOrMany.php:64
        Method      addEagerConstraints @ Illuminate/Database/Eloquent/Relations/MorphTo.php:95
        Method      addEagerConstraints @ Illuminate/Database/Eloquent/Relations/MorphToMany.php:93
        Method      addEagerConstraints @ Illuminate/Database/Eloquent/Relations/Relation.php:137
        Method      addGroupNamespaceToStringUses @ Illuminate/Routing/Route.php:959
        Method      addOneOfManyJoinSubQueryConstraints @ Illuminate/Database/Eloquent/Relations/Concerns/CanBeOneOfMany.php:57
        Method      addOneOfManyJoinSubQueryConstraints @ Illuminate/Database/Eloquent/Relations/HasOne.php:88
        Method      addOneOfManyJoinSubQueryConstraints @ Illuminate/Database/Eloquent/Relations/HasOneThrough.php:96
        Method      addOneOfManyJoinSubQueryConstraints @ Illuminate/Database/Eloquent/Relations/MorphOne.php:88
        Method      addOneOfManySubQueryConstraints @ Illuminate/Database/Eloquent/Relations/Concerns/CanBeOneOfMany.php:42
        Method      addOneOfManySubQueryConstraints @ Illuminate/Database/Eloquent/Relations/HasOne.php:67
        Method      addOneOfManySubQueryConstraints @ Illuminate/Database/Eloquent/Relations/HasOneThrough.php:79
        Method      addOneOfManySubQueryConstraints @ Illuminate/Database/Eloquent/Relations/MorphOne.php:67
        Method      addProviderToBootstrapFile @ Illuminate/Support/ServiceProvider.php:586
        Method      addQueryString @ Illuminate/Routing/RouteUrlGenerator.php:378
        Method      addResourceDestroy @ Illuminate/Routing/ResourceRegistrar.php:413
        Method      addSingletonDestroy @ Illuminate/Routing/ResourceRegistrar.php:527
        Method      addWhereConstraints @ Illuminate/Database/Eloquent/Relations/BelongsToMany.php:246
        Method      addWhereConstraints @ Illuminate/Database/Eloquent/Relations/MorphToMany.php:83
        Method      afterBootstrapping @ Illuminate/Foundation/Application.php:379
        Method      allowStrayRequests @ Illuminate/Http/Client/Factory.php:428
        Method      allowStrayRequests @ Illuminate/Http/Client/PendingRequest.php:1911
        Method      allowsTrashedBindings @ Illuminate/Routing/Route.php:624
        Method      askForPageViaCustomStrategy @ Illuminate/Foundation/Console/DocsCommand.php:213
        Method      assertExactJsonStructure @ Illuminate/Testing/TestResponse.php:1050
        Method      assertJsonStructure @ Illuminate/Testing/TestResponse.php:1036
        Method      assertNotStreamed @ Illuminate/Testing/TestResponse.php:662
        Method      assertStreamed @ Illuminate/Testing/TestResponse.php:647
        Method      assertStreamedContent @ Illuminate/Testing/TestResponse.php:678
        Method      assertStreamedJsonContent @ Illuminate/Testing/TestResponse.php:693
        Method      assertStructure @ Illuminate/Testing/AssertableJsonString.php:270
        Method      attributesToString @ Illuminate/View/Compilers/ComponentTagCompiler.php:789
        Method      beforeApplicationDestroyed @ Illuminate/Foundation/Testing/Concerns/InteractsWithTestCaseLifecycle.php:310
        Method      beforeBootstrapping @ Illuminate/Foundation/Application.php:367
        Method      bootstrap @ Illuminate/Console/Application.php:129
        Method      bootstrap @ Illuminate/Contracts/Console/Kernel.php:11
        Method      bootstrap @ Illuminate/Contracts/Http/Kernel.php:11
        Method      bootstrap @ Illuminate/Foundation/Bootstrap/BootProviders.php:14
        Method      bootstrap @ Illuminate/Foundation/Bootstrap/HandleExceptions.php:40
        Method      bootstrap @ Illuminate/Foundation/Bootstrap/LoadConfiguration.php:27
        Method      bootstrap @ Illuminate/Foundation/Bootstrap/LoadEnvironmentVariables.php:19
        Method      bootstrap @ Illuminate/Foundation/Bootstrap/RegisterFacades.php:17
        Method      bootstrap @ Illuminate/Foundation/Bootstrap/RegisterProviders.php:29
        Method      bootstrap @ Illuminate/Foundation/Bootstrap/SetRequestForConsole.php:15
        Method      bootstrap @ Illuminate/Foundation/Console/Kernel.php:490
        Method      bootstrap @ Illuminate/Foundation/Http/Kernel.php:182
        Method      bootstrapPath @ Illuminate/Contracts/Foundation/Application.php:29
        Method      bootstrapPath @ Illuminate/Foundation/Application.php:480
        Method      bootstrapWith @ Illuminate/Contracts/Foundation/Application.php:184
        Method      bootstrapWith @ Illuminate/Foundation/Application.php:334
        Method      bootstrapWithoutBootingProviders @ Illuminate/Foundation/Console/Kernel.php:532
        Method      bootstrapperBootstrapped @ Illuminate/Foundation/Cloud.php:33
        Method      bootstrapperBootstrapping @ Illuminate/Foundation/Cloud.php:20
        Method      bootstrappers @ Illuminate/Foundation/Console/Kernel.php:627
        Method      bootstrappers @ Illuminate/Foundation/Http/Kernel.php:548
        Method      broadcastRestored @ Illuminate/Database/Eloquent/BroadcastsEvents.php:83
        Method      buildClusterConnectionString @ Illuminate/Redis/Connectors/PhpRedisConnector.php:64
        Method      buildConnectString @ Illuminate/Database/Connectors/SqlServerConnector.php:201
        Method      buildFormRequestReplacements @ Illuminate/Routing/Console/ControllerMakeCommand.php:223
        Method      buildHostString @ Illuminate/Database/Connectors/SqlServerConnector.php:215
        Method      buildString @ Illuminate/JsonSchema/Deserializer.php:237
        Method      callBeforeApplicationDestroyedCallbacks @ Illuminate/Foundation/Testing/Concerns/InteractsWithTestCaseLifecycle.php:320
        Method      castAttributeAsEncryptedString @ Illuminate/Database/Eloquent/Concerns/HasAttributes.php:1457
        Method      castAttributeAsHashedString @ Illuminate/Database/Eloquent/Concerns/HasAttributes.php:1492
        Method      combineConstraints @ Illuminate/Database/Eloquent/Builder.php:1872
        Method      compileDisableForeignKeyConstraints @ Illuminate/Database/Schema/Grammars/MySqlGrammar.php:716
        Method      compileDisableForeignKeyConstraints @ Illuminate/Database/Schema/Grammars/PostgresGrammar.php:713
        Method      compileDisableForeignKeyConstraints @ Illuminate/Database/Schema/Grammars/SQLiteGrammar.php:681
        Method      compileDisableForeignKeyConstraints @ Illuminate/Database/Schema/Grammars/SqlServerGrammar.php:524
        Method      compileDropDefaultConstraint @ Illuminate/Database/Schema/Grammars/SqlServerGrammar.php:393
        Method      compileEnableForeignKeyConstraints @ Illuminate/Database/Schema/Grammars/MySqlGrammar.php:706
        Method      compileEnableForeignKeyConstraints @ Illuminate/Database/Schema/Grammars/PostgresGrammar.php:703
        Method      compileEnableForeignKeyConstraints @ Illuminate/Database/Schema/Grammars/SQLiteGrammar.php:671
        Method      compileEnableForeignKeyConstraints @ Illuminate/Database/Schema/Grammars/SqlServerGrammar.php:514
        Method      compileString @ Illuminate/View/Compilers/BladeCompiler.php:282
        Method      componentString @ Illuminate/View/Compilers/ComponentTagCompiler.php:232
        Method      configureForeignKeyConstraints @ Illuminate/Database/Connectors/SQLiteConnector.php:91
        Method      connectionString @ Illuminate/Database/Schema/MySqlSchemaState.php:110
        Method      constrain @ Illuminate/Database/Eloquent/Relations/MorphTo.php:351
        Method      constrained @ Illuminate/Database/Schema/ForeignIdColumnDefinition.php:36
        Method      containsStrict @ Illuminate/Collections/Collection.php:214
        Method      containsStrict @ Illuminate/Collections/Enumerable.php:137
        Method      containsStrict @ Illuminate/Collections/LazyCollection.php:274
        Method      convertEmptyStringsToNull @ Illuminate/Foundation/Configuration/Middleware.php:648
        Method      createBladeViewFromString @ Illuminate/View/Component.php:195
        Method      createRandomStringsNormally @ Illuminate/Support/Str.php:1176
        Method      createRandomStringsUsing @ Illuminate/Support/Str.php:1132
        Method      createRandomStringsUsingSequence @ Illuminate/Support/Str.php:1144
        Method      createSelectWithConstraint @ Illuminate/Database/Eloquent/Builder.php:1904
        Method      createSesTransport @ Illuminate/Mail/MailManager.php:256
        Method      createStringPayload @ Illuminate/Queue/Queue.php:305
        Method      createTestRequest @ Illuminate/Foundation/Testing/Concerns/MakesHttpRequests.php:743
        Method      createTestResponse @ Illuminate/Foundation/Testing/Concerns/MakesHttpRequests.php:755
        Method      decryptString @ Illuminate/Contracts/Encryption/StringEncrypter.php:24
        Method      decryptString @ Illuminate/Encryption/Encrypter.php:214
        Method      defaultStringLength @ Illuminate/Database/Schema/Builder.php:74
        Method      destroy @ Illuminate/Database/Eloquent/Model.php:1672
        Method      destroy @ Illuminate/Session/ArraySessionHandler.php:97
        Method      destroy @ Illuminate/Session/CacheBasedSessionHandler.php:80
        Method      destroy @ Illuminate/Session/CookieSessionHandler.php:112
        Method      destroy @ Illuminate/Session/DatabaseSessionHandler.php:265
        Method      destroy @ Illuminate/Session/FileSessionHandler.php:98
        Method      destroy @ Illuminate/Session/NullSessionHandler.php:53
        Method      destroyable @ Illuminate/Routing/PendingSingletonResourceRegistration.php:105
        Method      disableForeignKeyConstraints @ Illuminate/Database/Schema/Builder.php:639
        Method      doesntContainStrict @ Illuminate/Collections/Collection.php:248
        Method      doesntContainStrict @ Illuminate/Collections/LazyCollection.php:314
        Method      dropConstrainedForeignId @ Illuminate/Database/Schema/Blueprint.php:512
        Method      dropConstrainedForeignIdFor @ Illuminate/Database/Schema/Blueprint.php:542
        Method      duplicatesStrict @ Illuminate/Collections/Collection.php:376
        Method      duplicatesStrict @ Illuminate/Collections/Enumerable.php:260
        Method      duplicatesStrict @ Illuminate/Collections/LazyCollection.php:421
        Method      enableForeignKeyConstraints @ Illuminate/Database/Schema/Builder.php:627
        Method      encryptString @ Illuminate/Contracts/Encryption/StringEncrypter.php:14
        Method      encryptString @ Illuminate/Encryption/Encrypter.php:140
        Method      ensureCastsAreStringValues @ Illuminate/Database/Eloquent/Concerns/HasAttributes.php:811
        Method      ensureUnionConstraintsAreSupported @ Illuminate/JsonSchema/Deserializer.php:444
        Method      escapeString @ Illuminate/Database/Connection.php:1198
        Method      escapeWhenCastingToString @ Illuminate/Collections/Enumerable.php:1341
        Method      escapeWhenCastingToString @ Illuminate/Collections/Traits/EnumeratesValues.php:1052
        Method      escapeWhenCastingToString @ Illuminate/Contracts/Support/CanBeEscapedWhenCastToString.php:12
        Method      escapeWhenCastingToString @ Illuminate/Database/Eloquent/Model.php:2845
        Method      escapeWhenCastingToString @ Illuminate/Pagination/AbstractPaginator.php:823
        Method      eventStream @ Illuminate/Contracts/Routing/ResponseFactory.php:70
        Method      eventStream @ Illuminate/Routing/ResponseFactory.php:130
        Method      extractBladeViewFromString @ Illuminate/View/Component.php:173
        Method      extractConstructorParameters @ Illuminate/View/Component.php:121
        Method      extractFromString @ Illuminate/Translation/MessageSelector.php:58
        Method      extractQueryString @ Illuminate/Routing/UrlGenerator.php:628
        Method      forceDestroy @ Illuminate/Database/Eloquent/Model.php:1791
        Method      forceDestroy @ Illuminate/Database/Eloquent/SoftDeletes.php:83
        Method      forgetBootstrappers @ Illuminate/Console/Application.php:141
        Method      formatCommandString @ Illuminate/Console/Application.php:108
        Method      freshTimestampString @ Illuminate/Database/Eloquent/Concerns/HasTimestamps.php:146
        Method      fromAssertableJsonString @ Illuminate/Testing/Fluent/AssertableJson.php:163
        Method      fromClassMethodString @ Illuminate/Routing/RouteSignatureParameters.php:49
        Method      fromEncryptedString @ Illuminate/Database/Eloquent/Concerns/HasAttributes.php:1445
        Method      fromJsonString @ Illuminate/Http/JsonResponse.php:38
        Method      getAttributesFromAttributeString @ Illuminate/View/Compilers/ComponentTagCompiler.php:596
        Method      getBootstrapProvidersPath @ Illuminate/Foundation/Application.php:490
        Method      getLastRendered @ Illuminate/View/Engines/Engine.php:18
        Method      getRelationWithoutConstraints @ Illuminate/Database/Eloquent/Concerns/QueriesRelationships.php:1120
        Method      getRouteQueryString @ Illuminate/Routing/RouteUrlGenerator.php:398
        Method      getStringParameters @ Illuminate/Routing/RouteUrlGenerator.php:431
        Method      hasBeenBootstrapped @ Illuminate/Contracts/Foundation/Application.php:215
        Method      hasBeenBootstrapped @ Illuminate/Foundation/Application.php:389
        Method      htmlString @ Illuminate/Mail/Mailables/Content.php:132
        Method      ignoreFieldsAndIncludesInQueryString @ Illuminate/Http/Resources/JsonApi/Concerns/ResolvesJsonApiElements.php:421
        Method      initializeHasUniqueStringIds @ Illuminate/Database/Eloquent/Concerns/HasUniqueStringIds.php:28
        Method      isEmptyString @ Illuminate/Support/Traits/InteractsWithData.php:259
        Method      isParameterBackedEnumWithStringBackingType @ Illuminate/Reflection/Reflector.php:193
        Method      isUniqueConstraintError @ Illuminate/Database/Connection.php:881
        Method      isUniqueConstraintError @ Illuminate/Database/MySqlConnection.php:79
        Method      isUniqueConstraintError @ Illuminate/Database/PostgresConnection.php:77
        Method      isUniqueConstraintError @ Illuminate/Database/SQLiteConnection.php:59
        Method      isUniqueConstraintError @ Illuminate/Database/SqlServerConnection.php:85
        Method      jsonSearchStrings @ Illuminate/Testing/AssertableJsonString.php:370
        Method      latestReadWriteTypeUsed @ Illuminate/Database/Connection.php:1801
        Method      matchAgainstRoutes @ Illuminate/Routing/AbstractRouteCollection.php:78
        Method      mergeConstraintsFrom @ Illuminate/Database/Eloquent/Concerns/QueriesRelationships.php:1053
        Method      noConstraints @ Illuminate/Database/Eloquent/Relations/Relation.php:108
        Method      normalizeScalarString @ Illuminate/Http/Client/Factory.php:249
        Method      normalizeScalarString @ Illuminate/Http/Client/PendingRequest.php:1578
        Method      openViaBuiltInStrategy @ Illuminate/Foundation/Console/DocsCommand.php:375
        Method      openViaCustomStrategy @ Illuminate/Foundation/Console/DocsCommand.php:350
        Method      parseNameAndAttributeSelectionConstraint @ Illuminate/Database/Eloquent/Builder.php:1889
        Method      parsePipeString @ Illuminate/Pipeline/Pipeline.php:235
        Method      parseStringRule @ Illuminate/Validation/ValidationRuleParser.php:273
        Method      parseStringsToNativeTypes @ Illuminate/Support/ConfigurationUrlParser.php:150
        Method      parseUniqueConstraintViolation @ Illuminate/Database/Connection.php:892
        Method      parseUniqueConstraintViolation @ Illuminate/Database/MySqlConnection.php:90
        Method      parseUniqueConstraintViolation @ Illuminate/Database/PostgresConnection.php:88
        Method      parseUniqueConstraintViolation @ Illuminate/Database/SQLiteConnection.php:70
        Method      parseUniqueConstraintViolation @ Illuminate/Database/SqlServerConnection.php:96
        Method      pendingPotentiallyTranslatedString @ Illuminate/Translation/CreatesPotentiallyTranslatedStrings.php:13
        Method      prepareStringsForCompilationUsing @ Illuminate/View/Compilers/BladeCompiler.php:1015
        Method      preventStrayProcesses @ Illuminate/Process/Factory.php:159
        Method      preventStrayRequests @ Illuminate/Http/Client/Factory.php:405
        Method      preventStrayRequests @ Illuminate/Http/Client/PendingRequest.php:1898
        Method      preventStrayRequests @ Illuminate/Support/Facades/Http.php:153
        Method      preventingStrayProcesses @ Illuminate/Process/Factory.php:171
        Method      preventingStrayRequests @ Illuminate/Http/Client/Factory.php:417
        Method      prohibitDestructiveCommands @ Illuminate/Support/Facades/DB.php:137
        Method      properString @ Illuminate/Auth/Recaller.php:68
        Method      queryStringResolver @ Illuminate/Pagination/AbstractPaginator.php:563
        Method      quoteString @ Illuminate/Database/Grammar.php:228
        Method      quoteString @ Illuminate/Database/Schema/Grammars/SqlServerGrammar.php:1036
        Method      readStream @ Illuminate/Contracts/Filesystem/Filesystem.php:50
        Method      readStream @ Illuminate/Filesystem/FilesystemAdapter.php:701
        Method      recordRequestResponsePair @ Illuminate/Http/Client/Factory.php:460
        Method      referencesString @ Illuminate/Mail/Mailables/Headers.php:93
        Method      registerRequestRebindHandler @ Illuminate/Auth/AuthServiceProvider.php:84
        Method      removeAbstractAlias @ Illuminate/Container/Container.php:626
        Method      removeProviderFromBootstrapFile @ Illuminate/Support/ServiceProvider.php:625
        Method      replacePlaceholderInString @ Illuminate/Validation/Validator.php:411
        Method      requestRebinder @ Illuminate/Routing/RoutingServiceProvider.php:99
        Method      resolveQueryString @ Illuminate/Pagination/AbstractPaginator.php:548
        Method      respectFieldsAndIncludesInQueryString @ Illuminate/Http/Resources/JsonApi/Concerns/ResolvesJsonApiElements.php:409
        Method      restrictOnDelete @ Illuminate/Database/Schema/ForeignKeyDefinition.php:72
        Method      restrictOnUpdate @ Illuminate/Database/Schema/ForeignKeyDefinition.php:32
        Method      sendRequestThroughRouter @ Illuminate/Foundation/Http/Kernel.php:163
        Method      setGlobalToAndRemoveCcAndBcc @ Illuminate/Mail/Mailer.php:452
        Method      setTouchedRelations @ Illuminate/Database/Eloquent/Concerns/HasRelationships.php:1216
        Method      setTransactionManagerResolver @ Illuminate/Events/Dispatcher.php:840
        Method      shouldBeStrict @ Illuminate/Database/Eloquent/Model.php:566
        Method      str @ Illuminate/Support/Traits/InteractsWithData.php:273
        Method      straightJoin @ Illuminate/Database/Query/Builder.php:845
        Method      straightJoinSub @ Illuminate/Database/Query/Builder.php:874
        Method      straightJoinWhere @ Illuminate/Database/Query/Builder.php:859
        Method      stream @ Illuminate/Contracts/Routing/ResponseFactory.php:80
        Method      stream @ Illuminate/Routing/ResponseFactory.php:197
        Method      streamDownload @ Illuminate/Contracts/Routing/ResponseFactory.php:102
        Method      streamDownload @ Illuminate/Routing/ResponseFactory.php:243
        Method      streamJson @ Illuminate/Contracts/Routing/ResponseFactory.php:91
        Method      streamJson @ Illuminate/Routing/ResponseFactory.php:227
        Method      streamedContent @ Illuminate/Testing/TestResponse.php:1954
        Method      strict @ Illuminate/Validation/Rules/Email.php:120
        Method      string @ Illuminate/Cache/Repository.php:242
        Method      string @ Illuminate/Collections/Arr.php:1171
        Method      string @ Illuminate/Config/Repository.php:89
        Method      string @ Illuminate/Contracts/JsonSchema/JsonSchema.php:28
        Method      string @ Illuminate/Database/Schema/Blueprint.php:849
        Method      string @ Illuminate/JsonSchema/JsonSchemaTypeFactory.php:34
        Method      string @ Illuminate/Support/Traits/InteractsWithData.php:285
        Method      string @ Illuminate/Translation/Translator.php:196
        Method      string @ Illuminate/Validation/Rule.php:289
        Method      stringable @ Illuminate/Translation/Translator.php:623
        Method      stringable @ Illuminate/View/Compilers/Concerns/CompilesEchos.php:23
        Method      stringifyAddresses @ Illuminate/Mail/Transport/CloudflareTransport.php:173
        Method      stringifyClosure @ Illuminate/Foundation/Console/EventListCommand.php:192
        Method      stripConditions @ Illuminate/Translation/MessageSelector.php:91
        Method      stripParentheses @ Illuminate/View/Compilers/BladeCompiler.php:700
        Method      stripQuotes @ Illuminate/View/Compilers/ComponentTagCompiler.php:806
        Method      stripTableForPluck @ Illuminate/Database/Query/Builder.php:3860
        Method      stripTags @ Illuminate/Support/Stringable.php:858
        Method      substr @ Illuminate/Support/Str.php:1758
        Method      substr @ Illuminate/Support/Stringable.php:1012
        Method      substrCount @ Illuminate/Support/Str.php:1772
        Method      substrCount @ Illuminate/Support/Stringable.php:1025
        Method      substrReplace @ Illuminate/Support/Str.php:1790
        Method      substrReplace @ Illuminate/Support/Stringable.php:1038
        Method      supportsStraightJoins @ Illuminate/Database/Query/Grammars/Grammar.php:228
        Method      supportsStraightJoins @ Illuminate/Database/Query/Grammars/MySqlGrammar.php:451
        Method      syncMiddlewareToRouter @ Illuminate/Foundation/Http/Kernel.php:520
        Method      throwFirstReported @ Illuminate/Support/Testing/Fakes/ExceptionHandlerFake.php:245
        Method      toHtmlString @ Illuminate/Support/Stringable.php:1398
        Method      toPasswordRulesString @ Illuminate/Validation/Rules/Password.php:440
        Method      toString @ Illuminate/JsonSchema/Types/Type.php:119
        Method      toString @ Illuminate/Support/Stringable.php:1485
        Method      toString @ Illuminate/Support/Uri.php:406
        Method      toString @ Illuminate/Testing/Constraints/ArraySubset.php:92
        Method      toString @ Illuminate/Testing/Constraints/CountInDatabase.php:77
        Method      toString @ Illuminate/Testing/Constraints/HasInDatabase.php:113
        Method      toString @ Illuminate/Testing/Constraints/NotSoftDeletedInDatabase.php:109
        Method      toString @ Illuminate/Testing/Constraints/SeeInHtml.php:132
        Method      toString @ Illuminate/Testing/Constraints/SeeInOrder.php:86
        Method      toString @ Illuminate/Testing/Constraints/SoftDeletedInDatabase.php:111
        Method      toString @ Illuminate/Translation/PotentiallyTranslatedString.php:96
        Method      toStringOr @ Illuminate/Support/Str.php:1225
        Method      toStringable @ Illuminate/Support/Uri.php:357
        Method      trimStrings @ Illuminate/Foundation/Configuration/Middleware.php:661
        Method      typeString @ Illuminate/Database/Schema/Grammars/MySqlGrammar.php:767
        Method      typeString @ Illuminate/Database/Schema/Grammars/PostgresGrammar.php:786
        Method      typeString @ Illuminate/Database/Schema/Grammars/SQLiteGrammar.php:718
        Method      typeString @ Illuminate/Database/Schema/Grammars/SqlServerGrammar.php:576
        Method      uniqueStrict @ Illuminate/Collections/Enumerable.php:1227
        Method      uniqueStrict @ Illuminate/Collections/Traits/EnumeratesValues.php:959
        Method      useBootstrap @ Illuminate/Pagination/AbstractPaginator.php:627
        Method      useBootstrapFive @ Illuminate/Pagination/AbstractPaginator.php:659
        Method      useBootstrapFour @ Illuminate/Pagination/AbstractPaginator.php:648
        Method      useBootstrapPath @ Illuminate/Foundation/Application.php:501
        Method      useBootstrapThree @ Illuminate/Pagination/AbstractPaginator.php:637
        Method      usePrefetchStrategy @ Illuminate/Foundation/Vite.php:363
        Method      validateString @ Illuminate/Validation/Concerns/ValidatesAttributes.php:2721
        Method      whereInStrict @ Illuminate/Collections/Enumerable.php:424
        Method      whereInStrict @ Illuminate/Collections/Traits/EnumeratesValues.php:715
        Method      whereNotInStrict @ Illuminate/Collections/Enumerable.php:461
        Method      whereNotInStrict @ Illuminate/Collections/Traits/EnumeratesValues.php:768
        Method      whereStrict @ Illuminate/Collections/Enumerable.php:405
        Method      whereStrict @ Illuminate/Collections/Traits/EnumeratesValues.php:688
        Method      withQueryString @ Illuminate/Contracts/Pagination/CursorPaginator.php:43
        Method      withQueryString @ Illuminate/Contracts/Pagination/Paginator.php:43
        Method      withQueryString @ Illuminate/Pagination/AbstractCursorPaginator.php:329
        Method      withQueryString @ Illuminate/Pagination/AbstractPaginator.php:259
        Method      withoutForeignKeyConstraints @ Illuminate/Database/Schema/Builder.php:654
        Method      writeStream @ Illuminate/Contracts/Filesystem/Filesystem.php:91
        Method      writeStream @ Illuminate/Filesystem/FilesystemAdapter.php:715
        Property    $abstractAliases @ Illuminate/Container/Container.php:82
        Property    $allowedStrayRequestUrls @ Illuminate/Http/Client/Factory.php:90
        Property    $allowedStrayRequestUrls @ Illuminate/Http/Client/PendingRequest.php:195
        Property    $alwaysTrust @ Illuminate/Http/Middleware/TrustHosts.php:21
        Property    $alwaysTrustHeaders @ Illuminate/Http/Middleware/TrustProxies.php:40
        Property    $alwaysTrustProxies @ Illuminate/Http/Middleware/TrustProxies.php:33
        Property    $assertionableRenderStrings @ Illuminate/Mail/Mailable.php:186
        Property    $beforeApplicationDestroyedCallbacks @ Illuminate/Foundation/Testing/Concerns/InteractsWithTestCaseLifecycle.php:72
        Property    $bindingRegistrar @ Illuminate/Broadcasting/Broadcasters/Broadcaster.php:46
        Property    $bootstrapPath @ Illuminate/Foundation/Application.php:124
        Property    $bootstrapProviderPath @ Illuminate/Foundation/Bootstrap/RegisterProviders.php:21
        Property    $bootstrappers @ Illuminate/Console/Application.php:52
        Property    $bootstrappers @ Illuminate/Foundation/Console/Kernel.php:119
        Property    $bootstrappers @ Illuminate/Foundation/Http/Kernel.php:42
        Property    $connectionsTransacting @ Illuminate/Foundation/Testing/DatabaseTransactionsManager.php:11
        Property    $constraints @ Illuminate/Database/Eloquent/Relations/Relation.php:62
        Property    $constraints @ Illuminate/Validation/Rules/Date.php:22
        Property    $constraints @ Illuminate/Validation/Rules/Dimensions.php:16
        Property    $constraints @ Illuminate/Validation/Rules/Numeric.php:15
        Property    $constraints @ Illuminate/Validation/Rules/StringRule.php:15
        Property    $constructorParametersCache @ Illuminate/View/Component.php:77
        Property    $defaultStringLength @ Illuminate/Database/Schema/Builder.php:43
        Property    $escapeWhenCastingToString @ Illuminate/Collections/Traits/EnumeratesValues.php:67
        Property    $escapeWhenCastingToString @ Illuminate/Database/Eloquent/Model.php:139
        Property    $escapeWhenCastingToString @ Illuminate/Pagination/AbstractPaginator.php:80
        Property    $expectedOutputSubstrings @ Illuminate/Foundation/Testing/Concerns/InteractsWithConsole.php:36
        Property    $hasBeenBootstrapped @ Illuminate/Foundation/Application.php:68
        Property    $htmlString @ Illuminate/Mail/Mailables/Content.php:45
        Property    $lastRendered @ Illuminate/View/Engines/Engine.php:11
        Property    $morphableConstraints @ Illuminate/Database/Eloquent/Relations/MorphTo.php:75
        Property    $prefetchStrategy @ Illuminate/Foundation/Vite.php:118
        Property    $prepareStringsForCompilationUsing @ Illuminate/View/Compilers/BladeCompiler.php:69
        Property    $preventStrayProcesses @ Illuminate/Process/Factory.php:42
        Property    $preventStrayRequests @ Illuminate/Http/Client/Factory.php:83
        Property    $preventStrayRequests @ Illuminate/Http/Client/PendingRequest.php:188
        Property    $queryStringResolver @ Illuminate/Pagination/AbstractPaginator.php:115
        Property    $randomStringFactory @ Illuminate/Support/Str.php:73
        Property    $registrar @ Illuminate/Routing/PendingResourceRegistration.php:16
        Property    $registrar @ Illuminate/Routing/PendingSingletonResourceRegistration.php:16
        Property    $registry @ Illuminate/Console/Signals.php:14
        Property    $scriptTagAttributesResolvers @ Illuminate/Foundation/Vite.php:69
        Property    $sizeToReport @ Illuminate/Http/Testing/File.php:27
        Property    $streamedContent @ Illuminate/Testing/TestResponse.php:68
        Property    $strict @ Illuminate/Testing/Constraints/ArraySubset.php:20
        Property    $strictRfcCompliant @ Illuminate/Validation/Rules/Email.php:22
        Property    $string @ Illuminate/Translation/PotentiallyTranslatedString.php:13
        Property    $stringCallbacks @ Illuminate/Auth/Access/Gate.php:71
        Property    $stringableHandlers @ Illuminate/Translation/Translator.php:67
        Property    $styleTagAttributesResolvers @ Illuminate/Foundation/Vite.php:76
        Property    $unexpectedOutputSubstrings @ Illuminate/Foundation/Testing/Concerns/InteractsWithConsole.php:50
        Property    $usesRequestQueryString @ Illuminate/Http/Resources/JsonApi/Concerns/ResolvesJsonApiElements.php:29"#]].assert_eq(&out);
}

// ── Code Actions ──────────────────────────────────────────────────────────────

/// Code actions on a class declaration line return at least one action.
/// Guards against code actions returning empty on real-world classes.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_code_actions_class_declaration() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 17 (0-based) = class declaration.
    let resp = s
        .code_action("Illuminate/Auth/AuthManager.php", 17, 0, 17, 50)
        .await;
    expect![[r#"
        refactor         Generate 4 getters/setters
        refactor         Promote constructor parameter
        refactor.extract Extract interface 'AuthManagerInterface' [edit]"#]]
    .assert_eq(&render_code_actions(&resp));
}

// ── Signature Help ────────────────────────────────────────────────────────────

/// Signature help inside a function call shows the function's parameter list.
/// Guards against signature help returning no signatures on real code.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_signature_help_inside_call() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    // Use a self-contained synthetic file within the Laravel namespace so
    // PSR-4 resolution still applies.
    let src = "<?php\nnamespace Illuminate\\Auth;\n\
               function make_guard(string $name, array $config): string { return ''; }\n\
               $g = make_guard(\n";
    s.open("Illuminate/Auth/__test_sighel.php", src).await;

    // Line 3 (0-based) = `$g = make_guard(` — cursor after `(`.
    let resp = s
        .signature_help("Illuminate/Auth/__test_sighel.php", 3, 16)
        .await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    let out = render_signature_help(&resp);
    expect!["▶ make_guard(string $name, array $config)  @param0"].assert_eq(&out);
}

// ── Inlay Hints ───────────────────────────────────────────────────────────────

/// Inlay hints for the AuthManager method bodies are returned without timeout.
/// Guards against inlay hint requests stalling on real-world files.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_inlay_hints_method_bodies() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    let auth_line_count = read("Illuminate/Auth/AuthManager.php").lines().count() as u32;
    let resp = s
        .inlay_hints("Illuminate/Auth/AuthManager.php", 0, 0, auth_line_count, 0)
        .await;
    expect![[r#"
        60:65 name: [param]
        73:55 name: [param]
        86:35 name: [param]
        93:44 name: [param]
        93:51 config: [param]
        96:41 string: [param]
        129:12 name: [param]
        130:12 provider: [param]
        130:38 provider: [param]
        131:12 session: [param]
        132:53 name: [param]
        133:55 name: [param]
        134:47 name: [param]
        140:29 cookie: [param]
        142:30 events: [param]
        144:27 request: [param]
        144:47 seconds: [param]
        147:40 minutes: [param]
        166:12 provider: [param]
        166:38 provider: [param]
        167:12 request: [param]
        168:12 inputKey: [param]
        169:12 storageKey: [param]
        170:12 hash: [param]
        173:28 seconds: [param]
        209:32 name: [param]
        211:64 name: [param]
        234:29 driver: [param]
        234:38 callback: [param]
        235:38 callback: [param]
        235:49 request: [param]
        235:72 provider: [param]
        237:32 seconds: [param]
        279:50 callback: [param]"#]]
    .assert_eq(&render_inlay_hints(&resp));
}

/// Inlay hints for a file with inferred parameter types actually contain labels.
/// Guards against inlay hints always returning an empty array on real code.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_inlay_hints_content() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;

    // Use a synthetic file with obvious parameter-name hints to check content.
    let src = "<?php\nuse Illuminate\\Support\\Str;\nStr::camel('hello_world');\n";
    s.open("__test_inlay_hints.php", src).await;

    let resp = s.inlay_hints("__test_inlay_hints.php", 0, 0, 3, 0).await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    expect!["2:11 value: [param]"].assert_eq(&render_inlay_hints(&resp));
}

// ── Rename ────────────────────────────────────────────────────────────────────

/// Renaming the `AuthManager` class at its declaration produces a workspace edit.
/// Guards against rename returning null.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_rename_class_declaration() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Line 17 (0-based), character 6 = "AuthManager".
    let resp = s
        .rename("Illuminate/Auth/AuthManager.php", 17, 6, "AuthManagerV2")
        .await;
    assert!(resp["error"].is_null(), "rename returned error: {resp:#}");
    let out = canonicalize_workspace_edit(&resp["result"], &s.uri(""));
    // Application.php's `\Illuminate\Auth\AuthManager::class` alias entry is a
    // fully-qualified reference the old text walker missed; the posting-based
    // rename narrows the edit to the final `AuthManager` segment.
    expect![[r#"
        // Illuminate/Auth/AuthManager.php
        17:6-17:17 → "AuthManagerV2"

        // Illuminate/Auth/AuthServiceProvider.php
        36:55-36:66 → "AuthManagerV2"

        // Illuminate/Foundation/Application.php
        1639:40-1639:51 → "AuthManagerV2""#]]
    .assert_eq(&out);
}

// ── Find Implementations ─────────────────────────────────────────────────────

/// `Factory::guard()` is an interface method; implementations should include
/// `AuthManager::guard()` from the concrete class.
///
/// AuthManager writes `implements FactoryContract` where `FactoryContract` is a
/// use-import alias for `Illuminate\Contracts\Auth\Factory`. The workspace index
/// resolves the alias so `subtypes_of["Factory"]` includes AuthManager.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_find_implementations_interface_method() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Contracts/Auth/Factory.php",
        &read("Illuminate/Contracts/Auth/Factory.php"),
    )
    .await;

    // Line 12 (0-based) = `    public function guard($name = null);`
    // Character 20 = "guard".
    let resp = s
        .implementation("Illuminate/Contracts/Auth/Factory.php", 12, 20)
        .await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    let out = render_locations(&resp, &s.uri(""));
    expect!["Illuminate/Auth/AuthManager.php:69:20-69:25"].assert_eq(&out);
}

// ── Type Hierarchy ───────────────────────────────────────────────────────────

/// Supertypes of `AuthManager` includes the `Factory` interface.
///
/// `typeHierarchy/supertypes` for `AuthManager` returns `Factory` (resolved
/// through the `FactoryContract` use-import alias). Fixed in `type_hierarchy.rs`
/// by resolving use-import aliases in `supertypes_of_from_workspace`.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_type_hierarchy_supertypes() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;

    // Prepare type hierarchy at the AuthManager class name (line 17, char 6).
    let prep = s
        .prepare_type_hierarchy("Illuminate/Auth/AuthManager.php", 17, 6)
        .await;
    assert!(
        prep["error"].is_null(),
        "prepareTypeHierarchy error: {prep:#}"
    );
    let items = prep["result"].as_array().expect("array result");
    assert!(!items.is_empty(), "prepareTypeHierarchy returned no items");

    let supers = s.supertypes(items[0].clone()).await;
    assert!(supers["error"].is_null(), "supertypes error: {supers:#}");
    let names: Vec<&str> = supers["result"]
        .as_array()
        .map(|a| a.iter().filter_map(|i| i["name"].as_str()).collect())
        .unwrap_or_default();
    expect!["Factory, CreatesUserProviders, RebindsCallbacksToSelf"].assert_eq(&names.join(", "));
}

/// Subtypes of the `Factory` interface includes `AuthManager`.
/// Guards against type hierarchy subtypes returning empty after workspace index.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_type_hierarchy_subtypes() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Contracts/Auth/Factory.php",
        &read("Illuminate/Contracts/Auth/Factory.php"),
    )
    .await;

    // Line 4 (0-based) = `interface Factory`; character 10 = "Factory".
    let prep = s
        .prepare_type_hierarchy("Illuminate/Contracts/Auth/Factory.php", 4, 10)
        .await;
    assert!(
        prep["error"].is_null(),
        "prepareTypeHierarchy error: {prep:#}"
    );
    let items = prep["result"].as_array().expect("array result");
    assert!(!items.is_empty(), "prepareTypeHierarchy returned no items");

    let subs = s.subtypes(items[0].clone()).await;
    assert!(subs["error"].is_null(), "subtypes error: {subs:#}");
    let names: Vec<&str> = subs["result"]
        .as_array()
        .map(|a| a.iter().filter_map(|i| i["name"].as_str()).collect())
        .unwrap_or_default();
    expect!["AuthManager"].assert_eq(&names.join(", "));
}

/// `textDocument/implementation` on the `Factory` interface name returns
/// `AuthManager` (the concrete implementor).
///
/// Find implementations on the `Factory` interface name includes `AuthManager`
/// (which implements it via the `FactoryContract` alias). Fixed in
/// `implementation.rs` by resolving use-import aliases in the implements check.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_find_implementations_interface_name() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;
    s.open(
        "Illuminate/Contracts/Auth/Factory.php",
        &read("Illuminate/Contracts/Auth/Factory.php"),
    )
    .await;

    // Line 4 (0-based) = `interface Factory`; character 10 = "Factory".
    let resp = s
        .implementation("Illuminate/Contracts/Auth/Factory.php", 4, 10)
        .await;
    assert!(resp["error"].is_null(), "error: {resp:#}");
    let out = render_locations(&resp, &s.uri(""));
    expect!["Illuminate/Auth/AuthManager.php:17:6-17:17"].assert_eq(&out);
}

// ── Under-load stability ──────────────────────────────────────────────────────

/// Open multiple large files concurrently, then verify each feature still
/// responds in time.  Protects against the server request queue serializing
/// or salsa contention causing timeouts when several heavy files are open.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn laravel_features_stable_with_multiple_open_files() {
    if !laravel_available() {
        return;
    }
    let mut s = TestServer::with_root(LARAVEL_SRC).await;
    s.wait_for_index_ready_secs(60).await;

    // Open four large files sequentially; each open() waits for diagnostics
    // (parse + analysis complete) before returning, so requests afterwards
    // do not race against in-progress work.
    s.open(
        "Illuminate/Auth/AuthManager.php",
        &read("Illuminate/Auth/AuthManager.php"),
    )
    .await;
    s.open(
        "Illuminate/Database/Eloquent/Model.php",
        &read("Illuminate/Database/Eloquent/Model.php"),
    )
    .await;
    s.open(
        "Illuminate/Support/Str.php",
        &read("Illuminate/Support/Str.php"),
    )
    .await;
    s.open(
        "Illuminate/Contracts/Auth/Factory.php",
        &read("Illuminate/Contracts/Auth/Factory.php"),
    )
    .await;

    // Hover must complete without timeout and return real content.
    let hover = s.hover("Illuminate/Auth/AuthManager.php", 69, 20).await;
    let hover_out = render_hover(&hover);
    assert!(
        !hover_out.contains("error:") && hover_out != "<no hover>",
        "hover failed or returned nothing under load: {hover_out}"
    );

    // Document symbols must complete without timeout and return real symbols.
    let syms = s.document_symbols("Illuminate/Auth/AuthManager.php").await;
    let syms_out = render_document_symbols(&syms);
    assert!(
        !syms_out.contains("error:") && syms_out != "<no symbols>",
        "documentSymbol failed or returned nothing under load: {syms_out}"
    );

    // Completion must complete without timeout and offer real items.
    let comp = s
        .completion("Illuminate/Auth/AuthManager.php", 59, 15)
        .await;
    assert!(
        comp["error"].is_null(),
        "completion failed under load: {comp:#}"
    );
    let items = comp["result"]["items"]
        .as_array()
        .or_else(|| comp["result"].as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(items >= 1, "completion returned 0 items under load");

    // Workspace symbols must complete without timeout and find a Guard-related symbol.
    let ws = s.workspace_symbols("Guard").await;
    let ws_out = render_workspace_symbols(&ws, &s.uri(""));
    assert!(
        ws_out.to_lowercase().contains("guard"),
        "workspace/symbol failed or found no Guard match under load: {ws_out}"
    );

    // References must complete without timeout and return real locations.
    let refs = s
        .references("Illuminate/Auth/AuthManager.php", 69, 20, true)
        .await;
    let refs_out = render_locations(&refs, &s.uri(""));
    assert!(
        !refs_out.contains("error:") && refs_out != "<none>",
        "references failed or returned nothing under load: {refs_out}"
    );
}
