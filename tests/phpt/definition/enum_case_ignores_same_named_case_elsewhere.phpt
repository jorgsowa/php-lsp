===request===
definition
===file:a.php===
<?php
enum Other { case ACTIVE; }
===file:b.php===
<?php
enum Status { case ACTIVE; }
//                 ^^^^^^ def
===file:main.php===
<?php
Status::AC$0TIVE;
