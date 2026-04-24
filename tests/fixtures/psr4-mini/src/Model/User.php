<?php

namespace App\Model;

class User
{
    public function __construct(
        public string $name,
        public int $age,
    ) {
    }

    public function greeting(): string
    {
        return "Hello, {$this->name}";
    }
}
