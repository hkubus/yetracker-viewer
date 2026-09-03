// @ts-check

import node from '@astrojs/node';
import { defineConfig } from 'astro/config';

import icon from 'astro-icon';

// Single source of truth for host/port precedence:
// WEB_HOST/WEB_PORT first, then generic HOST/PORT, then defaults.
const host = process.env.WEB_HOST ?? process.env.HOST ?? '127.0.0.1';
const port = Number(process.env.WEB_PORT ?? process.env.PORT ?? 4321);

// https://astro.build/config
export default defineConfig({
  envDir: '../..',
  adapter: node({
    mode: 'standalone',
  }),
  output: 'server',
  server: {
    host,
    port,
  },
  integrations: [icon()],
});
