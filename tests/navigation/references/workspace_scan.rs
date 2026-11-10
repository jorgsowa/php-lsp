//! Workspace scan, fast-path, and parallelism tests: cross-file indexing, consistency.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn references_fast_path_final_class_cross_file() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(
        dir.path().join("class.php"),
        "<?php\nfinal class Order {\n    public function submit(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\n$order = new Order();\n$order->submit();\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("ignored.php"),
        "<?php\n$unknown->submit();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    server
        .open(
            "class.php",
            "<?php\nfinal class Order {\n    public function submit(): void {}\n}\n",
        )
        .await;

    let resp = server.references("class.php", 2, 20, false).await;

    assert!(resp["error"].is_null(), "references error: {resp:?}");
    // Only caller.php (typed call) must appear; ignored.php (untyped) is excluded by fast path.
    expect!["caller.php:2:8-2:14"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn parallel_warm_finds_all_references_across_many_files() {
    let dir = tempfile::tempdir().unwrap();
    let caller_count = 15usize;
    std::fs::write(
        dir.path().join("def.php"),
        "<?php\nfunction target(): void {}",
    )
    .unwrap();
    for i in 0..caller_count {
        std::fs::write(
            dir.path().join(format!("caller_{i}.php")),
            "<?php\ntarget();",
        )
        .unwrap();
    }
    for i in 0..5usize {
        std::fs::write(
            dir.path().join(format!("other_{i}.php")),
            format!("<?php\nfunction other_{i}() {{}}"),
        )
        .unwrap();
    }

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open("def.php", "<?php\nfunction target(): void {}")
        .await;

    let resp = server.references("def.php", 1, 9, false).await;
    let out = render_locations(&resp, &server.uri(""));
    assert_eq!(
        out.lines().count(),
        caller_count,
        "expected {caller_count} references: {out}"
    );
    expect![[r#"
        caller_0.php:1:0-1:6
        caller_1.php:1:0-1:6
        caller_10.php:1:0-1:6
        caller_11.php:1:0-1:6
        caller_12.php:1:0-1:6
        caller_13.php:1:0-1:6
        caller_14.php:1:0-1:6
        caller_2.php:1:0-1:6
        caller_3.php:1:0-1:6
        caller_4.php:1:0-1:6
        caller_5.php:1:0-1:6
        caller_6.php:1:0-1:6
        caller_7.php:1:0-1:6
        caller_8.php:1:0-1:6
        caller_9.php:1:0-1:6"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn parallel_warm_gives_consistent_results_on_repeated_references_calls() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"//- /a.php
<?php
function fo$0o(): void {}

//- /b.php
<?php
foo();

//- /c.php
<?php
foo(); foo();
"#,
        )
        .await;
    let c = opened.cursor();

    let resp1 = server.references(&c.path, c.line, c.character, false).await;
    let resp2 = server.references(&c.path, c.line, c.character, false).await;

    let root = server.uri("");
    let out1 = render_locations(&resp1, &root);
    expect![[r#"
        b.php:1:0-1:3
        c.php:1:0-1:3
        c.php:1:7-1:10"#]]
    .assert_eq(&out1);
    assert_eq!(
        out1,
        render_locations(&resp2, &root),
        "repeated references calls must return identical results"
    );
}

/// Files that do not contain the symbol name at all must not appear in
/// results — the candidate prefilter (text_cache substring scan) should
/// exclude them before any parsing or AST work happens.
#[tokio::test]
async fn candidate_prefilter_excludes_files_not_mentioning_name() {
    let mut server = TestServer::new().await;
    server
        .open_fixture(
            r#"//- /ship.php
<?php
function launchRocket(): void {}

//- /mission.php
<?php
launchRocket();

//- /unrelated.php
<?php
// This file never mentions launchRocket at all
function countStars(): int { return 42; }
"#,
        )
        .await;

    let resp = server.references("ship.php", 1, 9, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["mission.php:1:0-1:12"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Method references: a file with the same method name but a different class
/// must not appear when the server has type information (scoped ingestion path).
#[tokio::test]
async fn scoped_ingestion_excludes_same_name_different_class() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(
        dir.path().join("probe.php"),
        "<?php\nfinal class SolarProbe {\n    public function transmit(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("radio.php"),
        "<?php\nfinal class RadioTower {\n    public function transmit(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\n$p = new SolarProbe();\n$p->transmit();\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("tower_caller.php"),
        "<?php\n$r = new RadioTower();\n$r->transmit();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "probe.php",
            "<?php\nfinal class SolarProbe {\n    public function transmit(): void {}\n}\n",
        )
        .await;

    // References on SolarProbe::transmit — caller.php must appear; tower_caller.php must not.
    let resp = server.references("probe.php", 2, 23, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:2:4-2:12"].assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn references_method_called_through_untyped_property_assigned_in_constructor() {
    let dir = tempfile::tempdir().unwrap();
    let context = "<?php\nnamespace App;\nclass Context {\n    public function getAll(): array { return []; }\n}\n";
    std::fs::write(dir.path().join("Context.php"), context).unwrap();
    std::fs::write(
        dir.path().join("Enumerator.php"),
        "<?php\nnamespace App;\nclass Enumerator {\n    private $context;\n    public function __construct(Context $context) { $this->context = $context; }\n    protected function vars(): array { return $this->context->getAll(); }\n}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("Context.php", context).await;

    let resp = server.references("Context.php", 3, 22, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["Enumerator.php:5:62-5:68"].assert_eq(&render_locations(&resp, &server.uri("")));
}
