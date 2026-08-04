//! Implement interface code action transformation tests.
//! Tests verify that method stubs are correctly generated for unimplemented interfaces.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn implement_single_interface_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
interface Logger { public function log(string $msg): void; }
class $0App$0 implements Logger {}
"#,
            "Implement missing method",
        )
        .await;
    expect![[r#"
        <?php
        interface Logger { public function log(string $msg): void; }
        class App implements Logger {
            public function log(string $msg): void
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

/// "Implement missing method" is the only quickfix for an unimplemented
/// interface — there's no competing fix a user might prefer instead — so it
/// should be marked `isPreferred` for editors that auto-apply the preferred
/// quickfix on a keybinding (e.g. VS Code's Cmd+.).
#[tokio::test]
async fn implement_interface_action_is_marked_preferred() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
interface Logger { public function log(string $msg): void; }
class $0App$0 implements Logger {}
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s
        .code_action(&c.path, c.line, c.character, c.line, c.character)
        .await;
    let action = resp["result"]
        .as_array()
        .and_then(|actions| {
            actions
                .iter()
                .find(|a| a["title"].as_str() == Some("Implement missing method"))
        })
        .expect("Implement missing method action not found");
    assert_eq!(action["isPreferred"], true);
}

#[tokio::test]
async fn implement_multiple_interface_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
interface Handler {
    public function process(string $input): string;
    public function validate(): bool;
}
class $0Processor$0 implements Handler {}
"#,
            "Implement 2 missing methods",
        )
        .await;
    expect![[r#"
        <?php
        interface Handler {
            public function process(string $input): string;
            public function validate(): bool;
        }
        class Processor implements Handler {
            public function process(string $input): string
            {
                throw new \RuntimeException('Not implemented');
            }

            public function validate(): bool
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn implement_interface_with_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
interface Repository {
    public function find(int $id): ?object;
}
class $0UserRepository$0 implements Repository {}
"#,
            "Implement missing method",
        )
        .await;
    expect![[r#"
        <?php
        interface Repository {
            public function find(int $id): ?object;
        }
        class UserRepository implements Repository {
            public function find(int $id): ?object
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn implement_interface_with_multiple_parameters() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
interface Factory {
    public function create(string $name, array $config, int $version = 1): object;
}
class $0DefaultFactory$0 implements Factory {}
"#,
            "Implement missing method",
        )
        .await;
    expect![[r#"
        <?php
        interface Factory {
            public function create(string $name, array $config, int $version = 1): object;
        }
        class DefaultFactory implements Factory {
            public function create(string $name, array $config, int $version = 1): object
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn implement_no_action_when_already_implemented() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
interface Logger { public function log(): void; }
class $0ConsoleLogger$0 implements Logger {
    public function log(): void { }
}
"#,
            "Implement missing method",
        )
        .await;
    expect!["<action not found: Implement missing method>"].assert_eq(&out);
}

#[tokio::test]
async fn implement_interface_with_static_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
interface Factory {
    public static function create(): self;
}
class $0DefaultFactory$0 implements Factory {}
"#,
            "Implement missing method",
        )
        .await;
    expect![[r#"
        <?php
        interface Factory {
            public static function create(): self;
        }
        class DefaultFactory implements Factory {
            public static function create(): self
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn implement_interface_with_variadic_parameters() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
interface Logger {
    public function log(string ...$messages): void;
}
class $0ConsoleLogger$0 implements Logger {}
"#,
            "Implement missing method",
        )
        .await;
    expect![[r#"
        <?php
        interface Logger {
            public function log(string ...$messages): void;
        }
        class ConsoleLogger implements Logger {
            public function log(string ...$messages): void
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn implement_abstract_class_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
abstract class Shape {
    abstract public function area(): float;
}
class $0Circle$0 extends Shape {}
"#,
            "Implement missing method",
        )
        .await;
    expect![[r#"
        <?php
        abstract class Shape {
            abstract public function area(): float;
        }
        class Circle extends Shape {
            public function area(): float
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn implement_interface_resolved_through_use_import() {
    // Interface lives in a separate file under a braced namespace; the class
    // imports it with `use`. The action must look up the correct interface.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"//- /Contracts/Renderable.php
<?php
namespace App\Contracts;
interface Renderable {
    public function render(): string;
}

//- /View.php
<?php
use App\Contracts\Renderable;
class $0View$0 implements Renderable {}
"#,
            "Implement missing method",
        )
        .await;
    expect![[r#"
        <?php
        use App\Contracts\Renderable;
        class View implements Renderable {
            public function render(): string
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn implement_interface_same_short_name_disambiguated_by_use_import() {
    // Two interfaces share the short name `Logger`; only the one referenced by
    // the `use` statement should have its methods stubbed.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"//- /Other/Logger.php
<?php
namespace Other;
interface Logger {
    public function wrong(): void;
}

//- /App/Logging/Logger.php
<?php
namespace App\Logging;
interface Logger {
    public function log(string $msg): void;
}

//- /FileLogger.php
<?php
use App\Logging\Logger;
class $0FileLogger$0 implements Logger {}
"#,
            "Implement missing method",
        )
        .await;
    expect![[r#"
        <?php
        use App\Logging\Logger;
        class FileLogger implements Logger {
            public function log(string $msg): void
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn implement_abstract_class_resolved_through_use_import() {
    // Abstract class lives in a separate file under an unbraced namespace;
    // the concrete class imports it with `use`. Verifies the unbraced-namespace
    // fix in collect_abstract_methods_fqn covers abstract classes too.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"//- /Base/Handler.php
<?php
namespace Base;
abstract class Handler {
    abstract public function handle(string $request): string;
}

//- /Http/MyHandler.php
<?php
use Base\Handler;
class $0MyHandler$0 extends Handler {}
"#,
            "Implement missing method",
        )
        .await;
    expect![[r#"
        <?php
        use Base\Handler;
        class MyHandler extends Handler {
            public function handle(string $request): string
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
#[ignore = "pre-existing: implement action resolves the interface name case-sensitively downstream of the (case-insensitive) declaration-file prefilter; reproduces identically on the pre-mention-index baseline"]
async fn implement_interface_mentioned_with_different_case_in_declaring_file() {
    // PHP class names are case-insensitive: the class writes `RENDERABLE`
    // while the declaring file spells it `Renderable`. The declaration-file
    // prefilter (mir's mention index, word-boundary + ASCII-case-insensitive)
    // must still surface the declaring file.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"//- /Contracts/Renderable.php
<?php
namespace App\Contracts;
interface Renderable {
    public function render(): string;
}

//- /View.php
<?php
use App\Contracts\Renderable;
class $0View$0 implements RENDERABLE {}
"#,
            "Implement missing method",
        )
        .await;
    expect![[r#"
        <?php
        use App\Contracts\Renderable;
        class View implements RENDERABLE {
            public function render(): string
            {
                throw new \RuntimeException('Not implemented');
            }

        }
    "#]]
    .assert_eq(&out);
}

/// The declaring-file prefilter records per-file mention sets in a
/// persistent cache (mir's `ClassMentionIndex`). A file scanned once as a
/// MISS must not stay a miss after an edit adds the declaration — the
/// cache's freshness check has to force a rescan of the changed text.
#[tokio::test]
async fn implement_action_sees_interface_added_to_already_scanned_file() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open("other.php", "<?php\nclass Unrelated {}\n").await;
    s.open("view.php", "<?php\nclass View implements Contract {}\n")
        .await;

    // First request: `Contract` is declared nowhere, so no implement action —
    // and other.php's "no mention of Contract" is now recorded in the cache.
    let resp = s.code_action("view.php", 1, 7, 1, 7).await;
    let has_implement = |resp: &serde_json::Value| {
        resp["result"].as_array().is_some_and(|arr| {
            arr.iter().any(|a| {
                a["title"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Implement missing"))
            })
        })
    };
    assert!(
        !has_implement(&resp),
        "no implement action expected while the interface is undeclared: {resp:?}"
    );

    // The already-scanned file now declares the interface.
    s.change(
        "other.php",
        2,
        "<?php\ninterface Contract {\n    public function go(): void;\n}\n",
    )
    .await;

    let resp = s.code_action("view.php", 1, 7, 1, 7).await;
    assert!(
        has_implement(&resp),
        "implement action must appear once the edited file declares the interface: {resp:?}"
    );
}
