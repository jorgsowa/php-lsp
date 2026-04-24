//! Hover tests. Uses `$0` cursor markers so the tested position sits
//! visibly in the source instead of as a `(line, col)` literal.

mod common;

use common::TestServer;

async fn hover_value(server: &mut TestServer, fixture: &str) -> String {
    let opened = server.open_fixture(fixture).await;
    let c = opened.cursor();
    let resp = server.hover(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "hover errored: {resp:?}");
    assert!(!resp["result"].is_null(), "hover returned null");
    resp["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn hover_on_opened_document() {
    let mut server = TestServer::new().await;
    let value = hover_value(
        &mut server,
        r#"<?php
function gr$0eet(string $name): string { return $name; }
"#,
    )
    .await;
    assert!(value.contains("greet"), "hover must show 'greet': {value}");
}

/// Variable type from one method body must not appear in hover for the same
/// variable name in a different method body (scope pollution via flat TypeMap).
#[tokio::test]
async fn hover_variable_type_is_scoped_to_enclosing_method() {
    let mut server = TestServer::new().await;
    let value = hover_value(
        &mut server,
        r#"<?php
class Widget {}
class Invoice {}
class Service {
    public function methodA(): void { $result = new Widget(); }
    public function methodB(): void { $res$0ult = new Invoice(); }
}
"#,
    )
    .await;
    assert!(
        !value.contains("Widget"),
        "Widget from methodA must not appear in methodB hover: {value}"
    );
    assert!(
        value.contains("Invoice"),
        "Invoice from methodB should appear: {value}"
    );
}

/// Hovering `$obj->method()` must show the signature from the receiver's
/// resolved class, not the first class with that method name.
#[tokio::test]
async fn hover_method_call_resolves_receiver_class() {
    let mut server = TestServer::new().await;
    let value = hover_value(
        &mut server,
        r#"<?php
class Mailer { public function process(string $to): bool {} }
class Queue  { public function process(int $id): void {} }
$mailer = new Mailer();
$mailer->pro$0cess('');
"#,
    )
    .await;
    assert!(
        value.contains("Mailer"),
        "hover should show Mailer::process: {value}"
    );
    assert!(
        !value.contains("int $id"),
        "must NOT show Queue::process params: {value}"
    );
}
