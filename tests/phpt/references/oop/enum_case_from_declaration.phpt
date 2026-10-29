===description===
Find-references from an enum case's own declaration must find every
`Status::Active`-style access, not just the declaration itself. Case
names are conventionally PascalCase (like class names), and
`Status::Active` is syntactically identical to real class-constant
access, so both the declaration-site classification and the constant
walker's declaration matching need explicit `EnumMemberKind::Case`
handling — without it, this returns only the declaration and misses
every real usage.
===request===
references include_declaration
===file===
<?php
enum Status {
    case Act$0ive;
    //   ^^^^^^ def
}
function describe(Status $s): string {
    return match ($s) {
        Status::Active => 'active',
        //      ^^^^^^ ref
    };
}
echo Status::Active;
//           ^^^^^^ ref
