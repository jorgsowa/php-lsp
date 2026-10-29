===file===
<?php
class NotATrait {}
class User {
    use NotATrait;
//      ^^^^^^^^^ error: NotATrait is a class, not a trait
}
