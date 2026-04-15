<?php

declare(strict_types=1);

namespace App\Bench;

/**
 * Validates User entity data before persistence.
 */
class UserValidator
{
    private const MIN_AGE = 0;
    private const MAX_AGE = 150;

    /**
     * Run all validation checks against the given user.
     *
     * @param User $user
     * @return bool
     */
    public function validate(User $user): bool
    {
        return $this->validateEmail($user->getEmail())
            && $this->validateAge($user->getAge());
    }

    /**
     * Check that an e-mail address looks structurally valid.
     *
     * @param string $email
     * @return bool
     */
    public function validateEmail(string $email): bool
    {
        return filter_var($email, FILTER_VALIDATE_EMAIL) !== false;
    }

    /**
     * Check that an age is within an acceptable range.
     *
     * @param int $age
     * @return bool
     */
    public function validateAge(int $age): bool
    {
        return $age >= self::MIN_AGE && $age <= self::MAX_AGE;
    }

    /**
     * Check that a name is non-empty and not too long.
     *
     * @param string $name
     * @return bool
     */
    public function validateName(string $name): bool
    {
        $len = strlen($name);
        return $len > 0 && $len <= 255;
    }
}
