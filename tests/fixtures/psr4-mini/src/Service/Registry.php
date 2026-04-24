<?php

namespace App\Service;

use App\Model\User;

class Registry
{
    /** @var User[] */
    private array $users = [];

    public function register(User $user): void
    {
        $this->users[] = $user;
    }

    public function count(): int
    {
        return count($this->users);
    }
}
