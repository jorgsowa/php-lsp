<?php

/*
 * This file is part of the SymfonyCasts SassBundle package.
 * Copyright (c) SymfonyCasts <https://symfonycasts.com/>
 * For the full copyright and license information, please view the LICENSE
 * file that was distributed with this source code.
 */

namespace Symfonycasts\SassBundle\DependencyInjection;

use Symfony\Component\Config\Definition\Builder\ArrayNodeDefinition;
use Symfony\Component\Config\Definition\Builder\TreeBuilder;
use Symfony\Component\Config\Definition\ConfigurationInterface;
use Symfony\Component\Config\FileLocator;
use Symfony\Component\DependencyInjection\ContainerBuilder;
use Symfony\Component\DependencyInjection\Extension\Extension;
use Symfony\Component\DependencyInjection\Loader;

class SymfonycastsSassExtension extends Extension implements ConfigurationInterface
{
    public function load(array $configs, ContainerBuilder $container): void
    {
        $loader = new Loader\PhpFileLoader($container, new FileLocator(__DIR__.'/../../config'));
        $loader->load('services.php');

        $configuration = $this->getConfiguration($configs, $container);
        $config = $this->processConfiguration($configuration, $configs);

        // BC Layer with SassBundle < 0.4
        if (isset($config['embed_sourcemap'])) {
            $config['sass_options']['embed_source_map'] = $config['embed_sourcemap'];
        }

        $container->findDefinition('sass.builder')
            ->replaceArgument(0, $config['root_sass'])
            ->replaceArgument(1, '%kernel.project_dir%/var/sass')
            ->replaceArgument(3, $config['binary'])
            ->replaceArgument(4, $config['sass_options'])
        ;

        $container->findDefinition('sass.css_asset_compiler')
            ->replaceArgument(0, $config['root_sass'])
            ->replaceArgument(1, '%kernel.project_dir%/var/sass')
        ;
    }

    public function getConfiguration(array $config, ContainerBuilder $container): ?ConfigurationInterface
    {
        return $this;
    }

    public function getConfigTreeBuilder(): TreeBuilder
    {
        $treeBuilder = new TreeBuilder('symfonycasts_sass');

        $rootNode = $treeBuilder->getRootNode();
        \assert($rootNode instanceof ArrayNodeDefinition);

        $rootNode
            ->children()
                ->arrayNode('root_sass')
                    ->info('Path to your Sass root file')
                    ->beforeNormalization()->castToArray()->end()
                    ->cannotBeEmpty()
                    ->scalarPrototype()
                        ->end()
                    ->validate()
                        ->ifTrue(static function (array $paths): bool {
                            if (1 === \count($paths)) {
                                return false;
                            }

                            $filenames = [];
                            foreach ($paths as $path) {
                                $filename = basename($path, '.scss');
                                $filenames[$filename] = $filename;
                            }

                            return \count($filenames) !== \count($paths);
                        })
                        ->thenInvalid('The "root_sass" paths need to end with unique filenames.')
                        ->end()
                    ->defaultValue(['%kernel.project_dir%/assets/styles/app.scss'])
                ->end()
                ->scalarNode('binary')
                    ->info('The Sass binary to use')
                    ->defaultNull()
                ->end()
                ->arrayNode('sass_options')
                    ->addDefaultsIfNotSet()
                    ->children()
                        ->enumNode('style')
                            ->info('The style of the generated CSS: compressed or expanded.')
                            ->values(['compressed', 'expanded'])
                            ->defaultValue('expanded')
                        ->end()
                        ->booleanNode('charset')
                            ->info('Whether to include the charset declaration in the generated Sass.')
                        ->end()
                        ->booleanNode('error_css')
                            ->info('Emit a CSS file when an error occurs.')
                        ->end()
                        ->booleanNode('source_map')
                            ->info('Whether to generate source maps.')
                            ->defaultValue(true)
                        ->end()
                        ->booleanNode('embed_sources')
                            ->info('Embed source file contents in source maps.')
                        ->end()
                        ->booleanNode('embed_source_map')
                            ->info('Embed source map contents in CSS.')
                            ->defaultValue('%kernel.debug%')
                        ->end()
                        ->arrayNode('load_path')
                            ->info('Additional load paths')
                            ->scalarPrototype()
                                ->end()
                        ->end()
                        ->booleanNode('quiet')
                            ->info('Don\'t print warnings.')
                        ->end()
                        ->booleanNode('quiet_deps')
                            ->info(' Don\'t print compiler warnings from dependencies.')
                        ->end()
                        ->booleanNode('stop_on_error')
                            ->info('Don\'t compile more files once an error is encountered.')
                        ->end()
                        ->booleanNode('trace')
                            ->info('Print full Dart stack traces for exceptions.')
                        ->end()
                    ->end()
                ->end()
                ->booleanNode('embed_sourcemap')
                    ->setDeprecated('symfonycast/sass-bundle', '0.4', 'Option "%node%" at "%path%" is deprecated. Use "sass_options.embed_source_map" instead".')
                    ->defaultNull()
                ->end()
            ->end()
        ;

        return $treeBuilder;
    }
}
