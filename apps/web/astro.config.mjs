// @ts-check

import { fileURLToPath } from 'node:url';
import node from '@astrojs/node';
import { defineConfig, fontProviders, passthroughImageService } from 'astro/config';
import { readServerEnv } from './env.mjs';

// The web app and the API share the repo-root .env.
const repoRoot = fileURLToPath(new URL('../..', import.meta.url));

// Self-hosted Be Vietnam Pro (latin + latin-ext, the ranges @fontsource ships). Browsers only download a face
// when the page actually uses that weight and a character from its range.
/** @type {[string, ...string[]]} */
const LATIN = [
  'U+0000-00FF',
  'U+0131',
  'U+0152-0153',
  'U+02BB-02BC',
  'U+02C6',
  'U+02DA',
  'U+02DC',
  'U+0304',
  'U+0308',
  'U+0329',
  'U+2000-206F',
  'U+20AC',
  'U+2122',
  'U+2191',
  'U+2193',
  'U+2212',
  'U+2215',
  'U+FEFF',
  'U+FFFD',
];
/** @type {[string, ...string[]]} */
const LATIN_EXT = [
  'U+0100-02BA',
  'U+02BD-02C5',
  'U+02C7-02CC',
  'U+02CE-02D7',
  'U+02DD-02FF',
  'U+0304',
  'U+0308',
  'U+0329',
  'U+1D00-1DBF',
  'U+1E00-1E9F',
  'U+1EF2-1EFF',
  'U+2020',
  'U+20A0-20AB',
  'U+20AD-20C0',
  'U+2113',
  'U+2C60-2C7F',
  'U+A720-A7FF',
];

/**
 * @param {'latin' | 'latin-ext'} subset
 * @param {number} weight
 */
const fontFile = (subset, weight) => `@fontsource/be-vietnam-pro/files/be-vietnam-pro-${subset}-${weight}-normal.woff2`;

// https://astro.build/config
export default defineConfig({
  adapter: node({
    mode: 'standalone',
  }),
  output: 'server',
  // `astro dev`/`astro preview` listen where server.mjs would: WEB_HOST/WEB_PORT, then HOST/PORT, then
  // 127.0.0.1:4321, blank values counting as unset. An invalid port stops every command with a clear message.
  server: readServerEnv(process.env),
  // Covers are served by the API; nothing goes through Astro's image pipeline, so sharp is not needed.
  image: {
    service: passthroughImageService(),
  },
  // <ClientRouter /> would otherwise prefetch every link on hover. Links opt in with `data-astro-prefetch`.
  prefetch: {
    prefetchAll: false,
  },
  build: {
    // Small stylesheets are inlined as <style>. The CSP needs style-src 'unsafe-inline' regardless: components set
    // per-era colors through `style` attributes, and <Font /> emits an inline <style> with the @font-face rules.
    inlineStylesheets: 'auto',
  },
  // Rendered by <Font cssVariable="--font-body" /> in the layout, with a metric-adjusted local fallback derived from
  // the font files. Only the weights the styles use (400, 600, 700). `optional`: a face that isn't there for the first
  // render is not swapped in for that page, so a late font never shifts the layout (cached faces render from the start).
  fonts: [
    {
      provider: fontProviders.local(),
      name: 'Be Vietnam Pro',
      cssVariable: '--font-body',
      display: 'optional',
      fallbacks: ['system-ui', 'sans-serif'],
      options: {
        variants: [
          { weight: 400, style: 'normal', unicodeRange: LATIN, src: [fontFile('latin', 400)] },
          { weight: 400, style: 'normal', unicodeRange: LATIN_EXT, src: [fontFile('latin-ext', 400)] },
          { weight: 600, style: 'normal', unicodeRange: LATIN, src: [fontFile('latin', 600)] },
          { weight: 600, style: 'normal', unicodeRange: LATIN_EXT, src: [fontFile('latin-ext', 600)] },
          { weight: 700, style: 'normal', unicodeRange: LATIN, src: [fontFile('latin', 700)] },
          { weight: 700, style: 'normal', unicodeRange: LATIN_EXT, src: [fontFile('latin-ext', 700)] },
        ],
      },
    },
  ],
  vite: {
    envDir: repoRoot,
    build: {
      // Astro inlines small processed <script> modules into the HTML; keep them as same-origin files so the CSP
      // can allow scripts from 'self' without 'unsafe-inline'. Other assets keep Vite's default limit.
      assetsInlineLimit: (filePath) => (filePath.endsWith('.js') ? false : undefined),
    },
  },
});
