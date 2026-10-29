//! Property reference tests: declared properties, promoted properties, access sites, nullsafe.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn references_on_promoted_property_cross_file() {
    let dir = tempfile::tempdir().unwrap();
    let entity_src = "<?php\nclass User {\n    public function __construct(public readonly string $email) {}\n}\n";
    std::fs::write(dir.path().join("entity.php"), entity_src).unwrap();
    std::fs::write(
        dir.path().join("service.php"),
        "<?php\nfunction notify(User $u): void {\n    echo $u->email;\n    echo $u?->email;\n}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    server.open("entity.php", entity_src).await;

    let resp = server.references("entity.php", 2, 56, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect![[r#"
        service.php:2:13-2:18
        service.php:3:14-3:19"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}
