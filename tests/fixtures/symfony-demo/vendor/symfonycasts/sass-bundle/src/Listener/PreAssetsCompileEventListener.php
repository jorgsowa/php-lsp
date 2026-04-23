<?php

/*
 * This file is part of the SymfonyCasts SassBundle package.
 * Copyright (c) SymfonyCasts <https://symfonycasts.com/>
 * For the full copyright and license information, please view the LICENSE
 * file that was distributed with this source code.
 */

namespace Symfonycasts\SassBundle\Listener;

use Symfony\Component\AssetMapper\Event\PreAssetsCompileEvent;
use Symfony\Component\Console\Input\ArrayInput;
use Symfony\Component\Console\Style\SymfonyStyle;
use Symfonycasts\SassBundle\SassBuilder;

class PreAssetsCompileEventListener
{
    public function __construct(private readonly SassBuilder $sassBuilder)
    {
    }

    public function __invoke(PreAssetsCompileEvent $preAssetsCompileEvent): void
    {
        $io = new SymfonyStyle(
            new ArrayInput([]),
            $preAssetsCompileEvent->getOutput()
        );

        $this->sassBuilder->setOutput($io);

        $process = $this->sassBuilder->runBuild(false);

        $process->wait(function ($type, $buffer) use ($io) {
            $io->write($buffer);
        });

        if ($process->isSuccessful()) {
            return;
        }

        throw new \RuntimeException(sprintf('Error compiling sass: "%s"', $process->getErrorOutput()));
    }
}
