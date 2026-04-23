<?php

/*
 * This file is part of the SymfonyCasts SassBundle package.
 * Copyright (c) SymfonyCasts <https://symfonycasts.com/>
 * For the full copyright and license information, please view the LICENSE
 * file that was distributed with this source code.
 */

namespace Symfonycasts\SassBundle\Command;

use Symfony\Component\Console\Attribute\AsCommand;
use Symfony\Component\Console\Command\Command;
use Symfony\Component\Console\Input\InputInterface;
use Symfony\Component\Console\Output\OutputInterface;
use Symfony\Component\Console\Style\SymfonyStyle;
use Symfonycasts\SassBundle\SassBuilder;

#[AsCommand(
    name: 'sass:build',
    description: 'Builds the Sass assets'
)]
class SassBuildCommand extends Command
{
    public function __construct(
        private SassBuilder $sassBuilder
    ) {
        parent::__construct();
    }

    protected function configure(): void
    {
        $this->addOption('watch', 'w', null, 'Watch for changes and rebuild automatically');
    }

    protected function execute(InputInterface $input, OutputInterface $output): int
    {
        $io = new SymfonyStyle($input, $output);

        $this->sassBuilder->setOutput($io);

        $process = $this->sassBuilder->runBuild(
            $input->getOption('watch')
        );

        $process->wait(function ($type, $buffer) use ($io) {
            $io->write($buffer);
        });

        if (!$process->isSuccessful()) {
            $io->error('Sass build failed');

            return self::FAILURE;
        }

        return self::SUCCESS;
    }
}
