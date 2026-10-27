//! Inline variable code action transformation tests.
//! Tests verify that variables are replaced with their assigned expressions.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn inline_variable_simple_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$greeting = "Hello";
echo $0$greeting$0;
"#,
            "Inline variable '$greeting'",
        )
        .await;
    expect![[r#"
        <?php
        echo "Hello";
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_numeric_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$count = 42;
$total = $0$count$0 * 2;
"#,
            "Inline variable '$count'",
        )
        .await;
    expect![[r#"
        <?php
        $total = 42 * 2;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_removes_assignment() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = 5;
echo $0$x$0;
echo $x;
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        echo 5;
        echo 5;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_function_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$name = getName();
echo "Hello " . $0$name$0;
"#,
            "Inline variable '$name'",
        )
        .await;
    expect![[r#"
        <?php
        echo "Hello " . getName();
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_array_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$items = [1, 2, 3];
foreach ($0$items$0 as $item) {
    echo $item;
}
"#,
            "Inline variable '$items'",
        )
        .await;
    expect![[r#"
        <?php
        foreach ([1, 2, 3] as $item) {
            echo $item;
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_with_complex_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$result = $this->getValue() + 10 * 2;
echo $0$result$0;
"#,
            "Inline variable '$result'",
        )
        .await;
    expect![[r#"
        <?php
        echo $this->getValue() + 10 * 2;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_multiple_uses_same_line() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = 5;
echo $0$x$0 + $x;
"#,
            "Inline variable '$x'",
        )
        .await;
    // Both uses should be replaced
    expect![[r#"
        <?php
        echo 5 + 5;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_no_action_when_multiple_assignments() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$tmp = 1;
$tmp = 2;
return $0$tmp$0;
"#,
            "Inline variable '$tmp'",
        )
        .await;
    expect!["<action not found: Inline variable '$tmp'>"].assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_no_action_when_later_assignment_exists() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$tmp = 1;
echo $0$tmp$0;
$tmp = 2;
"#,
            "Inline variable '$tmp'",
        )
        .await;
    expect!["<action not found: Inline variable '$tmp'>"].assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_equality_does_not_block_inline() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = 1;
if ($x == 2) {
    echo $0$x$0;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        if (1 == 2) {
            echo 1;
        }
    "#]]
    .assert_eq(&out);
}

/// A second statement on the assignment's line survives the deletion.
#[tokio::test]
async fn inline_variable_keeps_second_statement_on_same_line() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = 5; doSomethingElse();
echo $0$x$0;
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        doSomethingElse();
        echo 5;
    "#]]
    .assert_eq(&out);
}

/// A semicolon inside a nested call's string argument must not be mistaken
/// for the statement terminator when extracting the RHS.
#[tokio::test]
async fn inline_variable_rhs_with_semicolon_in_string_argument() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = foo("a;b");
echo $0$x$0;
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        echo foo("a;b");
    "#]]
    .assert_eq(&out);
}

/// A same-named variable or parameter in another function is a different variable.
#[tokio::test]
async fn inline_variable_stays_in_its_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function a() {
    $x = compute();
    return $0$x$0;
}
function b($x) {
    return $x;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        function a() {
            return compute();
        }
        function b($x) {
            return $x;
        }
    "#]]
    .assert_eq(&out);
}

/// A lower-precedence initializer keeps its grouping where an operator follows.
#[tokio::test]
async fn inline_variable_parenthesizes_binary_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($a, $b) {
    $x = $a + $b;
    return $0$x$0 * 2;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        function f($a, $b) {
            return ($a + $b) * 2;
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_no_action_when_read_in_string() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$name = getName();
echo $0$name$0;
echo "Hi $name";
"#,
            "Inline variable '$name'",
        )
        .await;
    expect!["<action not found: Inline variable '$name'>"].assert_eq(&out);
}

/// Inlining a conditional assignment past its `if` would run it unconditionally.
#[tokio::test]
async fn inline_variable_no_action_when_assigned_in_inner_block() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($c) {
    if ($c) {
        $x = 1;
    }
    return $0$x$0;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect!["<action not found: Inline variable '$x'>"].assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_multiline_initializer() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f() {
    $x = [
        'a' => 1,
    ];
    return $0$x$0;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        function f() {
            return [
                'a' => 1,
            ];
        }
    "#]]
    .assert_eq(&out);
}

/// Inlining `$tmp` past `$a = $b` would read the new `$a` and break the swap.
#[tokio::test]
async fn inline_variable_no_action_when_initializer_operand_reassigned() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function swap($a, $b) {
    $tmp = $a;
    $a = $b;
    $b = $0$tmp$0;
    return [$a, $b];
}
"#,
            "Inline variable '$tmp'",
        )
        .await;
    expect!["<action not found: Inline variable '$tmp'>"].assert_eq(&out);
}

/// A by-reference argument must stay a variable.
#[tokio::test]
async fn inline_variable_no_action_when_passed_by_reference() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($s) {
    $m = null;
    preg_match('/x/', $s, $0$m$0);
    return $m;
}
"#,
            "Inline variable '$m'",
        )
        .await;
    expect!["<action not found: Inline variable '$m'>"].assert_eq(&out);
}

/// The same holds for a by-reference parameter of a user function.
#[tokio::test]
async fn inline_variable_no_action_when_passed_to_user_by_ref_param() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function fill(array &$out) { $out[] = 1; }
function f() {
    $items = [];
    fill($0$items$0);
}
"#,
            "Inline variable '$items'",
        )
        .await;
    expect!["<action not found: Inline variable '$items'>"].assert_eq(&out);
}

