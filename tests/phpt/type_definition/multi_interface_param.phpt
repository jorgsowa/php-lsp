===description===
Multiple interface inheritance (one class implements two).
===request===
type_definition
===file===
<?php
interface Logger {}
interface Config {}
class App implements Logger, Config {}
//    ^^^ type
function bootstrap(App $app$0): void {}
