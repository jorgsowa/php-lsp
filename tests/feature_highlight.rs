//! documentHighlight coverage using the `ref`/`read`/`write` annotation tags.

mod common;

use common::TestServer;

#[tokio::test]
async fn highlight_variable_occurrences_within_function() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function f(): void {
    $name = 'x';
//  ^^^^^ write
    echo $na$0me;
//       ^^^^^ read
    $name .= '!';
//  ^^^^^ write
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_method_call_within_same_file() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
class Greeter {
    public function hel$0lo(): void {}
    //              ^^^^^ ref
}
$g = new Greeter();
$g->hello();
//  ^^^^^ ref
$g->hello();
//  ^^^^^ ref
"#,
    )
    .await;
}
