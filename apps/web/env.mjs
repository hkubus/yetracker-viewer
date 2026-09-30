// @ts-check
/**
 * Runtime configuration of the web server. Everything is read from the environment when the process starts, never
 * baked in at build time, so one build can be deployed anywhere. `server.mjs` validates it before it loads the app
 * (a bad value stops the process with a clear message); `src/config.ts` reads the same values for SSR code, and
 * `astro.config.mjs` the listen address of the dev server.
 *
 * Plain JavaScript next to `server.mjs` on purpose: the production server imports it directly, without a build step,
 * so a deployment needs only `dist/`, `server.mjs` and this file (no `src/`, no `node_modules`). It must not import
 * any package: only `node:*` modules are available at runtime.
 */

/**
 * @typedef {Record<string, string | undefined>} EnvSource
 *
 * @typedef {object} RuntimeEnv
 * @property {string} publicApiBaseUrl Browser-facing API base URL without a trailing slash: absolute
 *   (`https://api.example.com`) or a same-origin path prefix (`/api`).
 * @property {string} internalApiBaseUrl Absolute API base URL used by server-side rendering, without a trailing slash.
 * @property {string | null} siteUrl Public origin of the site (`https://example.com`), used for canonical, sitemap and
 *   Open Graph URLs; `null` when unset (those URLs then come from the request).
 * @property {boolean} trustProxy Whether `X-Forwarded-Proto` / `X-Forwarded-Host` from a reverse proxy are trusted.
 *
 * @typedef {object} ServerEnv
 * @property {string} host Interface to listen on.
 * @property {number} port Port to listen on.
 */

export const DEFAULT_WEB_HOST = '127.0.0.1';
export const DEFAULT_WEB_PORT = 4321;
/** Development fallbacks, matching `.env.example`. Production requires explicit values. */
export const DEV_PUBLIC_API_URL = 'http://localhost:3000';
export const DEV_INTERNAL_API_URL = 'http://127.0.0.1:3000';

export class EnvError extends Error {
  /** @param {string[]} problems */
  constructor(problems) {
    super(`Invalid configuration:\n${problems.map((problem) => `  - ${problem}`).join('\n')}`);
    this.name = 'EnvError';
    /** @type {string[]} */
    this.problems = problems;
  }
}

/**
 * A trimmed variable; blank counts as unset so `PORT=` in an env file does not shadow a fallback.
 * @param {EnvSource} env
 * @param {string} name
 * @returns {string | undefined}
 */
function read(env, name) {
  const value = env[name]?.trim();
  return value ? value : undefined;
}

/**
 * The first set variable of `names`, with the name it came from (for error messages).
 * @param {EnvSource} env
 * @param {string[]} names
 * @returns {{ name: string, value: string } | undefined}
 */
function readFirst(env, names) {
  for (const name of names) {
    const value = read(env, name);
    if (value !== undefined) return { name, value };
  }
  return undefined;
}

const BOOLEANS = new Map([
  ['true', true],
  ['1', true],
  ['yes', true],
  ['on', true],
  ['false', false],
  ['0', false],
  ['no', false],
  ['off', false],
]);

/**
 * Parses a boolean variable: `true/false/1/0/yes/no/on/off` (any case). Unset → `fallback`.
 * @param {string | undefined} value
 * @param {boolean} fallback
 * @returns {boolean | undefined} `undefined` when the value is not a boolean.
 */
export function parseBoolean(value, fallback) {
  const trimmed = value?.trim();
  if (!trimmed) return fallback;
  return BOOLEANS.get(trimmed.toLowerCase());
}

/**
 * Parses a TCP port: decimal digits only, 1–65535.
 * @param {string} value
 * @returns {number | undefined}
 */
export function parsePort(value) {
  if (!/^\d{1,5}$/.test(value)) return undefined;
  const port = Number(value);
  return port >= 1 && port <= 65535 ? port : undefined;
}

/**
 * An absolute http(s) URL without credentials, query or fragment, normalized without a trailing slash.
 * @param {string} value
 * @returns {string | undefined}
 */
