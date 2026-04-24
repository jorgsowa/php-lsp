//! Comprehensive reference/find-usages coverage via the annotation DSL.
//!
//! Tests are written so the fixture itself specifies where references should
//! land — `// ^^^ def` for the declaration and `// ^^^ ref` for each use
//! site. `check_references_annotated` fails with a side-by-side diff if the
//! server returns anything missing or extra.

mod common;

use common::TestServer;

#[tokio::test]
async fn references_function_same_file() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function gr$0eet(): void {}
//       ^^^^^ def
greet();
//^^^^^ ref
greet();
//^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_method_same_file() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Greeter {
    public function he$0llo(): string { return 'hi'; }
    //              ^^^^^ def
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

#[tokio::test]
async fn references_static_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Reg {
    public static function ge$0t(): void {}
    //                     ^^^ def
}
Reg::get();
//   ^^^ ref
Reg::get();
//   ^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_cross_file_via_use() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /src/Greeter.php
<?php
namespace App;
class Greeter {
    public function hel$0lo(): string { return 'hi'; }
    //              ^^^^^ def
}

//- /src/main.php
<?php
use App\Greeter;
$g = new Greeter();
$g->hello();
//  ^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_no_usages_for_unused_function() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function un$0used(): void {}
//       ^^^^^^ def
"#,
    )
    .await;
}

#[tokio::test]
async fn references_class_used_in_new() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Wi$0dget {}
//    ^^^^^^ def
$a = new Widget();
//       ^^^^^^ ref
$b = new Widget();
//       ^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_distinguishes_like_named_methods() {
    // Two classes both define `process()`. Refs on Mailer::process must NOT
    // pick up Queue::process calls.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Mailer {
    public function pro$0cess(): void {}
    //              ^^^^^^^ def
}
class Queue {
    public function process(): void {}
}
$m = new Mailer();
$m->process();
//  ^^^^^^^ ref
$q = new Queue();
$q->process();
"#,
    )
    .await;
}

#[tokio::test]
async fn references_distinguishes_cross_namespace_functions() {
    // Two functions `greet` in different namespaces. Refs on `App\greet` must
    // NOT pick up the call to `Domain\greet`.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /src/app.php
<?php
namespace App;
function gr$0eet(): void {}
//       ^^^^^ def
greet();
//^^^^^ ref

//- /src/domain.php
<?php
namespace Domain;
function greet(): void {}
greet();
"#,
    )
    .await;
}

#[tokio::test]
async fn references_distinguishes_cross_namespace_classes() {
    // Two classes `User` in different namespaces. Refs on `App\User` must NOT
    // include the `new Domain\User()` site.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /src/app.php
<?php
namespace App;
class Us$0er {}
//    ^^^^ def
$a = new User();
//       ^^^^ ref

//- /src/domain.php
<?php
namespace Domain;
class User {}
$b = new User();
"#,
    )
    .await;
}

#[tokio::test]
async fn references_method_via_subclass_receiver_found() {
    // Method defined on a base class must also find calls on subclass receivers.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Base {
    public function wo$0rk(): void {}
    //              ^^^^ def
}
class Child extends Base {}
$c = new Child();
$c->work();
//  ^^^^ ref
"#,
    )
    .await;
}
