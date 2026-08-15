use super::common::{TestServer, render_diagnostics_notification};
use expect_test::expect;
use serde_json::json;

// ── Monorepo workspace scan ────────────────────────────────────────────────────

#[tokio::test]
async fn monorepo_workspace_scan_indexes_all_packages() {
    let mut s = TestServer::with_fixture("monorepo").await;
    s.wait_for_index_ready().await;

    let out = s.snapshot_workspace_symbols("").await;
    // Verify that symbols from all packages are indexed
    expect![[r#"
        Class       ListUsersCommand @ packages/cli/src/Command/ListUsersCommand.php:5
        Class       User @ packages/core/src/Entity/User.php:3
        Class       UserController @ packages/api/src/Controller/UserController.php:6
        Class       UserRepository @ packages/core/src/Repository/UserRepository.php:5
        Class       UserTest @ packages/tests/src/Integration/UserTest.php:6
        Method      __construct @ packages/api/src/Controller/UserController.php:7
        Method      __construct @ packages/cli/src/Command/ListUsersCommand.php:6
        Method      __construct @ packages/core/src/Entity/User.php:4
        Method      execute @ packages/cli/src/Command/ListUsersCommand.php:10
        Method      findAll @ packages/core/src/Repository/UserRepository.php:18
        Method      findById @ packages/core/src/Repository/UserRepository.php:9
        Method      getDisplayName @ packages/core/src/Entity/User.php:10
        Method      index @ packages/api/src/Controller/UserController.php:15
        Method      save @ packages/core/src/Repository/UserRepository.php:22
        Method      show @ packages/api/src/Controller/UserController.php:11
        Method      testUserRepository @ packages/tests/src/Integration/UserTest.php:7
        Property    $email @ packages/core/src/Entity/User.php:7
        Property    $id @ packages/core/src/Entity/User.php:5
        Property    $name @ packages/core/src/Entity/User.php:6
        Property    $repository @ packages/api/src/Controller/UserController.php:8
        Property    $repository @ packages/cli/src/Command/ListUsersCommand.php:7
        Property    $users @ packages/core/src/Repository/UserRepository.php:7"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn monorepo_workspace_symbols_scoped_by_package() {
    let mut s = TestServer::with_fixture("monorepo").await;
    s.wait_for_index_ready().await;

    let out = s.snapshot_workspace_symbols("UserController").await;
    expect!["Class       UserController @ packages/api/src/Controller/UserController.php:6"]
        .assert_eq(&out);
}

// ── Monorepo cross-package navigation ──────────────────────────────────────────

#[tokio::test]
async fn monorepo_cross_package_definition() {
    let mut s = TestServer::with_fixture("monorepo").await;
    s.wait_for_index_ready().await;

    let out = s
        .check_definition(
            r#"//- /packages/api/src/Controller/UserController.php
<?php
namespace Acme\Api\Controller;

use Acme\Core\Entity\User;
use Acme\Core\Repository\UserRepository;

class UserController {
    public function __construct(
        private UserRepository $repository,
    ) {}

    public function show(int $id): ?User {
        return $this->repository->findById($id);
    }

    public function index(): array {
        return $this->repository->find$0All();
    }
}
"#,
        )
        .await;

    expect!["packages/core/src/Repository/UserRepository.php:18:20-18:27"].assert_eq(&out);
}

#[tokio::test]
async fn monorepo_cross_package_references() {
    let mut s = TestServer::with_fixture("monorepo").await;
    s.wait_for_index_ready().await;

    let out = s
        .check_references(
            r#"//- /packages/core/src/Entity/User.php
<?php
namespace Acme\Core\Entity;

class User$0 {
    public function __construct(
        public readonly int $id,
        public readonly string $name,
        public readonly string $email,
    ) {}

    public function getDisplayName(): string {
        return $this->name;
    }
}
"#,
        )
        .await;

    expect![[r#"
        packages/api/src/Controller/UserController.php:11:36-11:40
        packages/api/src/Controller/UserController.php:3:4-3:25
        packages/core/src/Entity/User.php:3:6-3:10
        packages/core/src/Repository/UserRepository.php:22:25-22:29
        packages/core/src/Repository/UserRepository.php:3:4-3:25
        packages/core/src/Repository/UserRepository.php:9:40-9:44
        packages/tests/src/Integration/UserTest.php:3:4-3:25
        packages/tests/src/Integration/UserTest.php:9:20-9:24"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn monorepo_cross_package_hover() {
    let mut s = TestServer::with_fixture("monorepo").await;
    s.wait_for_index_ready().await;

    let out = s
        .check_hover(
            r#"//- /packages/api/src/Controller/UserController.php
<?php
namespace Acme\Api\Controller;

use Acme\Core\Entity\User;
use Acme\Core\Repository\UserRepository;

class UserController {
    public function __construct(
        private UserRepository$0 $repository,
    ) {}
}
"#,
        )
        .await;

    expect![[r#"
        ```php
        class UserRepository
        ```"#]]
    .assert_eq(&out);
}

// ── Monorepo diagnostics ───────────────────────────────────────────────────────

#[tokio::test]
async fn monorepo_cross_package_clean_file() {
    let mut s = TestServer::with_fixture("monorepo").await;
    s.wait_for_index_ready().await;

    s.check_diagnostics(
        r#"//- /packages/api/src/Controller/UserController.php
<?php
namespace Acme\Api\Controller;

use Acme\Core\Entity\User;
use Acme\Core\Repository\UserRepository;

class UserController {
    public function __construct(
        private UserRepository $repository,
    ) {}

    public function show(int $id): ?User {
        return $this->repository->findById($id);
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn monorepo_undefined_cross_package_class_diagnostic() {
    let mut s = TestServer::with_fixture("monorepo").await;
    s.wait_for_index_ready().await;

    s.check_diagnostics(
        r#"<?php
namespace Acme\Api\Controller;

use Acme\Core\Entity\NonexistentUser;

class GuestController {
    public function __construct(
        private NonexistentUser $user,
        //      ^^^^^^^^^^^^^^^ error: Class Acme\Core\Entity\NonexistentUser does not exist
    ) {}
}
"#,
    )
    .await;
}

// ── Multi-PSR4 workspace scan ──────────────────────────────────────────────────

#[tokio::test]
async fn multi_psr4_all_mappings_indexed() {
    let mut s = TestServer::with_fixture("multi-psr4").await;
    s.wait_for_index_ready().await;

    let out = s.snapshot_workspace_symbols("").await;
    expect![[r#"
        Class       Mailer @ src/Service/Mailer.php:5
        Class       MailerTest @ tests/Unit/MailerTest.php:6
        Class       SmtpClient @ lib/Transport/SmtpClient.php:3
        Method      __construct @ src/Service/Mailer.php:6
        Method      deliver @ lib/Transport/SmtpClient.php:4
        Method      send @ src/Service/Mailer.php:10
        Method      testSend @ tests/Unit/MailerTest.php:7
        Property    $client @ src/Service/Mailer.php:7"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn multi_psr4_cross_mapping_definition() {
    let mut s = TestServer::with_fixture("multi-psr4").await;
    s.wait_for_index_ready().await;

    let out = s
        .check_definition(
            r#"//- /src/Service/Mailer.php
<?php
namespace App\Service;

use Lib\Transport\SmtpClient;

class Mailer {
    public function __construct(
        private SmtpClient $client,
    ) {}

    public function send(string $to, string $subject, string $body): bool {
        return $this->client->deliver$0($to, $subject, $body);
    }
}
"#,
        )
        .await;

    expect!["lib/Transport/SmtpClient.php:4:20-4:27"].assert_eq(&out);
}

#[tokio::test]
async fn multi_psr4_cross_mapping_clean_diagnostics() {
    let mut s = TestServer::with_fixture("multi-psr4").await;
    s.wait_for_index_ready().await;

    s.check_diagnostics(
        r#"//- /src/Service/Mailer.php
<?php
namespace App\Service;

use Lib\Transport\SmtpClient;

class Mailer {
    public function __construct(
        private SmtpClient $client,
    ) {}

    public function send(string $to, string $subject, string $body): bool {
        return $this->client->deliver($to, $subject, $body);
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn multi_psr4_autoload_dev_scanned() {
    let mut s = TestServer::with_fixture("multi-psr4").await;
    s.wait_for_index_ready().await;

    let out = s.snapshot_workspace_symbols("MailerTest").await;
    expect!["Class       MailerTest @ tests/Unit/MailerTest.php:6"].assert_eq(&out);
}

// ── Single prefix mapped to an array of base dirs ──────────────────────────────
//
// Regression coverage for the composer PSR-4 array form, e.g.
//   { "App\\": ["src/", "lib/"] }
//
// All other multi_psr4_* tests use distinct prefixes per dir; this fixture
// shares one prefix across two dirs so resolution must search every base in
// the array before giving up.

#[tokio::test]
async fn multi_psr4_array_all_bases_indexed() {
    let mut s = TestServer::with_fixture("multi-psr4-array").await;
    s.wait_for_index_ready().await;

    let out = s.snapshot_workspace_symbols("").await;
    expect![[r#"
        Class       Alpha @ src/Alpha.php:3
        Class       Beta @ lib/Beta.php:3
        Method      __construct @ lib/Beta.php:4
        Method      describe @ src/Alpha.php:4
        Method      run @ lib/Beta.php:8
        Property    $alpha @ lib/Beta.php:5"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn multi_psr4_array_cross_base_definition() {
    let mut s = TestServer::with_fixture("multi-psr4-array").await;
    s.wait_for_index_ready().await;

    let out = s
        .check_definition(
            r#"//- /lib/Beta.php
<?php
namespace App;

class Beta {
    public function __construct(
        private Alpha$0 $alpha,
    ) {}
}
"#,
        )
        .await;

    expect!["src/Alpha.php:3:6-3:11"].assert_eq(&out);
}

// Regression for same-namespace cross-file refs: `Beta` lives in `lib/` and
// references `Alpha` (in `src/`) without a `use` statement, since both share
// the `App` namespace. The pre-load path must resolve same-namespace bare
// class references, not just `use`-imported ones.
#[tokio::test]
async fn multi_psr4_array_cross_base_clean_diagnostics() {
    let mut s = TestServer::with_fixture("multi-psr4-array").await;
    s.wait_for_index_ready().await;

    s.check_diagnostics(
        r#"//- /lib/Beta.php
<?php
namespace App;

class Beta {
    public function __construct(
        private Alpha $alpha,
    ) {}

    public function run(): string {
        return $this->alpha->describe();
    }
}
"#,
    )
    .await;
}

// ── PHP version with project structure ─────────────────────────────────────────

#[tokio::test]
async fn monorepo_php80_str_contains_no_error() {
    let mut s = TestServer::with_fixture_and_options(
        "monorepo",
        json!({
            "phpVersion": "8.0",
            "diagnostics": { "enabled": true }
        }),
    )
    .await;
    s.wait_for_index_ready().await;

    s.check_diagnostics(
        r#"<?php
namespace Acme\Core\Util;

class StringHelper {
    public function hasSubstring(string $haystack, string $needle): bool {
        return str_contains($haystack, $needle);
    }
}
"#,
    )
    .await;
}

// PHP version filtering across a monorepo fixture: depends on the salsa db being
// seeded with the configured version (fixed in mir-analyzer 0.31.0).
#[tokio::test]
async fn monorepo_php74_str_contains_error() {
    let mut s = TestServer::with_fixture_and_options(
        "monorepo",
        json!({
            "phpVersion": "7.4",
            "diagnostics": { "enabled": true }
        }),
    )
    .await;
    s.wait_for_index_ready().await;

    let notif = s
        .open(
            "test.php",
            "<?php\nnamespace Acme\\Core\\Util;\nclass StringHelper {\n    public function hasSubstring(string $haystack, string $needle): bool {\n        return str_contains($haystack, $needle);\n    }\n}\n",
        )
        .await;
    expect!["4:15-4:47 [1] UndefinedFunction: Function str_contains() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn multi_psr4_php81_array_is_list_available() {
    let mut s = TestServer::with_fixture_and_options(
        "multi-psr4",
        json!({
            "phpVersion": "8.1",
            "diagnostics": { "enabled": true }
        }),
    )
    .await;
    s.wait_for_index_ready().await;

    // array_is_list must resolve under PHP 8.1 (no UnknownFunction error).
    s.check_no_diagnostics(
        r#"<?php
namespace Lib\Util;

class ArrayHelper {
    public function isList(mixed $value): bool {
        if (array_is_list($value)) {
            return true;
        }
        return false;
    }
}
"#,
    )
    .await;
}

// ── Exclude paths interaction ──────────────────────────────────────────────────

#[tokio::test]
async fn monorepo_exclude_one_package() {
    let mut s = TestServer::with_fixture_and_options(
        "monorepo",
        json!({
            "excludePaths": ["packages/cli/"]
        }),
    )
    .await;
    s.wait_for_index_ready().await;

    let out = s.snapshot_workspace_symbols("").await;
    // ListUsersCommand (from packages/cli/) must be absent
    expect![[r#"
        Class       User @ packages/core/src/Entity/User.php:3
        Class       UserController @ packages/api/src/Controller/UserController.php:6
        Class       UserRepository @ packages/core/src/Repository/UserRepository.php:5
        Class       UserTest @ packages/tests/src/Integration/UserTest.php:6
        Method      __construct @ packages/api/src/Controller/UserController.php:7
        Method      __construct @ packages/core/src/Entity/User.php:4
        Method      findAll @ packages/core/src/Repository/UserRepository.php:18
        Method      findById @ packages/core/src/Repository/UserRepository.php:9
        Method      getDisplayName @ packages/core/src/Entity/User.php:10
        Method      index @ packages/api/src/Controller/UserController.php:15
        Method      save @ packages/core/src/Repository/UserRepository.php:22
        Method      show @ packages/api/src/Controller/UserController.php:11
        Method      testUserRepository @ packages/tests/src/Integration/UserTest.php:7
        Property    $email @ packages/core/src/Entity/User.php:7
        Property    $id @ packages/core/src/Entity/User.php:5
        Property    $name @ packages/core/src/Entity/User.php:6
        Property    $repository @ packages/api/src/Controller/UserController.php:8
        Property    $users @ packages/core/src/Repository/UserRepository.php:7"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn multi_psr4_exclude_tests_dir() {
    let mut s = TestServer::with_fixture_and_options(
        "multi-psr4",
        json!({
            "excludePaths": ["tests/"]
        }),
    )
    .await;
    s.wait_for_index_ready().await;

    let out = s.snapshot_workspace_symbols("").await;
    // MailerTest (from tests/) must be absent
    expect![[r#"
        Class       Mailer @ src/Service/Mailer.php:5
        Class       SmtpClient @ lib/Transport/SmtpClient.php:3
        Method      __construct @ src/Service/Mailer.php:6
        Method      deliver @ lib/Transport/SmtpClient.php:4
        Method      send @ src/Service/Mailer.php:10
        Property    $client @ src/Service/Mailer.php:7"#]]
    .assert_eq(&out);
}

// ── Workspace structure edge cases ────────────────────────────────────────────

#[tokio::test]
async fn monorepo_multiple_files_same_namespace_different_packages() {
    let mut s = TestServer::with_fixture("monorepo").await;
    s.wait_for_index_ready().await;

    let out = s.snapshot_workspace_symbols("User").await;
    expect![[r#"
        Class       ListUsersCommand @ packages/cli/src/Command/ListUsersCommand.php:5
        Class       User @ packages/core/src/Entity/User.php:3
        Class       UserController @ packages/api/src/Controller/UserController.php:6
        Class       UserRepository @ packages/core/src/Repository/UserRepository.php:5
        Class       UserTest @ packages/tests/src/Integration/UserTest.php:6
        Method      testUserRepository @ packages/tests/src/Integration/UserTest.php:7
        Property    $users @ packages/core/src/Repository/UserRepository.php:7"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn monorepo_with_fixture_and_options_and_version() {
    let mut s = TestServer::with_fixture_and_options(
        "monorepo",
        json!({
            "phpVersion": "8.4",
            "diagnostics": { "enabled": true }
        }),
    )
    .await;
    s.wait_for_index_ready().await;

    s.check_diagnostics(
        r#"<?php
namespace Acme\Core\Util;

class ArrayFunctions {
    public function findElement(array $arr, callable $fn): mixed {
        return array_find($arr, $fn);
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn multi_psr4_cross_package_in_same_namespace_call() {
    let mut s = TestServer::with_fixture("multi-psr4").await;
    s.wait_for_index_ready().await;

    s.check_diagnostics(
        r#"<?php
namespace Tests\Unit;

use App\Service\Mailer;
use Lib\Transport\SmtpClient;

class MailerTest {
    public function testSend(): void {
        $client = new SmtpClient();
        $mailer = new Mailer($client);
        $result = $mailer->send('test@example.com', 'Subject', 'Body');
    }
}
"#,
    )
    .await;
}
