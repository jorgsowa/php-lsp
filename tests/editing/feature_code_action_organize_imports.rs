//! Organize imports code action transformation tests.
//! Tests verify that imports are sorted and unused imports are removed.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn organize_imports_sorts_alphabetically() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use Zoo;
use Apple;
use Banana;

class Test {
    public function test() {
        new Apple();
        new Banana();
        new Zoo();$0
    }
}
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use Apple;
        use Banana;
        use Zoo;

        class Test {
            public function test() {
                new Apple();
                new Banana();
                new Zoo();
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_removes_unused() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use Apple;
use Banana;
use Cherry;

class Test {
    public function test() {
        new Apple();$0
    }
}
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use Apple;

        class Test {
            public function test() {
                new Apple();
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_removes_all_unused() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use Unused1;
use Unused2;
use Unused3;

$x = 1;$0
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php

        $x = 1;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_groups_by_kind() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use const MY_CONST;
use function strlen;
use DateTime;
use const ANOTHER_CONST;
use function count;
use stdClass;

$c1 = MY_CONST;
$c2 = ANOTHER_CONST;
$s = strlen("");
$co = count([]);
new DateTime();
new stdClass();$0
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use DateTime;
        use stdClass;

        use function count;
        use function strlen;

        use const ANOTHER_CONST;
        use const MY_CONST;

        $c1 = MY_CONST;
        $c2 = ANOTHER_CONST;
        $s = strlen("");
        $co = count([]);
        new DateTime();
        new stdClass();
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_handles_aliases() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use Zoo as Z;
use Apple as A;

class Test {
    public function test() {
        new A();
        new Z();$0
    }
}
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use Apple as A;
        use Zoo as Z;

        class Test {
            public function test() {
                new A();
                new Z();
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_deduplicates() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use \DateTime;
use \DateTime;
use \stdClass;
use \stdClass;

new DateTime();
new stdClass();$0
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use \DateTime;
        use \stdClass;

        new DateTime();
        new stdClass();
    "#]]
    .assert_eq(&out);
}

/// Two `use` statements for the same FQN under different aliases bind two
/// distinct names in scope — they must NOT be collapsed as duplicates, even
/// though a bare FQN-only dedup would treat them as the same import.
#[tokio::test]
async fn organize_imports_keeps_same_fqn_different_aliases() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use App\Models\User as UserModel;
use App\Models\User;
use App\Unused;

new User();
new UserModel();$0
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use App\Models\User as UserModel;
        use App\Models\User;

        new User();
        new UserModel();
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_case_insensitive_sort() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use Zebra;
use Apple;
use Banana;

new Zebra();
new Apple();
new Banana();$0
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use Apple;
        use Banana;
        use Zebra;

        new Zebra();
        new Apple();
        new Banana();
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_no_action_when_already_organized() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use Apple;
use Banana;

new Apple();
new Banana();$0
"#,
            "Organize imports",
        )
        .await;
    expect!["<action not found: Organize imports>"].assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_preserves_function_const_usage() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use function strlen;
use const PHP_VERSION;
use function implode;
use const PHP_OS;

$v = PHP_VERSION;
$o = PHP_OS;
$l1 = strlen("");
$l2 = implode(",", []);$0
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use function implode;
        use function strlen;

        use const PHP_OS;
        use const PHP_VERSION;

        $v = PHP_VERSION;
        $o = PHP_OS;
        $l1 = strlen("");
        $l2 = implode(",", []);
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_handles_uses_in_comments() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use DateTime;
use Unused;

/**
 * Working with DateTime class
 * The DateTime is a built-in class
 */
class Handler {
    public function create() {
        return new DateTime();
    }
}$0
"#,
            "Organize imports",
        )
        .await;
    // DateTime should be kept (used in code), Unused removed
    // Comment mentions of DateTime should not affect removal
    expect![[r#"
        <?php
        use DateTime;

        /**
         * Working with DateTime class
         * The DateTime is a built-in class
         */
        class Handler {
            public function create() {
                return new DateTime();
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_handles_uses_in_strings() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use Logger;
use Unused;

class Service {
    public function log() {
        $msg = "Using Logger class for logging";
        return new Logger();
    }
}$0
"#,
            "Organize imports",
        )
        .await;
    // Logger is used (new Logger() call), string mention shouldn't prevent removal of Unused
    expect![[r#"
        <?php
        use Logger;

        class Service {
            public function log() {
                $msg = "Using Logger class for logging";
                return new Logger();
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_no_action_when_aliased_import_is_used() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use App\Mailer as Mail;

$m = new Mail();$0
"#,
            "Organize imports",
        )
        .await;
    expect!["<action not found: Organize imports>"].assert_eq(&out);
}

#[tokio::test]
async fn organize_imports_function_only_group_has_no_blank_line() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use function Zlib\deflate;
use function App\format;

deflate(format('x'));$0
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use function App\format;
        use function Zlib\deflate;

        deflate(format('x'));
    "#]]
    .assert_eq(&out);
}

/// Regression test for a confirmed false positive: a PHP 7+ group-use
/// import (`use App\{Foo, Bar};`) was parsed as one opaque unit whose
/// "short name" was the literal string `"{Foo, Bar}"`, which never matches
/// real usages — so `is_used` always reported false and the whole group,
/// including members that ARE referenced, was deleted outright.
#[tokio::test]
async fn organize_imports_expands_group_use_members_used() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use App\{Foo, Bar};

$f = new Foo();
$b = new Bar();$0
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use App\Bar;
        use App\Foo;

        $f = new Foo();
        $b = new Bar();
    "#]]
    .assert_eq(&out);
}

/// Same group-use expansion, but only one member is actually referenced —
/// the unused member must be dropped while the used one is kept, proving
/// each member is checked independently rather than as an all-or-nothing unit.
#[tokio::test]
async fn organize_imports_expands_group_use_drops_unused_member() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use App\{Foo, Bar};

$f = new Foo();$0
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use App\Foo;

        $f = new Foo();
    "#]]
    .assert_eq(&out);
}

/// Group-use with an alias and a mixed `function` member — both the alias
/// binding and the per-member kind must be preserved through expansion.
#[tokio::test]
async fn organize_imports_expands_group_use_with_alias_and_function_member() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
use App\{Foo as F, function helper};

$f = new F();
helper();$0
"#,
            "Organize imports",
        )
        .await;
    expect![[r#"
        <?php
        use App\Foo as F;

        use function App\helper;

        $f = new F();
        helper();
    "#]]
    .assert_eq(&out);
}