export function parseAbsoluteBaseUrl(value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    return undefined;
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return undefined;
  if (url.username || url.password || url.search || url.hash) return undefined;
  if (/[?#]/.test(value)) return undefined;
  return `${url.origin}${url.pathname}`.replace(/\/+$/, '');
}

/**
 * The browser-facing API URL: an absolute http(s) URL, or a same-origin path prefix such as `/api` (the site's own
 * pages live at `/`, so the API cannot share the root).
 * @param {string} value
 * @returns {string | undefined}
 */
export function parsePublicApiUrl(value) {
  if (value.startsWith('/')) {
    if (value.startsWith('//') || /[?#\\\s]/.test(value)) return undefined;
    const path = value.replace(/\/+$/, '');
    return path ? path : undefined;
  }
  return parseAbsoluteBaseUrl(value);
}

/**
 * The site's public origin (`https://example.com`). Paths are rejected: the site is always served from the root.
 * @param {string} value
 * @returns {string | undefined}
 */
export function parseSiteUrl(value) {
  const base = parseAbsoluteBaseUrl(value);
  if (base === undefined) return undefined;
  const url = new URL(base);
  return url.pathname === '/' ? url.origin : undefined;
}

/**
 * Reads and validates the API URLs, `SITE_URL` and `TRUST_PROXY`. Throws an `EnvError` listing every problem. Blank
 * values count as unset.
 *
 * - `PUBLIC_API_URL`: absolute http(s) URL or a same-origin path like `/api`. Required in production; development
 *   falls back to `http://localhost:3000`.
 * - `API_INTERNAL_URL`: absolute http(s) URL used by server-side rendering. Falls back to an absolute
 *   `PUBLIC_API_URL`; development also falls back to `http://127.0.0.1:3000`.
 * - `SITE_URL` (optional): the public origin, e.g. `https://yetracker.example`. Canonical and `og:url` links need it
 *   (they are left out without it). robots.txt, the sitemap and Open Graph images fall back to the origin of the
 *   request (`Host`, or the trusted proxy headers), and such responses are then kept out of shared caches.
 * - `TRUST_PROXY` (optional, default false): trust the reverse proxy's `X-Forwarded-Proto` (the client used HTTPS:
 *   HSTS and `upgrade-insecure-requests`) and `X-Forwarded-Host` (the public host when `SITE_URL` is unset). Only
 *   enable it behind a proxy that sets or strips these headers.
 *
 * @param {EnvSource} env
 * @param {{ production: boolean }} options
 * @returns {RuntimeEnv}
 */
export function readRuntimeEnv(env, { production }) {
  /** @type {string[]} */
  const problems = [];

  const rawPublic = read(env, 'PUBLIC_API_URL');
  let publicApiBaseUrl = DEV_PUBLIC_API_URL;
  if (rawPublic !== undefined) {
    const parsed = parsePublicApiUrl(rawPublic);
    if (parsed === undefined) {
      problems.push(
        `PUBLIC_API_URL must be an absolute http(s) URL (https://api.example.com) or a same-origin path (/api), got ${JSON.stringify(rawPublic)}.`,
      );
    } else {
      publicApiBaseUrl = parsed;
    }
  } else if (production) {
    problems.push('PUBLIC_API_URL is not set: set the API URL browsers should use (https://api.example.com or /api).');
  }

  const rawInternal = read(env, 'API_INTERNAL_URL');
  let internalApiBaseUrl = DEV_INTERNAL_API_URL;
  if (rawInternal !== undefined) {
    const parsed = parseAbsoluteBaseUrl(rawInternal);
    if (parsed === undefined) {
      problems.push(
        `API_INTERNAL_URL must be an absolute http(s) URL such as http://127.0.0.1:3000, got ${JSON.stringify(rawInternal)}.`,
      );
    } else {
      internalApiBaseUrl = parsed;
    }
  } else if (rawPublic !== undefined && /^https?:\/\//i.test(publicApiBaseUrl)) {
    internalApiBaseUrl = publicApiBaseUrl;
  } else if (production) {
    problems.push(
      'API_INTERNAL_URL is not set: server-side rendering needs an absolute API URL such as http://127.0.0.1:3000 (it may only be omitted when PUBLIC_API_URL is absolute).',
    );
  }

  const rawSite = read(env, 'SITE_URL');
  let siteUrl = null;
  if (rawSite !== undefined) {
    const parsed = parseSiteUrl(rawSite);
    if (parsed === undefined) {
      problems.push(`SITE_URL must be an http(s) origin such as https://example.com, got ${JSON.stringify(rawSite)}.`);
    } else {
      siteUrl = parsed;
    }
  }

  const trustProxy = parseBoolean(env.TRUST_PROXY, false);
  if (trustProxy === undefined) {
    problems.push(
      `TRUST_PROXY must be true or false (also 1/0, yes/no, on/off), got ${JSON.stringify(env.TRUST_PROXY)}.`,
    );
  }

  if (problems.length > 0) throw new EnvError(problems);
  return { publicApiBaseUrl, internalApiBaseUrl, siteUrl, trustProxy: trustProxy === true };
}

const LOOPBACK_HOSTNAME = /^(?:localhost|.+\.localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[::1?\])$/i;

/**
 * Valid settings that a public deployment probably doesn't want, as log lines for the production server's startup.
 * @param {Pick<RuntimeEnv, 'siteUrl'>} runtimeEnv
 * @returns {string[]}
 */
export function runtimeEnvWarnings({ siteUrl }) {
  if (siteUrl === null) {
    return [
      "SITE_URL is not set: pages have no canonical or og:url links, and robots.txt, the sitemap and Open Graph images use the request's origin (and are not cached by shared caches). Set it to the public origin, such as https://yetracker.example.",
    ];
  }
  if (LOOPBACK_HOSTNAME.test(new URL(siteUrl).hostname)) {
    return [
      `SITE_URL is ${siteUrl}: canonical, sitemap and Open Graph URLs point at this machine, not at the public site.`,
    ];
  }
  return [];
}

/**
 * Reads the listen address: `WEB_HOST`/`WEB_PORT` take precedence over the generic `HOST`/`PORT`, then the
 * defaults (`127.0.0.1:4321`). Throws an `EnvError` for an invalid port.
 * @param {EnvSource} env
 * @returns {ServerEnv}
 */
export function readServerEnv(env) {
  /** @type {string[]} */
  const problems = [];
  const host = readFirst(env, ['WEB_HOST', 'HOST'])?.value ?? DEFAULT_WEB_HOST;
  const rawPort = readFirst(env, ['WEB_PORT', 'PORT']);
  let port = DEFAULT_WEB_PORT;
  if (rawPort !== undefined) {
    const parsed = parsePort(rawPort.value);
    if (parsed === undefined) {
      problems.push(`${rawPort.name} must be a port number between 1 and 65535, got ${JSON.stringify(rawPort.value)}.`);
    } else {
      port = parsed;
    }
  }
  if (problems.length > 0) throw new EnvError(problems);
  return { host, port };
}
