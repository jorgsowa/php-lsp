//! Class constants and global constant references.
//!
//! Tests constant declarations and usages:
//! - Class-level constants (const declarations)
//! - Global constants (const declarations and define())
//! - Constants accessed via ClassName::, self::, parent::, \Namespace\
//! - Constant in various contexts (expressions, type hints, default values)

use super::*;

#[tokio::test]
async fn constant_class_basic() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Status {
    const ACT$0IVE = 1;
    //    ^^^^^^ def
    const INACTIVE = 0;
}
$s = Status::ACTIVE;
//           ^^^^^^ ref
if ($val === Status::ACTIVE) {}
//                   ^^^^^^ ref
"#,
    )
    .await;
}
