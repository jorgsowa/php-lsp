<?php

declare(strict_types=1);

namespace App\Bench;

/**
 * Application-layer service wrapping UserRepository.
 */
class UserService
{
    private UserRepository $repo;

    /**
     * @param UserRepository $repo
     */
    public function __construct(UserRepository $repo)
    {
        $this->repo = $repo;
    }

    /**
     * Retrieve a user by ID, or null when not found.
     *
     * @param int $id
     * @return User|null
     */
    public function findById(int $id): ?User
    {
        return $this->repo->findById($id);
    }

    /**
     * Create and persist a new User.
     *
     * @param string $name
     * @param int    $age
     * @return User
     */
    public function createUser(string $name, int $age): User
    {
        $email = strtolower($name) . '@example.com';
        $user  = new User($name, $age, $email);
        $this->repo->save($user);

        return $user;
    }

    /**
     * Delete a user by ID.
     *
     * @param int $id
     * @return void
     */
    public function deleteUser(int $id): void
    {
        $this->repo->delete($id);
    }

    /**
     * Return every user known to the repository.
     *
     * @return User[]
     */
    public function listAll(): array
    {
        return $this->repo->findAll();
    }
}
