<?php

/*
 * This file is part of the SymfonyCasts SassBundle package.
 * Copyright (c) SymfonyCasts <https://symfonycasts.com/>
 * For the full copyright and license information, please view the LICENSE
 * file that was distributed with this source code.
 */

namespace Symfonycasts\SassBundle\AssetMapper;

use Symfony\Component\AssetMapper\Path\PublicAssetsPathResolverInterface;

class SassPublicPathAssetPathResolver implements PublicAssetsPathResolverInterface
{
    public function __construct(private readonly PublicAssetsPathResolverInterface $decorator)
    {
    }

    public function resolvePublicPath(string $logicalPath): string
    {
        $path = $this->decorator->resolvePublicPath($logicalPath);

        if (str_contains($path, '.scss')) {
            return str_replace('.scss', '.css', $path);
        }

        return $path;
    }

    public function getPublicFilesystemPath(): string
    {
        if (!method_exists($this->decorator, 'getPublicFilesystemPath')) {
            throw new \Exception('Something weird happened, we should never reach this line!');
        }

        $path = $this->decorator->getPublicFilesystemPath();

        if (str_contains($path, '.scss')) {
            return str_replace('.scss', '.css', $path);
        }

        return $path;
    }
}
