===request===
type_definition
===file===
<?php
class QueryBuilder {
//    ^^^^^^^^^^^^ type
    public function select(string $col): self { return $this; }
    public function where(string $cond): self { return $this; }
}
$q = new QueryBuilder();
$q->select('id')->where('active')$0->execute();
