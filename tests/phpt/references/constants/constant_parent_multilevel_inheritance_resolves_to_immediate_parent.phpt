===description===
`parent::CONST` is compile-time resolved to the *immediate* `extends`
class — in a three-level hierarchy, `Child`'s `parent::LABEL` must
resolve to `Middle`'s declaration (`Middle` is the class actually named
in `Child`'s `extends` clause), not walk further up to `Base`.
===request===
references include_declaration
===file===
<?php
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
