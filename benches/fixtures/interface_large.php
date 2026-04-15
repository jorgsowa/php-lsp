<?php

declare(strict_types=1);

namespace App\Bench;

/**
 * A large interface with 20+ method signatures for parse-benchmark purposes.
 */
interface RepositoryInterface
{
    public function find(int $id): ?object;

    public function findAll(): array;

    /** @param array<string, mixed> $criteria */
    public function findBy(array $criteria): array;

    /** @param array<string, mixed> $criteria */
    public function findOneBy(array $criteria): ?object;

    public function count(): int;

    /** @param array<string, mixed> $criteria */
    public function countBy(array $criteria): int;

    public function save(object $entity): void;

    public function delete(object $entity): void;

    public function deleteById(int $id): bool;

    /** @param object[] $entities */
    public function saveAll(array $entities): void;

    /** @param object[] $entities */
    public function deleteAll(array $entities): void;

    public function exists(int $id): bool;

    /** @param array<string, mixed> $criteria */
    public function existsBy(array $criteria): bool;

    /**
     * @param array<string, string> $orderBy  e.g. ['name' => 'ASC']
     * @param int|null $limit
     * @param int|null $offset
     * @return object[]
     */
    public function findAllOrdered(array $orderBy, ?int $limit = null, ?int $offset = null): array;

    /**
     * @param array<string, mixed> $criteria
     * @param array<string, string> $orderBy
     * @return object[]
     */
    public function findByOrdered(array $criteria, array $orderBy): array;

    public function beginTransaction(): void;

    public function commit(): void;

    public function rollback(): void;

    public function flush(): void;

    public function clear(): void;

    public function refresh(object $entity): void;

    public function detach(object $entity): void;

    public function merge(object $entity): object;

    /** @return class-string */
    public function getEntityClass(): string;

    public function getTableName(): string;

    public function createQueryBuilder(string $alias): object;

    public function createNativeQuery(string $sql): object;

    /** @param array<string, mixed> $params */
    public function executeRaw(string $sql, array $params = []): array;

    public function getLastInsertId(): int;
}
