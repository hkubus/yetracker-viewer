import { defineMiddleware } from 'astro:middleware';
import { STATUS_CODES } from 'node:http';
import { ERROR_STATUS_HEADER, isSecureRequest, PAGE_CACHE_CONTROL, publicApiOrigin } from './config';

const apiSources = publicApiOrigin ? [publicApiOrigin] : [];

/**
 * Scripts are same-origin files only: processed `<script>` modules are emitted as `/_astro/*.js` (never inlined, see
 * `astro.config.mjs`), and there are no inline scripts or event-handler attributes. Styles still need
 * 'unsafe-inline': `<Font />` emits an inline `<style>`, small stylesheets are inlined, and components set per-era
 * colors through `style` attributes. Covers, audio and search requests go to the API origin.
 */
function contentSecurityPolicy(secure: boolean, dev: boolean): string {
  const directives: [string, ...string[]][] = [
    ['default-src', "'self'"],
    ['base-uri', "'none'"],
    ['form-action', "'self'"],
    ['object-src', "'none'"],
    ['frame-ancestors', "'none'"],
    // The dev server injects an inline toolbar bootstrap and talks to Vite over a websocket.
    ['script-src', "'self'", ...(dev ? ["'unsafe-inline'"] : [])],
    ['style-src', "'self'", "'unsafe-inline'"],
    ['img-src', "'self'", 'data:', ...apiSources],
    ['media-src', "'self'", ...apiSources],
    ['connect-src', "'self'", ...apiSources, ...(dev ? ['ws:', 'wss:'] : [])],
    ['font-src', "'self'"],
    ['manifest-src', "'self'"],
  ];
  if (secure) directives.push(['upgrade-insecure-requests']);
  return directives.map((directive) => directive.join(' ')).join('; ');
}

function appendVary(headers: Headers, value: string): void {
  const current = headers.get('Vary');
  if (!current) {
    headers.set('Vary', value);
    return;
  }
  const tokens = current.split(',').map((token) => token.trim().toLowerCase());
  if (!tokens.includes('*') && !tokens.includes(value.toLowerCase())) headers.set('Vary', `${current}, ${value}`);
}

/**
 * The response with mutable headers (those of `Response.redirect()` are immutable), the status requested by an error
 * page through `ERROR_STATUS_HEADER` applied, and the standard reason phrase of its status: a page that sets
 * `Astro.response.status` keeps Astro's default "OK" otherwise, which would be sent as "404 OK".
 */
function prepare(response: Response): Response {
  const requestedStatus = Number(response.headers.get(ERROR_STATUS_HEADER));
  const status = requestedStatus >= 400 && requestedStatus <= 599 ? requestedStatus : response.status;
  const statusText = STATUS_CODES[status] ?? '';
  let { headers } = response;
  try {
    headers.delete(ERROR_STATUS_HEADER);
  } catch {
    headers = new Headers(headers);
    headers.delete(ERROR_STATUS_HEADER);
  }
  // An empty reason phrase is fine: Node sends the standard one.
  const wrongReason = response.statusText !== '' && response.statusText !== statusText;
  if (headers === response.headers && status === response.status && !wrongReason) return response;
  return new Response(response.body, { status, statusText, headers });
}

export const onRequest = defineMiddleware(async (context, next) => {
  const response = prepare(await next());
  const { headers } = response;
  headers.set('X-Content-Type-Options', 'nosniff');
  const secure = isSecureRequest(context.request);

  headers.set('Content-Security-Policy', contentSecurityPolicy(secure, import.meta.env.DEV));
  headers.set('X-Frame-Options', 'DENY');
  headers.set('Referrer-Policy', 'strict-origin-when-cross-origin');
  headers.set('Permissions-Policy', 'camera=(), geolocation=(), microphone=()');
  headers.set('Cross-Origin-Opener-Policy', 'same-origin');
  // Only for requests that really arrived over HTTPS (a TLS socket, or a trusted proxy saying so). No `preload` and
  // no `includeSubDomains`: those commit other hosts of the domain to HTTPS, which this app cannot know about.
  if (secure) headers.set('Strict-Transport-Security', 'max-age=63072000');
  // Bodies are compressed by server.mjs depending on Accept-Encoding.
  appendVary(headers, 'Accept-Encoding');

  // Only successful pages are cacheable, and a page that rendered without some of its data opts out with
  // `noStore()` (src/config.ts). Redirects and error pages are never cached: an out-of-range page redirect or an
  // outage must not stick at the edge.
  if (response.status === 200) {
    const isHtml = (headers.get('Content-Type') ?? '').toLowerCase().startsWith('text/html');
    if (isHtml && !headers.has('Cache-Control')) headers.set('Cache-Control', PAGE_CACHE_CONTROL);
  } else if (response.status >= 300) {
    headers.set('Cache-Control', 'no-store');
  }
  return response;
});
