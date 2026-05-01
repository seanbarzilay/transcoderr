import { defineConfig } from 'astro/config';
import tailwind from '@astrojs/tailwind';
import icon from 'astro-icon';

export default defineConfig({
  site: 'https://seanbarzilay.github.io',
  base: '/transcoderr',
  integrations: [tailwind(), icon()],
  build: { assets: '_astro' },
});
