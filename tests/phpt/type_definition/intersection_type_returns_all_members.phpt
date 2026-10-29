===description===
Intersection types (PHP 8.1+) return all matching types.
===request===
type_definition
===file===
<?php
interface Readable {}
//        ^^^^^^^^ type
interface Writable {}
//        ^^^^^^^^ type
function process(Readable&Writable $rw$0): void {}
