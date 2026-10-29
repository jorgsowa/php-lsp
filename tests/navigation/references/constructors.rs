//! Constructor-specific reference tests: scope isolation, namespaces, exclusions, FQN ranges.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn references_on_constructor_with_include_declaration_false() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
class Invoice {
    public function __con$0struct(int $id) {}
}
$a = new Invoice(1);
$b = new Invoice(2);
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.references(&c.path, c.line, c.character, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    let out = render_locations(&resp, &server.uri(""));
    expect![[r#"
        main.php:4:9-4:16
        main.php:5:9-5:16"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn references_on_constructor_are_scoped_to_owning_class() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.php"),
        "<?php\nclass Foo {\n    public function __construct(int $x) {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.php"),
        "<?php\nclass Bar {\n    public function __construct(string $s) {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("c.php"),
        "<?php\n$foo = new Foo(1);\n$bar = new Bar('x');\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    server
        .open(
            "a.php",
            "<?php\nclass Foo {\n    public function __construct(int $x) {}\n}\n",
        )
        .await;

    let resp = server.references("a.php", 2, 22, true).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect![[r#"
        a.php:2:20-2:31
        c.php:1:11-1:14"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn references_on_constructor_scoped_by_namespace_fqn() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.php"),
        "<?php\nnamespace Alpha;\nclass Widget {\n    public function __construct(int $x) {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.php"),
        "<?php\nnamespace Beta;\nclass Widget {\n    public function __construct(string $s) {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("c.php"),
        "<?php\n$a = new \\Alpha\\Widget(1);\n$b = new \\Beta\\Widget('x');\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    server
        .open(
            "a.php",
            "<?php\nnamespace Alpha;\nclass Widget {\n    public function __construct(int $x) {}\n}\n",
        )
        .await;

    let resp = server.references("a.php", 3, 22, true).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect![[r#"
        a.php:3:20-3:31
        c.php:1:9-1:22"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}
