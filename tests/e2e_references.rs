mod common;

use common::TestServer;

#[tokio::test]
async fn references_with_exclude_declaration() {
    let mut server = TestServer::new().await;
    server
        .open(
            "refs.php",
            "<?php\nfunction sub(int $a, int $b): int { return $a - $b; }\nsub(10, 3);\n",
        )
        .await;

    let resp = server.references("refs.php", 1, 9, false).await;

    assert!(resp["error"].is_null(), "references error: {:?}", resp);
    let result = &resp["result"];
    assert!(result.is_array(), "expected an array, got: {:?}", result);
    let locs = result.as_array().unwrap();
    assert_eq!(
        locs.len(),
        1,
        "expected exactly 1 call-site reference, got: {:?}",
        locs
    );
    assert_eq!(locs[0]["range"]["start"]["line"].as_u64().unwrap(), 2);
    assert_eq!(locs[0]["range"]["start"]["character"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn references_include_declaration_returns_both() {
    let mut server = TestServer::new().await;
    server
        .open(
            "refs_incl.php",
            "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\n",
        )
        .await;

    let resp = server.references("refs_incl.php", 1, 9, true).await;

    assert!(resp["error"].is_null());
    let locs = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        locs.len() >= 2,
        "expected declaration + call site, got: {:?}",
        locs
    );
}
