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

/**
 * The faces of the three weights, each split into the latin and latin-ext ranges. Every face carries its `subset`,
 * which `<Font preload>` filters on (see `localWithSubsets`).
 *
 * @param {number} weight
 */
const fontVariants = (weight) =>
  /** @type {const} */ ([
    { weight, style: 'normal', unicodeRange: LATIN, src: [fontFile('latin', weight)], subset: 'latin' },
    { weight, style: 'normal', unicodeRange: LATIN_EXT, src: [fontFile('latin-ext', weight)], subset: 'latin-ext' },
  ]);

/**
 * `fontProviders.local()` plus each variant's `subset` as the face's `meta.subset`, the field `<Font preload>` filters
 * on: the layout preloads the latin faces only. Latin-ext characters are rare in the catalog, and its three files
 * (40 KB) otherwise compete with the page itself on every first visit.
 *
 * @returns {import('astro').FontProvider<{ variants: ReturnType<typeof fontVariants>[number][] }>}
 */
function localWithSubsets() {
  const local = fontProviders.local();
  return {
    ...local,
    name: 'local-subsets',
    async resolveFont(options) {
      const resolved = await local.resolveFont(/** @type {any} */ (options));
      if (!resolved) return resolved;
      const variants = options.options?.variants ?? [];
      // The local provider returns one face per variant, in order.
      return {
        fonts: resolved.fonts.map((font, index) => ({
          ...font,
          meta: { ...font.meta, subset: variants[index]?.subset },
        })),
      };
    },
  };
}

// https://astro.build/config
export default defineConfig({
  adapter: node({
    mode: 'standalone',
  }),
  output: 'server',
  // `astro dev`/`astro preview` listen where server.mjs would: WEB_HOST/WEB_PORT, then HOST/PORT, then
  // 127.0.0.1:4321, blank values counting as unset. An invalid port stops every command with a clear message.
  server: readServerEnv(process.env),
  // Covers are served by the API; nothing goes through Astro's image pipeline, so sharp is not needed. Astro cannot
  // leave out the `/_image` endpoint of an on-demand build: this one renders the 404 page (src/image-endpoint.ts).
  image: {
    service: passthroughImageService(),
    endpoint: { route: '/_image', entrypoint: './src/image-endpoint.ts' },
  },
  // <ClientRouter /> would otherwise prefetch every link on hover. Links opt in with `data-astro-prefetch` (era cards,
  // previous/next page): the page is fetched on hover or focus, so the click doesn't wait for it.
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
  // The layout preloads the latin faces, so a first visit on a fast connection gets them in time too.
  fonts: [
    {
      provider: localWithSubsets(),
      name: 'Be Vietnam Pro',
      cssVariable: '--font-body',
      display: 'optional',
      fallbacks: ['system-ui', 'sans-serif'],
      options: {
        variants: [...fontVariants(400), ...fontVariants(600), ...fontVariants(700)],
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
