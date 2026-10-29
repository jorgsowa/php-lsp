===description===
A concrete class that extends an abstract class without implementing all
abstract methods produces an `UnimplementedAbstractMethod` diagnostic.
===allow-invalid-php===
===file===
<?php
abstract class BaseController {
    abstract public function handle(): void;
}

  class DashboardController extends BaseController {}
//^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: Class DashboardController must implement abstract method handle()
