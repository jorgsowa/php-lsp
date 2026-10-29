//! Static property references.
//!
//! Tests static property declarations and usages:
//! - ClassName::$property access
//! - self::$property within class scope
//! - parent::$property in inheritance
//! - Static properties with type hints
//! - Static properties in various expressions

use super::*;

#[tokio::test]
async fn static_property_type_safety() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Database {
    public static ?\PDO $connectio$0n = null;
    //                   ^^^^^^^^^^ def
}

class QueryBuilder {
    public function execute(): void {
        if (Database::$connection === null) {
        //             ^^^^^^^^^^ ref
            throw new Exception('Not connected');
        }

        Database::$connection->query('SELECT 1');
        //         ^^^^^^^^^^ ref
    }
}
"#,
    )
    .await;
}
