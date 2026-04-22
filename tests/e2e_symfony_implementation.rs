//! Find-implementations across symfony/demo: given an interface, the LSP
//! should surface concrete classes that implement it.
//!
//! Run with `cargo test --release -- --ignored`.

mod common;

use common::TestServer;

#[tokio::test]
#[ignore = "php-lsp gap: textDocument/implementation returns null on an interface reference in a `use`-importing file. Expected App\\Entity\\User to show up as implementor of UserInterface"]
async fn implementations_of_user_interface_include_app_user() {
    // `App\Entity\User implements UserInterface` — we ask the LSP for
    // implementors of `UserInterface` and verify our User entity shows up.
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Entity/User.php";
    let (text, line, character) = server.locate(path, "UserInterface", 1); // skip the `use` line
    server.open(path, &text).await;

    let resp = server.implementation(path, line, character).await;
    assert!(resp["error"].is_null(), "implementation error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected at least one implementation of UserInterface"
    );
    let arr = if result.is_array() {
        result.as_array().cloned().unwrap_or_default()
    } else {
        vec![result.clone()]
    };

    let has_app_user = arr.iter().any(|l| {
        let uri = l["uri"]
            .as_str()
            .or_else(|| l["targetUri"].as_str())
            .unwrap_or_default();
        uri.ends_with("/src/Entity/User.php")
    });
    assert!(
        has_app_user,
        "App\\Entity\\User should be listed as an implementor of UserInterface; got: {arr:?}"
    );
}
