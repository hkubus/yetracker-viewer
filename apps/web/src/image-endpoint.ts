import type { APIRoute } from 'astro';

/**
 * Stands in for Astro's image endpoint (`/_image`), which an on-demand build always registers (see `image.endpoint` in
 * `astro.config.mjs`). The site has no images to optimize (covers come from the API, the image service is
 * passthrough), so the route renders the 404 page like any unknown path, and Astro's image-processing code stays out
 * of the build.
 */
export const prerender = false;

export const GET: APIRoute = ({ rewrite }) => rewrite('/404');
