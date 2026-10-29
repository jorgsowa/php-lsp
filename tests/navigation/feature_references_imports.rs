//! References import collection tests (protocol-wired).
//! `use` imports for importable symbols should participate in find-references
//! just like regular usage sites.

use super::*;

#[tokio::test]
async fn references_handles_class_name_duplicate_with_member() {
    let mut s = TestServer::new().await;
    // Should find multiple references to Status (class + const member)
    s.check_references_annotated(
        r#"<?php
class Statu$0s {
//    ^^^^^^ def
    const Status = 1;
}
$x = Status::Status;
//   ^^^^^^ ref
"#,
    )
    .await;
}
