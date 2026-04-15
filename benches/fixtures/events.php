<?php

declare(strict_types=1);

namespace App\Bench;

/**
 * Fired after a new user is created.
 */
class UserCreatedEvent
{
    private User $user;

    public function __construct(User $user)
    {
        $this->user = $user;
    }

    public function getUser(): User
    {
        return $this->user;
    }
}

/**
 * Fired after a user is deleted.
 */
class UserDeletedEvent
{
    private int $userId;

    public function __construct(int $userId)
    {
        $this->userId = $userId;
    }

    public function getUserId(): int
    {
        return $this->userId;
    }
}

/**
 * Fired after a user's data is updated.
 */
class UserUpdatedEvent
{
    private User $user;

    public function __construct(User $user)
    {
        $this->user = $user;
    }

    public function getUser(): User
    {
        return $this->user;
    }
}
