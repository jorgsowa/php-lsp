===request===
references include_declaration
===file===
<?php
class Use$0r {
//    ^^^^ def
}

function getUser(int $id): ?User {
//                          ^^^^ ref
    return null;
}

function setUser(?User $user): void {
//                ^^^^ ref
    if ($user !== null) {}
}
