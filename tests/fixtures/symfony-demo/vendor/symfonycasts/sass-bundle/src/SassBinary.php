<?php

/*
 * This file is part of the SymfonyCasts SassBundle package.
 * Copyright (c) SymfonyCasts <https://symfonycasts.com/>
 * For the full copyright and license information, please view the LICENSE
 * file that was distributed with this source code.
 */

namespace Symfonycasts\SassBundle;

use Symfony\Component\Console\Style\SymfonyStyle;
use Symfony\Component\HttpClient\HttpClient;
use Symfony\Component\Process\Process;
use Symfony\Contracts\HttpClient\HttpClientInterface;

class SassBinary
{
    private const VERSION = '1.69.7';
    private HttpClientInterface $httpClient;

    public function __construct(
        private string $binaryDownloadDir,
        private ?string $binaryPath = null,
        private ?SymfonyStyle $output = null,
        ?HttpClientInterface $httpClient = null
    ) {
        $this->httpClient = $httpClient ?? HttpClient::create();
    }

    /**
     * @param array<string> $args
     */
    public function createProcess(array $args): Process
    {
        if (null === $this->binaryPath) {
            $binary = $this->getDefaultBinaryPath();
            if (!is_file($binary)) {
                $this->downloadExecutable();
            }
        } else {
            $binary = $this->binaryPath;
        }

        array_unshift($args, $binary);

        return new Process($args);
    }

    public function downloadExecutable(): void
    {
        $url = sprintf('https://github.com/sass/dart-sass/releases/download/%s/%s', self::VERSION, $this->getBinaryName());
        $isZip = str_ends_with($url, '.zip');

        $this->output?->note('Downloading Sass binary from '.$url);

        if (!is_dir($this->binaryDownloadDir)) {
            mkdir($this->binaryDownloadDir, 0777, true);
        }

        $targetPath = $this->binaryDownloadDir.'/'.self::getBinaryName();
        $progressBar = null;

        $response = $this->httpClient->request('GET', $url, [
            'on_progress' => function (int $dlNow, int $dlSize, array $info) use (&$progressBar): void {
                if (0 === $dlSize) {
                    return;
                }

                if (!$progressBar) {
                    $progressBar = $this->output?->createProgressBar($dlSize);
                }

                $progressBar?->setProgress($dlNow);
            },
        ]);

        $fileHandler = fopen($targetPath, 'w');
        foreach ($this->httpClient->stream($response) as $chunk) {
            fwrite($fileHandler, $chunk->getContent());
        }

        fclose($fileHandler);
        $progressBar?->finish();
        $this->output?->writeln('');

        if ($isZip) {
            if (!\extension_loaded('zip')) {
                throw new \Exception('Cannot unzip the downloaded sass binary. Please install the "zip" PHP extension.');
            }
            $archive = new \ZipArchive();
            $archive->open($targetPath);
            $archive->extractTo($this->binaryDownloadDir);
            $archive->close();
            unlink($targetPath);

            return;
        } else {
            $archive = new \PharData($targetPath);
            $archive->decompress();
            $archive->extractTo($this->binaryDownloadDir);

            // delete the .tar (the .tar.gz is deleted below)
            unlink(substr($targetPath, 0, -3));
        }

        unlink($targetPath);

        $binaryPath = $this->getDefaultBinaryPath();
        if (!is_file($binaryPath)) {
            throw new \Exception(sprintf('Could not find downloaded binary in "%s".', $binaryPath));
        }

        chmod($binaryPath, 0777);
    }

    public function getBinaryName(): string
    {
        $os = strtolower(\PHP_OS);
        $machine = strtolower(php_uname('m'));

        if (str_contains($os, 'darwin')) {
            if ('arm64' === $machine) {
                return $this->buildBinaryFileName('macos-arm64');
            }

            if ('x86_64' === $machine) {
                return $this->buildBinaryFileName('macos-x64');
            }

            throw new \Exception(sprintf('No matching machine found for Darwin platform (Machine: %s).', $machine));
        }

        if (str_contains($os, 'linux')) {
            $baseName = file_exists('/etc/alpine-release') ? 'linux-musl' : 'linux';
            if ('arm64' === $machine || 'aarch64' === $machine) {
                return $this->buildBinaryFileName($baseName.'-arm64');
            }
            if ('x86_64' === $machine) {
                return $this->buildBinaryFileName($baseName.'-x64');
            }

            throw new \Exception(sprintf('No matching machine found for Linux platform (Machine: %s).', $machine));
        }

        if (str_contains($os, 'win')) {
            if ('x86_64' === $machine || 'amd64' === $machine || 'i586' === $machine) {
                return $this->buildBinaryFileName('windows-x64', true);
            }

            throw new \Exception(sprintf('No matching machine found for Windows platform (Machine: %s).', $machine));
        }

        throw new \Exception(sprintf('Unknown platform or architecture (OS: %s, Machine: %s).', $os, $machine));
    }

    private function buildBinaryFileName(string $os, bool $isWindows = false): string
    {
        return 'dart-sass-'.self::VERSION.'-'.$os.($isWindows ? '.zip' : '.tar.gz');
    }

    private function getDefaultBinaryPath(): string
    {
        return $this->binaryDownloadDir.'/dart-sass/sass';
    }
}
