<?php

/*
 * This file is part of the SymfonyCasts SassBundle package.
 * Copyright (c) SymfonyCasts <https://symfonycasts.com/>
 * For the full copyright and license information, please view the LICENSE
 * file that was distributed with this source code.
 */

namespace Symfonycasts\SassBundle;

use Symfony\Component\Console\Style\SymfonyStyle;
use Symfony\Component\Process\InputStream;
use Symfony\Component\Process\Process;

class SassBuilder
{
    /**
     * Run "sass --help" to see all options.
     *
     * @see https://sass-lang.com/documentation/cli/dart-sass/
     */
    private const SASS_OPTIONS = [
        // Input and Output
        '--style' => 'expanded',                // Output style.  [expanded (default), compressed]
        '--[no-]charset' => null,               // Emit a @charset or BOM for CSS with non-ASCII characters.
        '--[no-]error-css' => null,             // Emit a CSS file when an error occurs.
        '--load-path' => null,                  // Additional load paths
        // Source Maps
        '--[no-]source-map' => true,            // Whether to generate source maps. (defaults to on)
        '--[no-]embed-sources' => null,         // Embed source file contents in source maps.
        '--[no-]embed-source-map' => null,      // Embed source map contents in CSS.
        // Warnings
        '--[no-]quiet' => null,                 // Don't print warnings.
        '--[no-]quiet-deps' => null,            // Don't print deprecation warnings for dependencies.
        // Other
        '--[no-]stop-on-error' => null,         // Don't compile more files once an error is encountered.
        '--[no-]trace' => null,                 // Print full Dart stack traces for exceptions.
    ];

    private ?SymfonyStyle $output = null;

    /**
     * @var array<string, bool|string>
     */
    private array $sassOptions;

    /**
     * @param array<string>              $sassPaths
     * @param array<string, bool|string> $sassOptions
     */
    public function __construct(
        private readonly array $sassPaths,
        private readonly string $cssPath,
        private readonly string $projectRootDir,
        private readonly ?string $binaryPath,
        bool|array $sassOptions = [],
    ) {
        if (\is_bool($sassOptions)) {
            // Until 0.4, the $sassOptions argument was a boolean named $embedSourceMap
            trigger_deprecation('symfonycasts/sass-bundle', '0.4', 'Passing a boolean to embed the source map is deprecated. Set \'sass_options.embed_source_map\' instead.');
            $sassOptions = ['embed_source_map' => $sassOptions];
            // ...and source maps were always generated.
            $sassOptions['source_map'] = true;
        }

        $this->setOptions($sassOptions);
    }

    /**
     * @internal
     */
    public static function guessCssNameFromSassFile(string $sassFile, string $outputDirectory): string
    {
        $fileName = basename($sassFile, '.scss');

        return $outputDirectory.'/'.$fileName.'.output.css';
    }

    public function runBuild(bool $watch): Process
    {
        $binary = $this->createBinary();

        $args = [
            ...$this->getScssCssTargets(),
            ...$this->getBuildOptions(['--watch' => $watch]),
        ];

        $process = $binary->createProcess($args);
        if ($watch) {
            $process->setTimeout(null);
            $inputStream = new InputStream();
            $process->setInput($inputStream);
        }

        $this->output?->note('Executing Sass (pass -v to see more details).');
        if ($this->output?->isVerbose()) {
            $this->output->writeln([
                '  Command:',
                '    '.$process->getCommandLine(),
            ]);
        }

        $process->start();

        return $process;
    }

    /**
     * @return array<string>
     */
    public function getScssCssTargets(): array
    {
        $targets = [];
        foreach ($this->sassPaths as $sassPath) {
            if (!is_file($sassPath)) {
                throw new \Exception(sprintf('Could not find Sass file: "%s"', $sassPath));
            }

            $targets[] = $sassPath.':'.$this->guessCssNameFromSassFile($sassPath, $this->cssPath);
        }

        return $targets;
    }

    /**
     * @param array<string, bool|array|string> $options
     *
     * @return list<string>
     */
    public function getBuildOptions(array $options = []): array
    {
        $buildOptions = [];
        $options = [...self::SASS_OPTIONS, ...$this->sassOptions, ...$options];
        foreach ($options as $option => $value) {
            // Set only the defined options.
            if (null === $value) {
                continue;
            }
            // --no-embed-source-map
            // --quiet
            if (str_starts_with($option, '--[no-]')) {
                $buildOptions[] = str_replace('[no-]', $value ? '' : 'no-', $option);
                continue;
            }
            // --style=compressed
            if (\is_string($value)) {
                $buildOptions[] = $option.'='.$value;
                continue;
            }
            // --load-path
            if (\is_array($value)) {
                foreach ($value as $item) {
                    $buildOptions[] = $option.'='.$item;
                }
                continue;
            }
            // --update
            // --watch
            if ($value) {
                $buildOptions[] = $option;
            }
        }

        // Filter forbidden associations of options.
        if (\in_array('--no-source-map', $buildOptions, true)) {
            $buildOptions = array_diff($buildOptions, [
                '--embed-sources',
                '--embed-source-map',
                '--no-embed-sources',
                '--no-embed-source-map',
            ]);
        }

        return array_values($buildOptions);
    }

    public function setOutput(SymfonyStyle $output): void
    {
        $this->output = $output;
    }

    private function createBinary(): SassBinary
    {
        return new SassBinary($this->projectRootDir.'/var', $this->binaryPath, $this->output);
    }

    /**
     * Save the Sass options for the build.
     *
     * Options are converted from PHP option names to CLI option names.
     *
     * @param array<string, bool|array|string> $options
     *
     * @see getOptionMap()
     */
    private function setOptions(array $options = []): void
    {
        $sassOptions = [];
        $optionMap = $this->getOptionMap();
        foreach ($options as $option => $value) {
            if (!isset($optionMap[$option])) {
                throw new \InvalidArgumentException(sprintf('Invalid option "%s". Available options are: "%s".', $option, implode('", "', array_keys($optionMap))));
            }
            $sassOptions[$optionMap[$option]] = $value;
        }
        $this->sassOptions = $sassOptions;
    }

    /**
     * Get a map of the Sass options as <php-option> => <cli-option>.
     * Example: ['embed_source_map' => '--embed-source-map'].
     *
     * @return array<string, string>
     */
    private function getOptionMap(): array
    {
        $phpOptions = [];
        foreach (array_keys(self::SASS_OPTIONS) as $cliOption) {
            $phpOption = str_replace(['--[no-]', '--'], '', $cliOption);
            $phpOption = str_replace('-', '_', $phpOption);

            $phpOptions[$phpOption] = $cliOption;
        }

        return $phpOptions;
    }
}
