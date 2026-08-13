//! References import collection tests (protocol-wired).
//! `use` imports for importable symbols should participate in find-references
//! just like regular usage sites.

use super::*;

#[tokio::test]
async fn references_include_class_imports() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"//- /main.php
<?php
use App\Ba$0r;
//  ^^^^^^^ ref
use function App\helper;
use const App\LIMIT;

//- /App/Bar.php
<?php
namespace App;
class Bar {}
//    ^^^ def
"#,
    )
    .await;
}

#[tokio::test]
async fn references_include_function_imports() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"//- /main.php
<?php
use function App\he$0lper;
//           ^^^^^^^^^^ ref

//- /App/functions.php
<?php
namespace App;
function helper() {}
//       ^^^^^^ def
"#,
    )
    .await;
}

#[tokio::test]
async fn references_include_const_imports() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /main.php
<?php
use const App\LI$0MIT;
//        ^^^^^^^^^ ref

//- /App/constants.php
<?php
namespace App;
const LIMIT = 100;
//    ^^^^^ def
"#,
    )
    .await;
}

#[tokio::test]
async fn references_include_aliased_class_imports() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"//- /main.php
<?php
use App\Services\OldSe$0rvice as Service;
//  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ ref

//- /App/Services/OldService.php
<?php
namespace App\Services;
class OldService {}
//    ^^^^^^^^^^ def
"#,
    )
    .await;
}

#[tokio::test]
async fn references_distinguishes_class_constant_access() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Status {
    const ACTIVE = 1;
    //    ^^^^^^ def
}
$x = Status::AC$0TIVE;
//           ^^^^^^ ref
"#,
    )
    .await;
}

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

#[tokio::test]
async fn references_include_namespaced_imports() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"//- /main.php
<?php
namespace App;
use App\Services\Log$0ger;
//  ^^^^^^^^^^^^^^^^^^^ ref

//- /App/Services/Logger.php
<?php
namespace App\Services;
class Logger {}
//    ^^^^^^ def
"#,
    )
    .await;
}

#[tokio::test]
async fn references_include_imported_function_with_same_name_as_builtin() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"//- /main.php
<?php
use function App\str$0len;
//           ^^^^^^^^^^ ref

//- /App/functions.php
<?php
namespace App;
function strlen(string $value): int { return 0; }
//       ^^^^^^ def
"#,
    )
    .await;
}
