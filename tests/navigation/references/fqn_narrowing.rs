//! FQCN-precise candidate narrowing for public static methods (and
//! class/function/global-constant symbols): `reference_candidate_files`
//! narrows via `DocumentStore::fqn_reachable_files` — same namespace, a
//! matching `use` import, or a literal fully-qualified mention — instead of
//! handing mir the whole workspace. Each test below is reachable through
//! exactly one of those three rules (or, for the regression guards, must
//! NOT be narrowed at all), so a bug in any one rule fails a specific test.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn narrowing_finds_aliased_import_call_site() {
    // `caller.php` never mentions `Widget` at all — only reachable via the
    // `use ... as` import-match rule.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.php"),
        "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\nuse App\\Widget as Base;\nBase::make();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "widget.php",
            "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("widget.php", "make", 0);
    let resp = server.references("widget.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:3:6-3:10"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn narrowing_finds_fully_qualified_inline_call_site() {
    // `caller.php` is in an unrelated namespace with no `use` import — only
    // reachable via the literal `\App\Widget` text-mention rule.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.php"),
        "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\n\\App\\Widget::make();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "widget.php",
            "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("widget.php", "make", 0);
    let resp = server.references("widget.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:2:13-2:17"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn narrowing_finds_same_namespace_unqualified_call_site() {
    // `caller.php` shares `Widget`'s namespace, calls it unqualified with no
    // `use` import — only reachable via the namespace-match rule.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.php"),
        "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace App;\nWidget::make();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "widget.php",
            "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("widget.php", "make", 0);
    let resp = server.references("widget.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:2:8-2:12"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn narrowing_finds_inherited_static_call_via_subclass_import() {
    // `caller.php` imports only `Sub` (a subclass of `Widget`) — never
    // `Widget` itself — and calls the inherited, unoverridden static method
    // via `Sub`. Only reachable because the owner+subtype FQCN set includes
    // `Sub`, proving subtype-closure inclusion, not just the owner.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.php"),
        "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("sub.php"),
        "<?php\nnamespace App;\nclass Sub extends Widget {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\nuse App\\Sub;\nSub::make();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "widget.php",
            "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("widget.php", "make", 0);
    let resp = server.references("widget.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:3:5-3:9"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn narrowing_disambiguates_same_short_name_different_namespaces() {
    // Two unrelated classes both named `Color`, each with a same-named
    // static method — the whole motivating scenario for this narrowing.
    // References on one must find only its own call site.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("color_a.php"),
        "<?php\nnamespace App\\Alpha;\nclass Color {\n    public static function from(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("color_b.php"),
        "<?php\nnamespace App\\Beta;\nclass Color {\n    public static function from(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller_a.php"),
        "<?php\nnamespace App\\Alpha;\nColor::from();\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller_b.php"),
        "<?php\nnamespace App\\Beta;\nColor::from();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "color_a.php",
            "<?php\nnamespace App\\Alpha;\nclass Color {\n    public static function from(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("color_a.php", "from", 0);
    let resp = server.references("color_a.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller_a.php:2:7-2:11"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Regression guard: an **instance** method must NOT be narrowed by FQCN —
/// `$this->svc->process()` can reference an instance method without ever
/// naming its class in the calling file (here, via a property whose type is
/// declared only on a *parent* class), the exact shape that makes FQCN
/// narrowing unsound for instance members. If narrowing were mistakenly
/// applied here, `caller.php` (which never imports/namespaces/qualifies
/// `Service`) would be wrongly excluded and this reference would go missing.
#[tokio::test]
async fn instance_method_is_not_fqn_narrowed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("service.php"),
        "<?php\nnamespace App;\nclass Service {\n    public function process(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("base.php"),
        "<?php\nnamespace Other;\nuse App\\Service;\nclass Base {\n    protected Service $svc;\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\nclass Caller extends Base {\n    public function run(): void {\n        $this->svc->process();\n    }\n}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "service.php",
            "<?php\nnamespace App;\nclass Service {\n    public function process(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("service.php", "process", 0);
    let resp = server.references("service.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:4:20-4:27"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Regression guard: a **global-namespace** function must NOT be narrowed —
/// PHP's fallback resolution lets any namespaced file call it unqualified
/// (`namespace App; helper();` resolves to `\helper` when `App\helper`
/// doesn't exist), so the caller matches none of the three narrowing rules.
#[tokio::test]
async fn global_function_is_not_fqn_narrowed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("helper.php"),
        "<?php\nfunction helper(): void {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace App;\nhelper();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open("helper.php", "<?php\nfunction helper(): void {}\n")
        .await;

    let (_, line, ch) = server.locate("helper.php", "helper", 0);
    let resp = server.references("helper.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:2:0-2:6"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn narrowing_finds_relative_qualified_reference() {
    // `caller.php` sits in namespace `Foo` and reaches `Foo\Bar\Baz` through
    // the relative-qualified `Bar\Baz` — no import, no FQN mention. Only
    // reachable via the namespace-segment-prefix rule.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("baz.php"),
        "<?php\nnamespace Foo\\Bar;\nclass Baz {\n    public static function make(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Foo;\nBar\\Baz::make();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "baz.php",
            "<?php\nnamespace Foo\\Bar;\nclass Baz {\n    public static function make(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("baz.php", "make", 0);
    let resp = server.references("baz.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:2:9-2:13"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn narrowing_finds_prefix_import_reference() {
    // `use Foo\Bar;` then `Bar\Baz::make()` — the import names a namespace
    // *prefix* of the target, not the target itself. Only reachable via the
    // import-segment-prefix rule.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("baz.php"),
        "<?php\nnamespace Foo\\Bar;\nclass Baz {\n    public static function make(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\nuse Foo\\Bar;\nBar\\Baz::make();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "baz.php",
            "<?php\nnamespace Foo\\Bar;\nclass Baz {\n    public static function make(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("baz.php", "make", 0);
    let resp = server.references("baz.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:3:9-3:13"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn narrowing_finds_bare_qualified_mention_from_global_namespace_file() {
    // A file with NO namespace declaration resolves `App\Widget` from the
    // root without a leading `\` — the text rule must not require one.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.php"),
        "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nApp\\Widget::make();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "widget.php",
            "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("widget.php", "make", 0);
    let resp = server.references("widget.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:1:12-1:16"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn constructor_narrowing_finds_new_via_aliased_import() {
    // `__construct` gets the same owner+subtype FQN narrowing as statics —
    // this caller instantiates through an alias, never mentioning `Widget`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.php"),
        "<?php\nnamespace App;\nclass Widget {\n    public function __construct() {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\nuse App\\Widget as Base;\nnew Base();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "widget.php",
            "<?php\nnamespace App;\nclass Widget {\n    public function __construct() {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("widget.php", "__construct", 0);
    let resp = server.references("widget.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:3:4-3:8"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Soundness guard: PHP allows a static call through an *instance* receiver
/// (`$obj::make()`), and the receiver's type can come from a property
/// declared only on a parent class in another file — so `caller.php` never
/// names `Widget` at all, and mir DOES record the call under the owner's
/// method key (verified empirically). FQN reachability alone would drop
/// this site; the member-name text needle unioned into the static scope is
/// what keeps it.
#[tokio::test]
async fn static_call_on_instance_receiver_typed_by_parent_property() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.php"),
        "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("base.php"),
        "<?php\nnamespace Other;\nuse App\\Widget;\nclass Base {\n    protected Widget $w;\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\nclass Caller extends Base {\n    public function run(): void {\n        $this->w::make();\n    }\n}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "widget.php",
            "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("widget.php", "make", 0);
    let resp = server.references("widget.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:4:18-4:22"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn narrowing_finds_class_symbol_via_aliased_import() {
    // Class symbol (not method) narrowing: `caller.php` never mentions
    // `Widget` textually, only via an aliased `use`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.php"),
        "<?php\nnamespace App;\nclass Widget {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\nuse App\\Widget as Base;\nfunction f(Base $b): void {}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open("widget.php", "<?php\nnamespace App;\nclass Widget {}\n")
        .await;

    let (_, line, ch) = server.locate("widget.php", "Widget", 0);
    let resp = server.references("widget.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect![[r#"
        caller.php:2:4-2:22
        caller.php:3:11-3:15"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Soundness guard, constructor flavor of the test above: an explicit
/// re-init call (`$obj->__construct()`) is recorded by mir (verified
/// empirically) and can live in a file that never names the class — the
/// receiver's type comes from a parent-declared property. The
/// `->__construct` text needle unioned into the constructor scope is what
/// keeps it; the bare word would drag in every file *declaring* a
/// constructor instead.
#[tokio::test]
async fn constructor_reinit_on_instance_receiver_typed_by_parent_property() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.php"),
        "<?php\nnamespace App;\nclass Widget {\n    public function __construct() {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("base.php"),
        "<?php\nnamespace Other;\nuse App\\Widget;\nclass Base {\n    protected Widget $w;\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\nclass Caller extends Base {\n    public function run(): void {\n        $this->w->__construct();\n    }\n}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "widget.php",
            "<?php\nnamespace App;\nclass Widget {\n    public function __construct() {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("widget.php", "__construct", 0);
    let resp = server.references("widget.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:4:18-4:29"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn narrowing_text_rule_is_case_insensitive() {
    // PHP resolves class names case-insensitively — a lowercased qualified
    // mention must still reach the file through the text rule.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.php"),
        "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\nuse App\\Widget;\n\\app\\widget::make();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "widget.php",
            "<?php\nnamespace App;\nclass Widget {\n    public static function make(): void {}\n}\n",
        )
        .await;

    let (_, line, ch) = server.locate("widget.php", "make", 0);
    let resp = server.references("widget.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:3:13-3:17"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn narrowing_finds_namespaced_function_via_use_function_import() {
    // A namespaced function reached through `use function` — the import rule
    // must match function imports, not just class ones.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("helper.php"),
        "<?php\nnamespace App\\Util;\nfunction helper(): void {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nnamespace Other;\nuse function App\\Util\\helper;\nhelper();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "helper.php",
            "<?php\nnamespace App\\Util;\nfunction helper(): void {}\n",
        )
        .await;

    let (_, line, ch) = server.locate("helper.php", "helper", 0);
    let resp = server.references("helper.php", line, ch, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect![[r#"
        caller.php:2:13-2:28
        caller.php:3:0-3:6"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}
