<?php

namespace App\Service;

use App\Model\User;

class Greeter
{
    public function greet(User $user): string
    {
        return $user->greeting();
    }
}
