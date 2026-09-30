import type { APIRoute } from 'astro';
import { ERA_PAGE_SIZE, OUTAGE_RETRY_AFTER_SECONDS, originDependentCacheControl, requestOrigin } from '../config';
import { loadEraSummaries } from '../utils/era-list';
import { eraPageHref, maxReachablePage, pageCountFor } from '../utils/era-page';

const XML_ESCAPES: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&apos;' };

function escapeXml(value: string): string {
  return value.replace(/[&<>"']/g, (char) => XML_ESCAPES[char] ?? char);
}

/** The home page, every era, and every further page of each era's song list. */
export const GET: APIRoute = async ({ request }) => {
  let eras: Awaited<ReturnType<typeof loadEraSummaries>>['eras'];
  try {
    ({ eras } = await loadEraSummaries());
  } catch (error) {
    // An empty sitemap would tell crawlers the pages are gone: answer "try later" instead.
    console.error('[sitemap] /eras request failed:', error);
    return new Response('The sitemap is temporarily unavailable.\n', {
      status: 503,
      headers: { 'Content-Type': 'text/plain; charset=utf-8', 'Retry-After': String(OUTAGE_RETRY_AFTER_SECONDS) },
    });
  }

  const origin = requestOrigin(request);
  const paths = ['/'];
  const lastPage = maxReachablePage(ERA_PAGE_SIZE);
  for (const era of eras) {
    const pages = era.songsCount === null ? 1 : Math.min(pageCountFor(era.songsCount, ERA_PAGE_SIZE), lastPage);
    for (let page = 1; page <= pages; page += 1) paths.push(eraPageHref(era.id, {}, page));
  }
  const body = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">',
    ...paths.map((path) => `  <url><loc>${escapeXml(`${origin}${path}`)}</loc></url>`),
    '</urlset>',
    '',
  ].join('\n');
  return new Response(body, {
    headers: {
      'Content-Type': 'application/xml; charset=utf-8',
      'Cache-Control': originDependentCacheControl('public, max-age=3600', 3600),
    },
  });
};
