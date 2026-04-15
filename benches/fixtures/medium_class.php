<?php

declare(strict_types=1);

namespace App\Bench;

interface Renderable
{
    public function render(): string;
}

interface Loggable
{
    public function toLogString(): string;
}

trait HasTimestamps
{
    private \DateTimeImmutable $createdAt;
    private \DateTimeImmutable $updatedAt;

    public function getCreatedAt(): \DateTimeImmutable
    {
        return $this->createdAt;
    }

    public function getUpdatedAt(): \DateTimeImmutable
    {
        return $this->updatedAt;
    }

    public function touch(): void
    {
        $this->updatedAt = new \DateTimeImmutable();
    }
}

trait HasSoftDelete
{
    private ?\DateTimeImmutable $deletedAt = null;

    public function softDelete(): void
    {
        $this->deletedAt = new \DateTimeImmutable();
    }

    public function isDeleted(): bool
    {
        return $this->deletedAt !== null;
    }

    public function restore(): void
    {
        $this->deletedAt = null;
    }
}

abstract class BaseModel implements Loggable
{
    use HasTimestamps;

    protected int $id;
    protected string $type;

    public function __construct(int $id, string $type)
    {
        $this->id        = $id;
        $this->type      = $type;
        $this->createdAt = new \DateTimeImmutable();
        $this->updatedAt = new \DateTimeImmutable();
    }

    public function getId(): int
    {
        return $this->id;
    }

    public function getType(): string
    {
        return $this->type;
    }

    abstract public function validate(): bool;

    public function toLogString(): string
    {
        return sprintf('[%s#%d]', $this->type, $this->id);
    }
}

class Article extends BaseModel implements Renderable
{
    use HasSoftDelete;

    private string $title;
    private string $body;
    private string $author;
    /** @var string[] */
    private array $tags;

    public function __construct(int $id, string $title, string $body, string $author)
    {
        parent::__construct($id, 'article');
        $this->title  = $title;
        $this->body   = $body;
        $this->author = $author;
        $this->tags   = [];
    }

    public function getTitle(): string
    {
        return $this->title;
    }

    public function setTitle(string $title): void
    {
        $this->title = $title;
        $this->touch();
    }

    public function getBody(): string
    {
        return $this->body;
    }

    public function setBody(string $body): void
    {
        $this->body = $body;
        $this->touch();
    }

    public function getAuthor(): string
    {
        return $this->author;
    }

    /** @param string[] $tags */
    public function setTags(array $tags): void
    {
        $this->tags = $tags;
    }

    /** @return string[] */
    public function getTags(): array
    {
        return $this->tags;
    }

    public function addTag(string $tag): void
    {
        if (!in_array($tag, $this->tags, true)) {
            $this->tags[] = $tag;
        }
    }

    public function validate(): bool
    {
        return $this->title !== '' && $this->body !== '' && $this->author !== '';
    }

    public function render(): string
    {
        $tags = implode(', ', $this->tags);
        return sprintf(
            "<article><h1>%s</h1><p>%s</p><footer>%s — tags: %s</footer></article>",
            htmlspecialchars($this->title),
            htmlspecialchars($this->body),
            htmlspecialchars($this->author),
            htmlspecialchars($tags),
        );
    }

    public function toLogString(): string
    {
        return sprintf('[article#%d title=%s]', $this->id, $this->title);
    }
}

class DraftArticle extends Article
{
    private bool $submitted = false;

    public function submit(): void
    {
        if (!$this->validate()) {
            throw new \RuntimeException('Cannot submit an invalid draft.');
        }
        $this->submitted = true;
        $this->touch();
    }

    public function isSubmitted(): bool
    {
        return $this->submitted;
    }

    public function validate(): bool
    {
        return parent::validate() && !$this->isDeleted();
    }
}
