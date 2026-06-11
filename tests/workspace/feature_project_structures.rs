use super::common::TestServer;
use expect_test::expect;
use serde_json::json;

// ── Helper functions for robust snapshot assertions ────────────────────────────

/// Assert that a symbol exists in the snapshot output, ignoring line numbers.
/// Example: assert_symbol_exists(&out, "UserController", "packages/api/src/Controller/UserController.php")
fn assert_symbol_exists(output: &str, symbol_name: &str, file_path: &str) {
    let pattern = format!("{} @", symbol_name);
    assert!(
        output.contains(&pattern),
        "Expected to find symbol '{}' in output:\n{}",
        symbol_name,
        output
    );
    assert!(
        output.contains(file_path),
        "Expected to find file path '{}' for symbol '{}' in output:\n{}",
        file_path,
        symbol_name,
        output
    );
}

/// Assert that a symbol does NOT exist in the snapshot output.
fn assert_symbol_not_exists(output: &str, symbol_name: &str) {
    let pattern = format!("{} @", symbol_name);
    assert!(
        !output.contains(&pattern),
        "Expected NOT to find symbol '{}' in output:\n{}",
        symbol_name,
        output
    );
}

/// Assert that multiple symbols exist in the output.
fn assert_all_symbols_exist(output: &str, symbols: &[(&str, &str)]) {
    for (symbol, file_path) in symbols {
        assert_symbol_exists(output, symbol, file_path);
    }
}

/// Assert that specific symbols are present and others are absent.
fn assert_workspace_symbols(output: &str, present: &[(&str, &str)], absent: &[&str]) {
    assert_all_symbols_exist(output, present);
    for symbol in absent {
        assert_symbol_not_exists(output, symbol);
    }
}

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
        Method      testUserRepository @ packages/tests/src/Integration/UserTest.php:7"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn monorepo_workspace_symbols_scoped_by_package() {
    let mut s = TestServer::with_fixture("monorepo").await;
    s.wait_for_index_ready().await;

    let out = s.snapshot_workspace_symbols("UserController").await;
    assert_symbol_exists(
        &out,
        "UserController",
        "packages/api/src/Controller/UserController.php",
    );
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
        packages/core/src/Entity/User.php:3:6-3:10
        packages/core/src/Repository/UserRepository.php:22:25-22:29
        packages/core/src/Repository/UserRepository.php:9:40-9:44
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

class UserController {
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
    // Verify that symbols from all PSR-4 mappings are indexed
    assert_all_symbols_exist(
        &out,
        &[
            ("Mailer", "src/Service/Mailer.php"),
            ("MailerTest", "tests/Unit/MailerTest.php"),
            ("SmtpClient", "lib/Transport/SmtpClient.php"),
        ],
    );
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
    assert_all_symbols_exist(
        &out,
        &[("Alpha", "src/Alpha.php"), ("Beta", "lib/Beta.php")],
    );
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
    let empty = vec![];
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(
        diags
            .iter()
            .any(|d| d["message"].as_str().unwrap_or("").contains("str_contains")),
        "Expected str_contains undefined error with PHP 7.4"
    );
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
    // Verify that Core, Api, and Tests are indexed but Cli is excluded
    assert_workspace_symbols(
        &out,
        &[
            ("User", "packages/core/src/Entity/User.php"),
            (
                "UserController",
                "packages/api/src/Controller/UserController.php",
            ),
            (
                "UserRepository",
                "packages/core/src/Repository/UserRepository.php",
            ),
            ("UserTest", "packages/tests/src/Integration/UserTest.php"),
        ],
        &["ListUsersCommand"],
    );
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
    // Verify that App and Lib are indexed but Tests is excluded
    assert_workspace_symbols(
        &out,
        &[
            ("Mailer", "src/Service/Mailer.php"),
            ("SmtpClient", "lib/Transport/SmtpClient.php"),
        ],
        &["MailerTest"],
    );
}

// ── Workspace structure edge cases ────────────────────────────────────────────

#[tokio::test]
async fn monorepo_multiple_files_same_namespace_different_packages() {
    let mut s = TestServer::with_fixture("monorepo").await;
    s.wait_for_index_ready().await;

    let out = s.snapshot_workspace_symbols("User").await;
    // Verify that all User-related symbols are found across packages
    assert_all_symbols_exist(
        &out,
        &[
            ("User", "packages/core/src/Entity/User.php"),
            (
                "UserController",
                "packages/api/src/Controller/UserController.php",
            ),
            (
                "UserRepository",
                "packages/core/src/Repository/UserRepository.php",
            ),
            ("UserTest", "packages/tests/src/Integration/UserTest.php"),
        ],
    );
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
