===description===
Same constant name in different classes must not interfere
===request===
references include_declaration
===file===
<?php
class DatabaseConfig {
    const PO$0RT = 5432;
    //    ^^^^ def
}

class CacheConfig {
    const PORT = 6379;
}

$db_port = DatabaseConfig::PORT;
//                         ^^^^ ref

$cache_port = CacheConfig::PORT;
// Should not match DatabaseConfig::PORT
