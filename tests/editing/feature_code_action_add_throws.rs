//! "Add missing @throws tags" code action.
//!
//! Triggered when a function/method already has a docblock and its body
//! contains `throw new ClassName()` expressions not yet covered by `@throws`.

use super::*;
use expect_test::expect;

// ── Availability ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn add_throws_offered_for_undocumented_throw() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Repo {
    /**
     * @param int $id
     */
    public function $0find$0(int $id): User
    {
        if ($id <= 0) {
            throw new InvalidArgumentException("bad id");
        }
        return new User();
    }
}
"#,
        )
        .await;
    assert!(
        out.contains("Add @throws InvalidArgumentException to PHPDoc"),
        "expected action in: {out}"
    );
}

/// `throw new self()` must resolve to the enclosing class's real name, not
/// offer the nonsensical "Add @throws self" — and must dedupe correctly
/// against an existing `@throws MyException` tag that already documents it.
#[tokio::test]
async fn add_throws_self_resolves_to_enclosing_class_and_dedupes() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class MyException extends \Exception {
    /**
     * @throws MyException
     */
    public static function $0throwSelf$0(): void
    {
        throw new self();
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Add @throws"),
        "throw new self() should resolve to MyException, already documented: {out}"
    );
}

/// `throw new static()` resolves the same way as `self`.
#[tokio::test]
async fn add_throws_static_resolves_to_enclosing_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class MyException extends \Exception {
    /** doc */
    public static function $0throwStatic$0(): void
    {
        throw new static();
    }
}
"#,
        )
        .await;
    assert!(
        out.contains("Add @throws MyException to PHPDoc"),
        "expected 'MyException', not the literal 'static', got: {out}"
    );
}

/// `throw new parent()` resolves to the class's `extends` name.
#[tokio::test]
async fn add_throws_parent_resolves_to_extends_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class BaseException extends \Exception {}
class SpecificException extends BaseException {
    /** doc */
    public function rethrow$0(): void
    {
        throw new parent();
    }
}
"#,
        )
        .await;
    assert!(
        out.contains("Add @throws BaseException to PHPDoc"),
        "expected 'BaseException', not the literal 'parent', got: {out}"
    );
}

#[tokio::test]
async fn add_throws_offered_for_multiple_missing() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Repo {
    /** @return User */
    public function $0load$0(int $id): User
    {
        if ($id <= 0) {
            throw new InvalidArgumentException("bad id");
        }
        throw new RuntimeException("not found");
    }
}
"#,
        )
        .await;
    assert!(
        out.contains("Add missing @throws tags to PHPDoc"),
        "expected action in: {out}"
    );
}

#[tokio::test]
async fn add_throws_not_offered_when_all_documented() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Repo {
    /**
     * @throws RuntimeException
     */
    public function $0run$0(): void
    {
        throw new RuntimeException("error");
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Add @throws") && !out.contains("Add missing @throws"),
        "should not offer when all throws are already documented, got: {out}"
    );
}

#[tokio::test]
async fn add_throws_not_offered_when_no_docblock() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Repo {
    public function $0run$0(): void
    {
        throw new RuntimeException("error");
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Add @throws"),
        "should not offer when there is no docblock, got: {out}"
    );
}

#[tokio::test]
async fn add_throws_not_offered_for_inherit_doc() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Child extends Base {
    /** {@inheritDoc} */
    public function $0run$0(): void
    {
        throw new RuntimeException("error");
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Add @throws"),
        "should not offer for @inheritDoc, got: {out}"
    );
}

#[tokio::test]
async fn add_throws_not_offered_for_dynamic_throw() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Repo {
    /** @return void */
    public function $0run$0(): void
    {
        $e = new RuntimeException("error");
        throw $e;
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Add @throws"),
        "should not offer for non-new throw expressions, got: {out}"
    );
}

#[tokio::test]
async fn add_throws_not_offered_when_no_throws_in_body() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Repo {
    /** @return string */
    public function $0greet$0(): string
    {
        return "hello";
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Add @throws"),
        "should not offer when method has no throw statements, got: {out}"
    );
}

