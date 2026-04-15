<?php

declare(strict_types=1);

namespace App\Bench;

/**
 * A simple user entity used for parse benchmarks.
 */
class User
{
    private string $name;
    private int $age;
    private string $email;

    public function __construct(string $name, int $age, string $email)
    {
        $this->name  = $name;
        $this->age   = $age;
        $this->email = $email;
    }

    /**
     * Return the user's full display name.
     */
    public function getName(): string
    {
        return $this->name;
    }

    /**
     * Return the user's age in years.
     */
    public function getAge(): int
    {
        return $this->age;
    }

    /**
     * Return the user's email address.
     */
    public function getEmail(): string
    {
        return $this->email;
    }

    public function isAdult(): bool
    {
        return $this->age >= 18;
    }

    public function __toString(): string
    {
        return sprintf('%s <%s>', $this->name, $this->email);
    }
}
