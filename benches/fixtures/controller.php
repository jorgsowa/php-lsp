<?php

declare(strict_types=1);

namespace App\Bench;

/**
 * HTTP-style controller that orchestrates UserService and UserValidator.
 *
 * Cross-file references in this file:
 *   - UserService   (service.php)
 *   - UserValidator (validator.php)
 *   - UserCreatedEvent / UserDeletedEvent (events.php)
 *   - User          (small_class.php)
 */
class UserController
{
    private UserService $service;

    private UserValidator $validator;

    /**
     * @param UserService   $service
     * @param UserValidator $validator
     */
    public function __construct(UserService $service, UserValidator $validator)
    {
        $this->service   = $service;
        $this->validator = $validator;
    }

    /**
     * List all users.
     *
     * @return User[]
     */
    public function index(): array
    {
        return $this->service->listAll();
    }

    /**
     * Show a single user by ID.
     *
     * @param int $id
     * @return User|null
     */
    public function show(int $id): ?User
    {
        return $this->service->findById($id);
    }

    /**
     * Create a new user from the given data.
     *
     * @param array $data  Expected keys: name (string), age (int)
     * @return User|null   Null when validation fails.
     */
    public function store(array $data): ?User
    {
        $name = (string) ($data['name'] ?? '');
        $age  = (int)    ($data['age']  ?? 0);

        $user = $this->service->createUser($name, $age);

        if (!$this->validator->validate($user)) {
            $this->service->deleteUser($user->getId());
            return null;
        }

        $event = new UserCreatedEvent($user);

        return $event->getUser();
    }

    /**
     * Delete a user by ID.
     *
     * @param int $id
     * @return bool
     */
    public function destroy(int $id): bool
    {
        $user = $this->service->findById($id);

        if ($user === null) {
            return false;
        }

        $this->service->deleteUser($id);

        $event = new UserDeletedEvent($id);
        $event->getUserId();

        return true;
    }
}
