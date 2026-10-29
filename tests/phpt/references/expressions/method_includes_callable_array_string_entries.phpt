===description===
PHP callable arrays encode method references in string literals:
`[Service::class, 'handle']`, `[$service, 'handle']`, and similar framework
callback registrations. PHPStorm includes these in find-usages; omitting
them makes controller/listener/action methods look unused even when they are
wired through a dispatcher.
===request===
references include_declaration
===file===
<?php
class Handler {
    public function han$0dle(): void {}
    //              ^^^^^^ def
}
$handler = new Handler();
$callable = [$handler, 'handle'];
//                      ^^^^^^ ref
$static = [Handler::class, 'handle'];
//                          ^^^^^^ ref
