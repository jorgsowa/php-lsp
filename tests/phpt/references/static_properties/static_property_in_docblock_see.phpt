===description===
Static property references include docblock @see links, resolving the class case-insensitively.
===request===
references
===file:src/Util/Docblock.php===
<?php
namespace Psy\Util;

class Docblock {
    public static $vec$0tors = [];

    /**
     * Tags described by {@see DocBlock::$vectors} are split into parts.
     */
    public function parse(): void {
        $count = \count(self::$vectors);
    }
}

===file:src/Formatter/DocblockFormatter.php===
<?php
namespace Psy\Formatter;

use Psy\Util\Docblock;

class DocblockFormatter {
    /**
     * @see DocBlock::$vectors
     * @see Other::$vectors
     */
    public static function format(Docblock $docblock): array {
        return \array_keys($docblock::$vectors);
    }
}
===expect===
src/Formatter/DocblockFormatter.php:11:39-11:46
src/Formatter/DocblockFormatter.php:7:23-7:30
src/Util/Docblock.php:10:31-10:38
src/Util/Docblock.php:7:42-7:49
