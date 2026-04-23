<?php

/*
 * This file is part of the SymfonyCasts SassBundle package.
 * Copyright (c) SymfonyCasts <https://symfonycasts.com/>
 * For the full copyright and license information, please view the LICENSE
 * file that was distributed with this source code.
 */

namespace Symfonycasts\SassBundle\AssetMapper;

use Symfony\Component\AssetMapper\AssetMapperInterface;
use Symfony\Component\AssetMapper\Compiler\AssetCompilerInterface;
use Symfony\Component\AssetMapper\MappedAsset;
use Symfony\Component\Filesystem\Path;
use Symfonycasts\SassBundle\SassBuilder;

class SassCssCompiler implements AssetCompilerInterface
{
    public function __construct(
        private array $scssPaths,
        private string $cssPathDirectory,
        private string $projectDir,
        private readonly SassBuilder $sassBuilder
    ) {
    }

    public function supports(MappedAsset $asset): bool
    {
        foreach ($this->scssPaths as $path) {
            $absolutePath = Path::isAbsolute($path) ? $path : Path::makeAbsolute($path, $this->projectDir);

            if (realpath($asset->sourcePath) === realpath($absolutePath)) {
                return true;
            }
        }

        return false;
    }

    public function compile(string $content, MappedAsset $asset, AssetMapperInterface $assetMapper): string
    {
        $cssFile = $this->sassBuilder->guessCssNameFromSassFile($asset->sourcePath, $this->cssPathDirectory);

        $asset->addFileDependency($cssFile);

        if (!is_file($cssFile) || ($content = file_get_contents($cssFile)) === false) {
            throw new \RuntimeException('The file '.$cssFile.' doesn\'t exist, run php bin/console sass:build');
        }

        return $content;
    }
}
