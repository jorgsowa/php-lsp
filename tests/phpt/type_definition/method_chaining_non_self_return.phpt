===description===
Chaining where an intermediate method returns a *different* class (not self).
Without the correct fallback (resolving the full call, not just its receiver),
typeDefinition would return the receiver's class instead of the return class.
===request===
type_definition
===file===
<?php
class Pipeline {
    public function stage(string $name): Stage { return new Stage(); }
}
class Stage {
//    ^^^^^ type
    public function run(): Result { return new Result(); }
}
class Result {}
$p = new Pipeline();
$p->stage('init')$0->run();
