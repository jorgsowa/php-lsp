mod common;

use common::TestServer;

#[tokio::test]
async fn code_lens_returned_for_function() {
    let mut server = TestServer::new().await;
    server
        .open("lens.php", "<?php\nfunction lensed(): void {}\nlensed();\n")
        .await;

    let resp = server.code_lens("lens.php").await;

    assert!(resp["error"].is_null(), "codeLens error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        result.is_array(),
        "codeLens must return an array: {:?}",
        result
    );
    let lenses = result.as_array().unwrap();
    assert!(!lenses.is_empty(), "expected at least one code lens");
    let has_ref_lens = lenses.iter().any(|l| {
        l["command"]["title"]
            .as_str()
            .map(|t| t.contains("reference"))
            .unwrap_or(false)
    });
    assert!(
        has_ref_lens,
        "expected a reference-count lens, got: {:?}",
        lenses
    );
}
