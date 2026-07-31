//! Code action: "Convert switch to match"
//!
//! Converts a `switch` statement to a `return match(...)` expression when:
//! - Every non-empty case body is a single `return <expr>;`
//! - A `default` case is present (ensures equivalent behavior)
//! - No leveled `break N;` statements

use super::*;
use expect_test::expect;

// ── Offered ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn switch_to_match_offered_for_simple_switch() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
function getLabel(string $status): string {
    $0switch ($status) {
        case 'active':
            return 'Active';
        case 'inactive':
            return 'Inactive';
        default:
            return 'Unknown';
    }$0
}
"#,
        )
        .await;
    assert!(
        out.contains("Convert switch to match"),
        "expected action in: {out}"
    );
}

#[tokio::test]
async fn switch_to_match_offered_when_cursor_anywhere_in_switch() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
function getLabel(string $status): string {
    switch ($status) {
        case 'active':
            return 'Active';
        default:
            return $0'Unknown';
    }
}
"#,
        )
        .await;
    assert!(
        out.contains("Convert switch to match"),
        "expected action in: {out}"
    );
}

// ── Not offered ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn switch_to_match_not_offered_without_default() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
function getLabel(string $status): string {
    $0switch ($status) {
        case 'active':
            return 'Active';
        case 'inactive':
            return 'Inactive';
    }$0
    return 'Unknown';
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert switch to match"),
        "should not offer without default, got: {out}"
    );
}

#[tokio::test]
async fn switch_to_match_not_offered_for_statement_body() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
function run(string $cmd): void {
    $0switch ($cmd) {
        case 'start':
            doStart();
            break;
        default:
            doNothing();
    }$0
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert switch to match"),
        "should not offer when case body is not a return, got: {out}"
    );
}

#[tokio::test]
async fn switch_to_match_not_offered_for_multi_statement_case() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
function getLabel(string $status): string {
    $0switch ($status) {
        case 'active':
            $label = 'Active';
            return $label;
        default:
            return 'Unknown';
    }$0
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert switch to match"),
        "should not offer for multi-statement case body, got: {out}"
    );
}

#[tokio::test]
async fn switch_to_match_not_offered_for_alternative_syntax() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
function getLabel(string $status): string {
    $0switch ($status):
        case 'active':
            return 'Active';
        default:
            return 'Unknown';
    endswitch;$0
    return '';
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert switch to match"),
        "should not offer for alternative switch syntax, got: {out}"
    );
}

/// `switch` compares with loose `==`, `match` with strict `===`. `0 == null`
/// and `0 == false` both hold loosely but not strictly, so converting a
/// switch with mixed-kind case literals would silently change behavior.
#[tokio::test]
async fn switch_to_match_not_offered_for_mixed_type_case_literals() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
function describe(mixed $x): string {
    $0switch ($x) {
        case 0:
            return 'zero-ish';
        case null:
            return 'nullish';
        default:
            return 'other';
    }$0
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert switch to match"),
        "should not offer when case literals mix int/null (loose-vs-strict quirk), got: {out}"
    );
}

#[tokio::test]
async fn switch_to_match_offered_when_case_literals_share_one_kind() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
function grade(int $score): string {
    $0switch ($score) {
        case 5:
            return 'A';
        case 4:
            return 'B';
        default:
            return 'C';
    }$0
}
"#,
        )
        .await;
    assert!(
        out.contains("Convert switch to match"),
        "should still offer when every case literal is the same kind, got: {out}"
    );
}

// ── Applied edits ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn switch_to_match_converts_basic_switch() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function getLabel(string $status): string {
    $0switch ($status) {
        case 'active':
            return 'Active';
        case 'inactive':
            return 'Inactive';
        default:
            return 'Unknown';
    }$0
}
"#,
            "Convert switch to match",
        )
        .await;
    expect![[r#"
        <?php
        function getLabel(string $status): string {
            return match ($status) {
                'active' => 'Active',
                'inactive' => 'Inactive',
                default => 'Unknown',
            };
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn switch_to_match_groups_fall_through_cases() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function getGroup(int $n): string {
    $0switch ($n) {
        case 1:
        case 2:
            return 'small';
        case 3:
        case 4:
        case 5:
            return 'medium';
        default:
            return 'large';
    }$0
}
"#,
            "Convert switch to match",
        )
        .await;
    expect![[r#"
        <?php
        function getGroup(int $n): string {
            return match ($n) {
                1, 2 => 'small',
                3, 4, 5 => 'medium',
                default => 'large',
            };
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn switch_to_match_strips_dead_break_after_return() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function getLabel(string $status): string {
    $0switch ($status) {
        case 'active':
            return 'Active';
            break;
        default:
            return 'Unknown';
            break;
    }$0
}
"#,
            "Convert switch to match",
        )
        .await;
    expect![[r#"
        <?php
        function getLabel(string $status): string {
            return match ($status) {
                'active' => 'Active',
                default => 'Unknown',
            };
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn switch_to_match_preserves_expression_complexity() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function getDiscount(string $type, int $amount): int {
    $0switch ($type) {
        case 'premium':
            return $amount * 2;
        case 'standard':
            return $amount + 10;
        default:
            return 0;
    }$0
}
"#,
            "Convert switch to match",
        )
        .await;
    expect![[r#"
        <?php
        function getDiscount(string $type, int $amount): int {
            return match ($type) {
                'premium' => $amount * 2,
                'standard' => $amount + 10,
                default => 0,
            };
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn switch_to_match_works_inside_class_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Formatter {
    public function format(string $type): string {
        $0switch ($type) {
            case 'upper':
                return strtoupper($this->value);
            default:
                return $this->value;
        }$0
    }
}
"#,
            "Convert switch to match",
        )
        .await;
    expect![[r#"
        <?php
        class Formatter {
            public function format(string $type): string {
                return match ($type) {
                    'upper' => strtoupper($this->value),
                    default => $this->value,
                };
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn switch_to_match_handles_integer_cases() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function grade(int $score): string {
    $0switch ($score) {
        case 5:
            return 'Excellent';
        case 4:
            return 'Good';
        case 3:
            return 'Average';
        default:
            return 'Poor';
    }$0
}
"#,
            "Convert switch to match",
        )
        .await;
    expect![[r#"
        <?php
        function grade(int $score): string {
            return match ($score) {
                5 => 'Excellent',
                4 => 'Good',
                3 => 'Average',
                default => 'Poor',
            };
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn switch_to_match_innermost_wins_for_nested_switch() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function classify(string $a, string $b): string {
    switch ($a) {
        case 'x':
            $0switch ($b) {
                case 'y':
                    return 'xy';
                default:
                    return 'x_other';
            }$0
        default:
            return 'other';
    }
}
"#,
            "Convert switch to match",
        )
        .await;
    expect![[r#"
        <?php
        function classify(string $a, string $b): string {
            switch ($a) {
                case 'x':
                    return match ($b) {
                        'y' => 'xy',
                        default => 'x_other',
                    };
                default:
                    return 'other';
            }
        }
    "#]]
    .assert_eq(&out);
}
