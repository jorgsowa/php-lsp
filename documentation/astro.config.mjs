// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// https://astro.build/config
export default defineConfig({
	// GitHub Pages project site: served at https://jorgsowa.github.io/php-lsp/
	site: 'https://jorgsowa.github.io',
	base: '/php-lsp',
	integrations: [
		starlight({
			title: 'php-lsp',
			description: 'A high-performance PHP language server written in Rust.',
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/jorgsowa/php-lsp' }],
			sidebar: [
				{ label: 'Getting Started', slug: 'getting-started' },
				{ label: 'Features', slug: 'features' },
				{ label: 'Editors & AI Clients', slug: 'editors' },
				{ label: 'Configuration', slug: 'configuration' },
				{ label: 'Architecture', slug: 'architecture' },
			],
		}),
	],
});
