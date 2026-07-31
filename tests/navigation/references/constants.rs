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

#[tokio::test]
async fn constant_class_self_reference() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Config {
    const DEBU$0G = true;
    //    ^^^^^ def

    public static function isDebug(): bool {
        return self::DEBUG;
        //           ^^^^^ ref
    }

    public function check(): void {
        echo self::DEBUG ? 'debug' : 'prod';
        //         ^^^^^ ref
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_class_parent_reference() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Base {
    const VERS$0ION = '1.0';
    //    ^^^^^^^ def
}

class Extended extends Base {
    public function getVersion(): string {
        return parent::VERSION;
        //             ^^^^^^^ ref
    }
}

echo Extended::VERSION;
//             ^^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_class_cross_file() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /Config.php
<?php
class AppConfig {
    const TIMEO$0UT = 30;
    //    ^^^^^^^ def
    const MAX_RETRIES = 5;
}

//- /Client.php
<?php
$timeout = AppConfig::TIMEOUT;
//                    ^^^^^^^ ref

function retry() {
    for ($i = 0; $i < AppConfig::TIMEOUT; $i++) {
    //                           ^^^^^^^ ref
        try_request();
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_global_define_style() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
define('APP_VE$0RSION', '2.0.0');
//      ^^^^^^^^^^^ def

echo APP_VERSION;
//   ^^^^^^^^^^^ ref

if (defined('APP_VERSION')) {
    echo APP_VERSION;
    //   ^^^^^^^^^^^ ref
}
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_global_namespace_const() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
namespace App;

const MAX_S$0IZE = 1000;
//    ^^^^^^^^ def

function validate($input) {
    if (strlen($input) > MAX_SIZE) {
    //                   ^^^^^^^^ ref
        throw new Exception('too large');
    }
}

class Validator {
    public function check(string $s): bool {
        return strlen($s) <= MAX_SIZE;
        //                   ^^^^^^^^ ref
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_global_cross_namespace() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /config.php
<?php
namespace Config;

const DB_H$0OST = 'localhost';
//    ^^^^^^^ def

//- /database.php
<?php
namespace App\Database;

function connect() {
    $host = \Config\DB_HOST;
    //              ^^^^^^^ ref
}

class Connection {
    private string $host = \Config\DB_HOST;
    //                             ^^^^^^^ ref
}
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_in_default_parameter() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Limits {
    const DEF$0AULT_SIZE = 100;
    //    ^^^^^^^^^^^^ def
}

function process(int $size = Limits::DEFAULT_SIZE): void {
//                                   ^^^^^^^^^^^^ ref
    echo $size;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_in_array_initializer() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class HttpCode {
    const O$0K = 200;
    //    ^^ def
    const NOT_FOUND = 404;
}

$responses = [
    HttpCode::OK => 'success',
    //        ^^ ref
    HttpCode::NOT_FOUND => 'not found',
];

function status(): int {
    return HttpCode::OK;
    //               ^^ ref
}
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_multiple_same_name_different_class() {
    // Same constant name in different classes must not interfere
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class DatabaseConfig {
    const PO$0RT = 5432;
    //    ^^^^ def
}

class CacheConfig {
    const PORT = 6379;
}

$db_port = DatabaseConfig::PORT;
//                         ^^^^ ref

$cache_port = CacheConfig::PORT;
// Should not match DatabaseConfig::PORT
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_interface_usage() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
interface HttpMethods {
    const GET$0_TIMEOUT = 30;
    //    ^^^^^^^^^^^ def
    const POST_TIMEOUT = 60;
}

class Client implements HttpMethods {
    public function request(): void {
        $timeout = self::GET_TIMEOUT;
        //               ^^^^^^^^^^^ ref
    }
}

echo HttpMethods::GET_TIMEOUT;
//                ^^^^^^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_class_from_access_site() {
    // Cursor on a class constant ACCESS site must produce the same results as
    // cursor on the declaration — declaration + all usages.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Flags {
    const VERBOSE = 1;
    //    ^^^^^^^ def
    const DEBUG = 2;
}

$mode = Flags::VER$0BOSE;
//             ^^^^^^^ ref
if ($mode & Flags::VERBOSE) {}
//                 ^^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn constant_self_from_access_site() {
    // Cursor on `self::CONST` usage inside the same class must find declaration
    // and all usages (both `self::` and `ClassName::` forms).
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Config {
    const MAX = 100;
    //    ^^^ def

    public function check(int $v): bool {
        return $v <= self::MA$0X;
        //                 ^^^ ref
    }
}
$ok = Config::MAX;
//            ^^^ ref
"#,
    )
    .await;
}

/// Cursor on `parent::CONST` usage (not the declaration) must find the
/// declaration and all usages (`self::`, `parent::`, `ClassName::` forms) —
/// the `parent::` counterpart to `constant_self_from_access_site`, exercising
/// `resolve_reference_symbol`'s `class_before_double_colon` owner resolution
/// for the `"parent"` case the same way it already does for `self`/`static`.
#[tokio::test]
async fn constant_parent_from_access_site() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Base {
    const MAX = 100;
    //    ^^^ def

    public function limit(): int {
        return self::MAX;
        //           ^^^ ref
    }
}
class Derived extends Base {
    public function check(int $v): bool {
        return $v <= parent::MA$0X;
        //                   ^^^ ref
    }
}
$ok = Base::MAX;
//          ^^^ ref
"#,
    )
    .await;
}

/// `parent::CONST` is compile-time resolved to the *immediate* `extends`
/// class — in a three-level hierarchy, `Child`'s `parent::LABEL` must
/// resolve to `Middle`'s declaration (`Middle` is the class actually named
/// in `Child`'s `extends` clause), not walk further up to `Base`.
#[tokio::test]
async fn constant_parent_multilevel_inheritance_resolves_to_immediate_parent() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Base {
    public function noop(): void {}
}
class Middle extends Base {
    const LABEL = 'middle';
    //    ^^^^^ def

    public function readViaSelf(): string {
        return self::LABEL;
        //           ^^^^^ ref
    }
}
class Child extends Middle {
    public function readViaParent(): string {
        return parent::LA$0BEL;
        //             ^^^^^ ref
    }
}
$m = Middle::LABEL;
//           ^^^^^ ref
"#,
    )
    .await;
}

/// Rename shares `resolve_reference_symbol` with the references handler —
/// renaming from a `parent::CONST` cursor must edit the declaration and
/// every usage form, proving the owner-resolution fix also benefits rename.
#[tokio::test]
async fn rename_constant_from_parent_access_site() {
    let mut s = TestServer::new().await;
    s.check_rename_annotated(
        r#"<?php
class Base {
    const MAX = 100;
    //    ^^^ rename

    public function limit(): int {
        return self::MAX;
        //           ^^^ rename
    }
}
class Derived extends Base {
    public function check(int $v): bool {
        return $v <= parent::MA$0X;
        //                   ^^^ rename
    }
}
$ok = Base::MAX;
//          ^^^ rename
"#,
        "LIMIT",
    )
    .await;
}
