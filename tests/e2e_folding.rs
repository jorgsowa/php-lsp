mod common;

use common::TestServer;

#[tokio::test]
async fn folding_ranges_returned_for_class() {
    let mut server = TestServer::new().await;
    server
        .open(
            "fold.php",
            "<?php\nclass Folded {\n    public function method(): void {\n        // body\n    }\n}\n",
        )
        .await;

    let resp = server.folding_range("fold.php").await;

    assert!(resp["error"].is_null(), "foldingRange error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        result.is_array(),
        "foldingRange must return an array: {:?}",
        result
    );
    let ranges = result.as_array().unwrap();
    assert_eq!(
        ranges.len(),
        2,
        "expected 2 fold ranges (class + method), got: {:?}",
        ranges
    );
    let start_lines: Vec<u64> = ranges
        .iter()
        .map(|r| r["startLine"].as_u64().unwrap())
        .collect();
    assert!(
        start_lines.contains(&1),
        "missing class fold starting at line 1"
    );
    assert!(
        start_lines.contains(&2),
        "missing method fold starting at line 2"
    );
}
