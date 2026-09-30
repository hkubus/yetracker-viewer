import type { APIRoute } from 'astro';
import { originDependentCacheControl, requestOrigin } from '../config';

export const GET: APIRoute = ({ request }) => {
  const body = [
    'User-agent: *',
    // Search states of the home page, and search, category and sort variants of the era pages, are endless
    // combinations of the same songs. Plain era pages and their `?page=N` continuations stay crawlable.
    'Disallow: /?',
    'Disallow: /*?q=',
    'Disallow: /*&q=',
    'Disallow: /*?category=',
    'Disallow: /*&category=',
    'Disallow: /*?sort=',
    'Disallow: /*&sort=',
    'Allow: /',
    '',
    `Sitemap: ${requestOrigin(request)}/sitemap.xml`,
    '',
  ].join('\n');
  return new Response(body, {
    headers: {
      'Content-Type': 'text/plain; charset=utf-8',
      'Cache-Control': originDependentCacheControl('public, max-age=3600', 3600),
    },
  });
};