// ── Applied edits ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn add_throws_inserts_single_tag() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Repo {
    /**
     * @param int $id
     * @return User
     */
    public function $0find$0(int $id): User
    {
        if ($id <= 0) {
            throw new InvalidArgumentException("bad id");
        }
        return new User();
    }
}
"#,
            "Add @throws InvalidArgumentException to PHPDoc",
        )
        .await;
    expect![[r#"
        <?php
        class Repo {
            /**
             * @param int $id
             * @return User
             * @throws InvalidArgumentException
             */
            public function find(int $id): User
            {
                if ($id <= 0) {
                    throw new InvalidArgumentException("bad id");
                }
                return new User();
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn add_throws_inserts_multiple_tags_sorted() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Repo {
    /** @return User */
    public function $0load$0(int $id): User
    {
        if ($id <= 0) {
            throw new InvalidArgumentException("bad id");
        }
        throw new RuntimeException("not found");
    }
}
"#,
            "Add missing @throws tags to PHPDoc",
        )
        .await;
    expect![[r#"
        <?php
        class Repo {
            /** @return User 
             * @throws InvalidArgumentException
             * @throws RuntimeException
             */
            public function load(int $id): User
            {
                if ($id <= 0) {
                    throw new InvalidArgumentException("bad id");
                }
                throw new RuntimeException("not found");
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn add_throws_skips_already_documented() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Repo {
    /**
     * @throws RuntimeException already documented
     */
    public function $0run$0(): void
    {
        throw new RuntimeException("error");
        throw new InvalidArgumentException("bad");
    }
}
"#,
            "Add @throws InvalidArgumentException to PHPDoc",
        )
        .await;
    expect![[r#"
        <?php
        class Repo {
            /**
             * @throws RuntimeException already documented
             * @throws InvalidArgumentException
             */
            public function run(): void
            {
                throw new RuntimeException("error");
                throw new InvalidArgumentException("bad");
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn add_throws_works_for_standalone_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
/**
 * @param string $path
 */
function $0read_file$0(string $path): string
{
    if (!file_exists($path)) {
        throw new RuntimeException("missing");
    }
    return file_get_contents($path);
}
"#,
            "Add @throws RuntimeException to PHPDoc",
        )
        .await;
    expect![[r#"
        <?php
        /**
         * @param string $path
         * @throws RuntimeException
         */
        function read_file(string $path): string
        {
            if (!file_exists($path)) {
                throw new RuntimeException("missing");
            }
            return file_get_contents($path);
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn add_throws_captures_nested_throw() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Svc {
    /** @return void */
    public function $0process$0(array $data): void
    {
        foreach ($data as $item) {
            if ($item === null) {
                throw new DomainException("null item");
            }
        }
    }
}
"#,
            "Add @throws DomainException to PHPDoc",
        )
        .await;
    expect![[r#"
        <?php
        class Svc {
            /** @return void 
             * @throws DomainException
             */
            public function process(array $data): void
            {
                foreach ($data as $item) {
                    if ($item === null) {
                        throw new DomainException("null item");
                    }
                }
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn add_throws_does_not_cross_closure_boundary() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Svc {
    /** @return void */
    public function $0run$0(): void
    {
        $fn = function () {
            throw new RuntimeException("inside closure");
        };
        $fn();
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Add @throws"),
        "should not cross closure boundary, got: {out}"
    );
}

/// A throw in an unrelated sibling class's method must not leak into the
/// cursor method's own @throws suggestion. PHP has no nested classes, so
/// "boundary crossing" here means class-to-class, not method nesting.
#[tokio::test]
async fn add_throws_does_not_leak_from_sibling_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Quiet {
    /** @return void */
    public function $0method$0(): void
    {
        // no throws here
    }
}
class Noisy {
    public function method(): void
    {
        throw new RuntimeException("noisy");
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Add @throws"),
        "should not cross class boundary, got: {out}"
    );
}

#[tokio::test]
async fn add_throws_php8_throw_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Validator {
    /** @return string */
    public function $0validate$0(?string $val): string
    {
        return $val ?? throw new InvalidArgumentException("required");
    }
}
"#,
            "Add @throws InvalidArgumentException to PHPDoc",
        )
        .await;
    expect![[r#"
        <?php
        class Validator {
            /** @return string 
             * @throws InvalidArgumentException
             */
            public function validate(?string $val): string
            {
                return $val ?? throw new InvalidArgumentException("required");
            }
        }
    "#]]
    .assert_eq(&out);
}
