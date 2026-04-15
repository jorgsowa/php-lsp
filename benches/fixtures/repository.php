<?php

declare(strict_types=1);

namespace App\Bench;

/**
 * Provides data access for User entities.
 */
class UserRepository
{
    /** @var User[] */
    private array $store = [];

    private int $nextId = 1;

    /**
     * Find a user by their numeric ID.
     *
     * @param int $id
     * @return User|null
     */
    public function findById(int $id): ?User
    {
        foreach ($this->store as $user) {
            if ($user->getId() === $id) {
                return $user;
            }
        }

        return null;
    }

    /**
     * Persist or update a User record.
     *
     * @param User $user
     */
    public function save(User $user): void
    {
        $this->store[$user->getId()] = $user;
    }

    /**
     * Remove a user by ID. Returns true if a record was removed.
     *
     * @param int $id
     * @return bool
     */
    public function delete(int $id): bool
    {
        if (isset($this->store[$id])) {
            unset($this->store[$id]);
            return true;
        }

        return false;
    }

    /**
     * Return every stored User.
     *
     * @return User[]
     */
    public function findAll(): array
    {
        return array_values($this->store);
    }
}
