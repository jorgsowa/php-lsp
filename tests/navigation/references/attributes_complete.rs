//! Comprehensive attribute references across all declaration types.
//!
//! Tests attribute usage on:
//! - Classes, abstract classes, final classes
//! - Methods, static methods
//! - Properties, static properties
//! - Functions
//! - Parameters (function, method, property promotion)
//! - Enums and enum cases
//! - Multiple attributes on same element

use super::*;

#[tokio::test]
async fn attribute_on_static_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Singleton$0Factory {}
//    ^^^^^^^^^^^^^^^^ def

class Database {
    #[SingletonFactory]
    //^^^^^^^^^^^^^^^^ ref
    public static function getInstance(): self {
        static $instance = null;
        if ($instance === null) {
            $instance = new self();
        }
        return $instance;
    }
}
"#,
    )
    .await;
}