/// A by-value argument still inlines.
#[tokio::test]
async fn inline_variable_into_by_value_argument() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($s) {
    $p = '/x/';
    return preg_match($0$p$0, $s);
}
"#,
            "Inline variable '$p'",
        )
        .await;
    expect![[r#"
        <?php
        function f($s) {
            return preg_match('/x/', $s);
        }
    "#]]
    .assert_eq(&out);
}

/// `$this->h(1)` would call a method, not the closure held in the property.
#[tokio::test]
async fn inline_variable_parenthesizes_called_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class A {
    private $h;
    public function run() {
        $cb = $this->h;
        return $0$cb$0(1);
    }
}
"#,
            "Inline variable '$cb'",
        )
        .await;
    expect![[r#"
        <?php
        class A {
            private $h;
            public function run() {
                return ($this->h)(1);
            }
        }
    "#]]
    .assert_eq(&out);
}

/// A variable callee needs no parentheses.
#[tokio::test]
async fn inline_variable_called_variable_stays_bare() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($fn) {
    $cb = $fn;
    return $0$cb$0(1);
}
"#,
            "Inline variable '$cb'",
        )
        .await;
    expect![[r#"
        <?php
        function f($fn) {
            return $fn(1);
        }
    "#]]
    .assert_eq(&out);
}

/// `new Foo::class` doesn't parse; the class name goes in instead.
#[tokio::test]
async fn inline_variable_class_string_into_new() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Foo {}
function f($o) {
    $cls = Foo::class;
    $o instanceof $cls;
    return new $0$cls$0;
}
"#,
            "Inline variable '$cls'",
        )
        .await;
    expect![[r#"
        <?php
        class Foo {}
        function f($o) {
            $o instanceof Foo;
            return new Foo;
        }
    "#]]
    .assert_eq(&out);
}

/// Only a variable or `Foo::class` can stand after `new`.
#[tokio::test]
async fn inline_variable_no_action_when_string_named_class_is_instantiated() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f() {
    $cls = 'Foo';
    return new $0$cls$0();
}
"#,
            "Inline variable '$cls'",
        )
        .await;
    expect!["<action not found: Inline variable '$cls'>"].assert_eq(&out);
}

/// Two reads of `new Builder()` would build two objects.
#[tokio::test]
async fn inline_variable_no_action_when_new_is_read_twice() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f() {
    $b = new Builder();
    $0$b$0->add(1);
    return $b->build();
}
"#,
            "Inline variable '$b'",
        )
        .await;
    expect!["<action not found: Inline variable '$b'>"].assert_eq(&out);
}

