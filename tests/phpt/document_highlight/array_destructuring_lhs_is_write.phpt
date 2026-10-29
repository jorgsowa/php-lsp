===description===
Array-destructuring assignment: `[$a, $b] = expr` — both `$a` and `$b` on
the LHS are write positions. Previously they were tagged READ because the
`ExprKind::Assign` handler only checked for a direct `Variable` target and
fell into `visit_expr` for any other lhs, which produces READ.
===request===
document_highlight
===file===
<?php
function foo(): void {
    [$fi$0rst, $second] = [1, 2];
//   ^^^^^^ write
    echo $first;
//       ^^^^^^ read
}
