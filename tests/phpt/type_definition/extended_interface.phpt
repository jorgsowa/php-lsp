===description===
Parameter typed as interface that extends another.
===request===
type_definition
===file===
<?php
interface Animal {}
interface Pet extends Animal {}
//        ^^^ type
function adopt(Pet $pet$0): void {}
