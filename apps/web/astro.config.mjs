// @ts-check

import node from '@astrojs/node';
import { defineConfig } from 'astro/config';

import icon from 'astro-icon';

// https://astro.build/config
export default defineConfig({
  envDir: '../..',
  adapter: node({
    mode: 'standalone',
  }),
  output: 'server',
  server: {
    host: process.env.WEB_HOST ?? process.env.HOST ?? '127.0.0.1',
    port: Number(process.env.WEB_PORT ?? process.env.PORT ?? 4321),
  },
  integrations: [icon()],
});
