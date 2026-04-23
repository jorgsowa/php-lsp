//! E2E test proof-of-shape for the new harness.
//!
//! Compare with `src/backend.rs::integration::hover_on_opened_document` — the
//! builder collapses ~30 lines of JSON-RPC boilerplate and a `sleep(150ms)`
//! into three statements, and the sync is deterministic.

mod common;

use common::TestServer;

#[tokio::test]
async fn hover_on_opened_document() {
    let mut server = TestServer::new().await;
    server
        .open(
            "test.php",
            "<?php\nfunction greet(string $name): string { return $name; }\n",
        )
        .await;
    let resp = server.hover("test.php", 1, 10).await;

    assert!(resp["error"].is_null(), "hover errored: {:?}", resp);
    assert!(!resp["result"].is_null(), "hover returned null");
    let value = resp["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(
        value.contains("greet"),
        "hover must show 'greet', got: {value}"
    );
}

#[tokio::test]
async fn hover_with_cursor_marker() {
    let (src, line, character) = common::cursor("<?php\nfunction gr$0eet(): void {}\n");

    let mut server = TestServer::new().await;
    server.open("test.php", &src).await;

    let resp = server.hover("test.php", line, character).await;

    assert!(resp["error"].is_null());
    assert!(!resp["result"].is_null());
    let value = resp["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(value.contains("greet"), "hover value: {value}");
}

/// Gap 1: variable type from one method body must not appear in hover for the same
/// variable name in a different method body (scope pollution via flat TypeMap).
#[tokio::test]
async fn hover_variable_type_is_scoped_to_enclosing_method() {
    let src = concat!(
        "<?php\n",
        "class Widget {}\n",
        "class Invoice {}\n",
        "class Service {\n",
        "    public function methodA(): void { $result = new Widget(); }\n",
        "    public function methodB(): void { $result = new Invoice(); }\n",
        "}\n",
    );
    let mut server = TestServer::new().await;
    server.open("scope_test.php", src).await;

    let resp = server.hover("scope_test.php", 5, 40).await;

    assert!(
        resp["error"].is_null(),
        "hover should not error: {:?}",
        resp
    );
    assert!(
        !resp["result"].is_null(),
        "expected hover result, got null — document may not have been parsed yet"
    );
    let value = resp["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !value.contains("Widget"),
        "Widget from methodA must not appear in methodB hover, got: {}",
        value
    );
    assert!(
        value.contains("Invoice"),
        "Invoice from methodB should appear, got: {}",
        value
    );
}

/// Gap 2: hovering a method call site `$obj->method()` must show the signature
/// from the receiver's resolved class, not the first class with that method name.
#[tokio::test]
async fn hover_method_call_resolves_receiver_class() {
    let src = concat!(
        "<?php\n",
        "class Mailer { public function process(string $to): bool {} }\n",
        "class Queue  { public function process(int $id): void {} }\n",
        "$mailer = new Mailer();\n",
        "$mailer->process('');\n",
    );
    let mut server = TestServer::new().await;
    server.open("method_hover.php", src).await;

    let resp = server.hover("method_hover.php", 4, 12).await;

    assert!(
        resp["error"].is_null(),
        "hover should not error: {:?}",
        resp
    );
    assert!(
        !resp["result"].is_null(),
        "expected hover result on method call, got null"
    );
    let value = resp["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(
        value.contains("Mailer"),
        "hover should show Mailer::process, got: {}",
        value
    );
    assert!(
        !value.contains("int $id"),
        "must NOT show Queue::process params, got: {}",
        value
    );
}
