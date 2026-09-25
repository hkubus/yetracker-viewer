import { defineMiddleware } from 'astro:middleware';

// Inline <style> / <script is:inline> blocks require 'unsafe-inline'.
// frame-ancestors 'self' is the CSP successor of X-Frame-Options.
const csp = [
  "default-src 'self'",
  "base-uri 'self'",
  "form-action 'self'",
  "object-src 'none'",
  "frame-ancestors 'self'",
  "img-src 'self' data: blob: http: https:",
  "media-src 'self' data: blob: http: https:",
  "style-src 'self' 'unsafe-inline' https://fonts.googleapis.com",
  "font-src 'self' data: https://fonts.gstatic.com",
  "script-src 'self' 'unsafe-inline'",
  "connect-src 'self' http: https:",
].join('; ');

export const onRequest = defineMiddleware(async (context, next) => {
  const response = await next();
  response.headers.set('Content-Security-Policy', csp);
  response.headers.set('Permissions-Policy', 'camera=(), geolocation=(), microphone=()');
  response.headers.set('Referrer-Policy', 'strict-origin-when-cross-origin');
  if (new URL(context.request.url).protocol === 'https:') {
    response.headers.set('Strict-Transport-Security', 'max-age=63072000; includeSubDomains; preload');
  }
  response.headers.set('X-Content-Type-Options', 'nosniff');
  response.headers.set('X-Frame-Options', 'DENY');
  const pathname = new URL(context.request.url).pathname;
  // Pages that could not load their data set Cache-Control themselves (see
  // `noStore()` in src/config.ts) so a blip is never cached as an empty site.
  if (
    (pathname.startsWith('/eras/') || pathname === '/') &&
    !response.headers.has('Cache-Control') &&
    response.status < 400
  ) {
    response.headers.set('Cache-Control', 'public, max-age=60, s-maxage=300, stale-while-revalidate=600');
  }
  return response;
});
