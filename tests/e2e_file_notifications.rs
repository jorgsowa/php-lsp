mod common;

use common::TestServer;
use std::time::{Duration, Instant};

async fn poll_until_symbol_present(server: &mut TestServer, query: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let resp = server.workspace_symbols(query).await;
        if resp["result"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {:?} waiting for '{}' to appear in workspace symbols",
            timeout,
            query
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn poll_until_symbol_uri_contains(
    server: &mut TestServer,
    query: &str,
    needle: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let resp = server.workspace_symbols(query).await;
        let found = resp["result"]
            .as_array()
            .map(|a| {
                a.iter().any(|s| {
                    s["location"]["uri"]
                        .as_str()
                        .map(|u| u.contains(needle))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if found {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {:?} waiting for '{}' with URI containing '{}' in workspace symbols",
            timeout,
            query,
            needle
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn poll_until_symbol_absent(server: &mut TestServer, query: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let resp = server.workspace_symbols(query).await;
        let empty = resp["result"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true);
        if empty {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {:?} waiting for '{}' to disappear from workspace symbols",
            timeout,
            query
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

#[tokio::test]
async fn did_rename_files_updates_index_to_new_path() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let pre = server.workspace_symbols("User").await;
    let pre_symbols = pre["result"].as_array().cloned().unwrap_or_default();
    assert!(
        pre_symbols
            .iter()
            .any(|s| s["name"].as_str() == Some("User")),
        "User must be indexed initially: {pre_symbols:?}"
    );

    let old_uri = server.uri("src/Model/User.php");
    let new_uri = server.uri("src/Entity/User.php");

    let (content, _, _) = server.locate("src/Model/User.php", "<?php", 0);
    server.write_file("src/Entity/User.php", &content);
    server.remove_file("src/Model/User.php");

    server
        .did_rename_files(vec![(old_uri.clone(), new_uri.clone())])
        .await;

    poll_until_symbol_uri_contains(
        &mut server,
        "User",
        "Entity/User.php",
        Duration::from_secs(3),
    )
    .await;

    let post = server.workspace_symbols("User").await;
    let post_symbols = post["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !post_symbols.iter().any(|s| {
            s["location"]["uri"]
                .as_str()
                .map(|u| u.contains("Model/User.php"))
                .unwrap_or(false)
        }),
        "old URI must no longer appear in workspace symbols after rename: {post_symbols:?}"
    );
    assert!(
        post_symbols.iter().any(|s| {
            s["location"]["uri"]
                .as_str()
                .map(|u| u.contains("Entity/User.php"))
                .unwrap_or(false)
        }),
        "new URI must appear in workspace symbols after rename: {post_symbols:?}"
    );
}

#[tokio::test]
async fn did_create_files_adds_new_class_to_index() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let pre = server.workspace_symbols("OrderRepo").await;
    assert!(
        pre["result"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "OrderRepo must not be indexed before creation"
    );

    server.write_file(
        "src/Repository/OrderRepo.php",
        "<?php\nnamespace App\\Repository;\nclass OrderRepo {}\n",
    );
    let new_uri = server.uri("src/Repository/OrderRepo.php");

    server.did_create_files(vec![new_uri]).await;

    poll_until_symbol_present(&mut server, "OrderRepo", Duration::from_secs(3)).await;

    let post = server.workspace_symbols("OrderRepo").await;
    let post_symbols = post["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !post_symbols.is_empty(),
        "OrderRepo must be discoverable after did_create_files: {post_symbols:?}"
    );
}

#[tokio::test]
async fn did_delete_files_removes_class_and_clears_diagnostics() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let (content, _, _) = server.locate("src/Model/User.php", "<?php", 0);
    server.open("src/Model/User.php", &content).await;

    let uri = server.uri("src/Model/User.php");
    server.remove_file("src/Model/User.php");

    let results = server.did_delete_files(vec![uri]).await;

    let diag_notif = &results[0];
    let diagnostics = diag_notif["params"]["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        diagnostics.is_empty(),
        "publishDiagnostics after deletion must be empty, got: {diagnostics:?}"
    );

    poll_until_symbol_absent(&mut server, "User", Duration::from_secs(3)).await;

    let post = server.workspace_symbols("User").await;
    let post_symbols = post["result"].as_array().cloned().unwrap_or_default();
    assert!(
        post_symbols.is_empty(),
        "User must be removed from workspace symbols after deletion: {post_symbols:?}"
    );
}
