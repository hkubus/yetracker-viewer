/**
 * Server-only: API data that every page view sees the same way, kept in memory per process (see `cache.ts`) instead
 * of being fetched for each render: the era list (the home page, the era pages' previous/next links, the sitemap),
 * the catalog status and the recently leaked songs (the home page). The API lets clients cache these responses for a
 * minute, so these periods add no staleness a browser couldn't have.
 */
import { ApiServerError, fetchJson } from '../config';
import { cachedLoader, deepFreeze } from './cache';

/** When the API fails, a copy up to this old is still served (and the page kept out of shared caches). */
const STALE_MS = 10 * 60_000;

export const RECENT_LEAKS_LIMIT = 8;

/** A cached API response body, frozen: every request reads the same objects. */
function sharedJson(path: string, freshMs: number, validate?: (data: unknown) => void) {
  return cachedLoader(
    async () => {
      const { data } = await fetchJson<unknown>(path);
      validate?.(data);
      return deepFreeze(data);
    },
    { freshMs, staleMs: STALE_MS },
  );
}

/** `/eras`: every era, in catalog order. */
export const loadEras = sharedJson('/eras', 60_000, (data) => {
  if (!Array.isArray(data)) throw new ApiServerError('API response for /eras is not a list', '/eras');
});

/** `/status`. */
export const loadStatus = sharedJson('/status', 30_000);

/** The newest playable songs (`/songs` search mode). */
export const loadRecentLeaks = sharedJson(`/songs?playable=true&sort=leak-newest&limit=${RECENT_LEAKS_LIMIT}`, 30_000);

export interface EraSummary {
  id: number;
  name: string;
  /** `null` when the API did not say. */
  songsCount: number | null;
}

function toSummaries(data: unknown): EraSummary[] {
  const eras: EraSummary[] = [];
  for (const item of data as unknown[]) {
    if (typeof item !== 'object' || item === null) continue;
    const { id, name, songsCount } = item as Record<string, unknown>;
    if (typeof id !== 'number' || typeof name !== 'string') continue;
    eras.push({ id, name, songsCount: typeof songsCount === 'number' ? songsCount : null });
  }
  return eras;
}

/**
 * The eras in catalog order (id, name, songs count). `stale: true` when the API failed and an older copy (up to ten
 * minutes) is returned; without one the API error is thrown.
 */
export async function loadEraSummaries(): Promise<{ eras: EraSummary[]; stale: boolean }> {
  const { data, stale } = await loadEras();
  return { eras: toSummaries(data), stale };
}
