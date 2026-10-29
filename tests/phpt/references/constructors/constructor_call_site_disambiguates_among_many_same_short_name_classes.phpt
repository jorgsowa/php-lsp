===description===
`parent::__construct()` resolution (`resolve_parent_construct_class`) is
keyed off `WorkspaceIndexData::classes_by_name`, which buckets every
class by *short name only*. With several unrelated classes named `Base`
scattered across the workspace, the lookup must still land on the one
FQN actually named in the `extends` clause — not an arbitrary same-named
decoy from a different namespace.
===request===
references include_declaration
===file:alpha/base.php===
<?php
namespace Alpha;
class Base {
    public function __construct(int $id) {}
}

===file:alpha/child.php===
<?php
namespace Alpha;
class Child extends Base {
    public function __construct(int $id) {
        parent::__con$0struct($id);
        //      ^^^^^^^^^^^ ref
    }
}

===file:beta/base.php===
<?php
namespace Beta;
class Base {
    public function __construct(string $s) {}
}

===file:gamma/base.php===
<?php
namespace Gamma;
class Base {
    public function __construct(array $a) {}
}

===file:delta/base.php===
<?php
namespace Delta;
class Base {
    public function __construct(float $f) {}
}

===file:usage.php===
<?php
new \Alpha\Base(1);
//  ^^^^^^^^^^^ ref
new \Beta\Base('x');
new \Gamma\Base([]);
new \Delta\Base(1.5);
