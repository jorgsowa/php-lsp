<?php

/*
 * This file is part of the SymfonyCasts SassBundle package.
 * Copyright (c) SymfonyCasts <https://symfonycasts.com/>
 * For the full copyright and license information, please view the LICENSE
 * file that was distributed with this source code.
 */

namespace Symfonycasts\SassBundle;

use Symfony\Component\DependencyInjection\Extension\ExtensionInterface;
use Symfony\Component\HttpKernel\Bundle\Bundle;
use Symfonycasts\SassBundle\DependencyInjection\SymfonycastsSassExtension;

class SymfonycastsSassBundle extends Bundle
{
    protected function createContainerExtension(): ?ExtensionInterface
    {
        return new SymfonycastsSassExtension();
    }
}
