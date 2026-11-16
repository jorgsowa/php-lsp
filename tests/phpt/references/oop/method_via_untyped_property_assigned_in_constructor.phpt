===request===
references include_declaration
===file:src/Context.php===
<?php
namespace Psy;

class Context {
    public function get$0All(): array { return []; }
    //              ^^^^^^ def
}

===file:src/Command/ListCommand/VariableEnumerator.php===
<?php
namespace Psy\Command\ListCommand;

use Psy\Context;
use Psy\VarDumper\Presenter;

class VariableEnumerator extends Enumerator {
    private $context;

    /**
     * @param Presenter $presenter
     * @param Context   $context
     */
    public function __construct(Presenter $presenter, Context $context) {
        $this->context = $context;
        parent::__construct($presenter);
    }

    protected function getVariables(bool $showAll): array {
        $scopeVars = $this->context->getAll();
        //                           ^^^^^^ ref
        return $scopeVars;
    }
}

===disk:src/Command/ListCommand/Enumerator.php===
<?php
namespace Psy\Command\ListCommand;

use Psy\VarDumper\Presenter;

abstract class Enumerator {
    private $presenter;

    public function __construct(Presenter $presenter) {
        $this->presenter = $presenter;
    }
}

===file:src/Shell.php===
<?php
namespace Psy;

class Shell {
    private Context $context;

    public function getScopeVariables(): array {
        return $this->context->getAll();
        //                     ^^^^^^ ref
    }
}

===disk:src/Readline/History.php===
<?php
namespace Psy\Readline;

class History {
    public function getAll(): array { return []; }
}

===file:src/Readline/HistoryPrinter.php===
<?php
namespace Psy\Readline;

class HistoryPrinter {
    private $history;

    public function __construct(History $history) {
        $this->history = $history;
    }

    public function lines(): array {
        return $this->history->getAll();
    }
}