/// A side-effect-free initializer still inlines into every read.
#[tokio::test]
async fn inline_variable_property_read_twice() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class A {
    private $name;
    public function f() {
        $n = $this->name;
        return $0$n$0 . $n;
    }
}
"#,
            "Inline variable '$n'",
        )
        .await;
    expect![[r#"
        <?php
        class A {
            private $name;
            public function f() {
                return $this->name . $this->name;
            }
        }
    "#]]
    .assert_eq(&out);
}

/// Writing the property between the assignment and the read changes the value.
#[tokio::test]
async fn inline_variable_no_action_when_property_written_before_read() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($a) {
    $x = $a->v;
    $a->v = 2;
    return $0$x$0;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect!["<action not found: Inline variable '$x'>"].assert_eq(&out);
}

/// The same holds for an array element.
#[tokio::test]
async fn inline_variable_no_action_when_element_written_before_read() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($a) {
    $x = $a[0];
    $a[0] = 2;
    return $0$x$0;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect!["<action not found: Inline variable '$x'>"].assert_eq(&out);
}

/// `sort` writes its argument by reference.
#[tokio::test]
async fn inline_variable_no_action_when_operand_sorted_before_read() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($a) {
    $x = $a;
    sort($a);
    return $0$x$0;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect!["<action not found: Inline variable '$x'>"].assert_eq(&out);
}

/// The call would run after the mutation instead of before it.
#[tokio::test]
async fn inline_variable_no_action_when_call_moves_past_mutation() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($a) {
    $x = $a->count();
    $a->add(1);
    return $0$x$0;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect!["<action not found: Inline variable '$x'>"].assert_eq(&out);
}

/// `$r = &1` doesn't parse, and writes through `$r` change `$x`.
#[tokio::test]
async fn inline_variable_no_action_when_bound_by_reference() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f() {
    $x = 1;
    $r = &$x;
    $r = 2;
    return $0$x$0;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect!["<action not found: Inline variable '$x'>"].assert_eq(&out);
}

/// `foreach ([1] as &$v)` is a fatal error.
#[tokio::test]
async fn inline_variable_no_action_when_iterated_by_reference() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f() {
    $x = [1];
    foreach ($0$x$0 as &$v) {}
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect!["<action not found: Inline variable '$x'>"].assert_eq(&out);
}

/// `foreach (… as [$x])` writes `$x`.
#[tokio::test]
async fn inline_variable_no_action_when_destructured_by_foreach() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($rows) {
    $x = 0;
    foreach ($rows as [$x]) {}
    echo $0$x$0;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect!["<action not found: Inline variable '$x'>"].assert_eq(&out);
}

/// The call would run on every iteration.
#[tokio::test]
async fn inline_variable_no_action_when_call_read_in_loop() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($rows) {
    $v = foo();
    foreach ($rows as $row) {
        echo $0$v$0;
    }
}
"#,
            "Inline variable '$v'",
        )
        .await;
    expect!["<action not found: Inline variable '$v'>"].assert_eq(&out);
}

/// The call would run on every call of the closure.
#[tokio::test]
async fn inline_variable_no_action_when_call_read_in_arrow_fn() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f() {
    $v = foo();
    return fn() => $0$v$0;
}
"#,
            "Inline variable '$v'",
        )
        .await;
    expect!["<action not found: Inline variable '$v'>"].assert_eq(&out);
}

/// `compact('x')` reads the variable by name.
#[tokio::test]
async fn inline_variable_no_action_when_scope_uses_compact() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f() {
    $x = foo();
    echo $0$x$0;
    return compact('x');
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect!["<action not found: Inline variable '$x'>"].assert_eq(&out);
}

/// The call would only run when the branch does.
#[tokio::test]
async fn inline_variable_no_action_when_call_read_conditionally() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($c) {
    $v = foo();
    return $c ? $0$v$0 : null;
}
"#,
            "Inline variable '$v'",
        )
        .await;
    expect!["<action not found: Inline variable '$v'>"].assert_eq(&out);
}

/// `1.'a'` lexes as the float `1.` followed by a string.
#[tokio::test]
async fn inline_variable_parenthesizes_number_before_concat() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = 1;
echo $0$x$0.'a';
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        echo (1).'a';
    "#]]
    .assert_eq(&out);
}

/// `Foo::class::make()` isn't the static call; `Foo::make()` is.
#[tokio::test]
async fn inline_variable_class_string_into_static_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f() {
    $c = Foo::class;
    return $0$c$0::make();
}
"#,
            "Inline variable '$c'",
        )
        .await;
    expect![[r#"
        <?php
        function f() {
            return Foo::make();
        }
    "#]]
    .assert_eq(&out);
}

/// Writes to unrelated locals and output don't block a property read.
#[tokio::test]
async fn inline_variable_property_read_past_unrelated_write() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($a) {
    $x = $a->v;
    $n = 2;
    echo $n;
    return $0$x$0 * $n;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        function f($a) {
            $n = 2;
            echo $n;
            return $a->v * $n;
        }
    "#]]
    .assert_eq(&out);
}

/// A constant initializer inlines into loops and arrow functions.
#[tokio::test]
async fn inline_variable_constant_into_loop_and_arrow_fn() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function f($rows) {
    $limit = 10;
    foreach ($rows as $row) {
        $row->cap($0$limit$0);
    }
    return fn() => $limit;
}
"#,
            "Inline variable '$limit'",
        )
        .await;
    expect![[r#"
        <?php
        function f($rows) {
            foreach ($rows as $row) {
                $row->cap(10);
            }
            return fn() => 10;
        }
    "#]]
    .assert_eq(&out);
}
