/**
 * Server-only: the list of eras (id, name, songs count) cached in memory. Era pages only need it for their
 * previous/next links and the sitemap for its URLs, so one `/eras` request per minute per process is plenty (the
 * API itself lets `/eras` be cached for a minute).
 */
import { ApiServerError, fetchJson } from '../config';

export interface EraSummary {
  id: number;
  name: string;
  /** `null` when the API did not say. */
  songsCount: number | null;
}

const FRESH_MS = 60_000;
/** When the API fails, a copy up to this old is still good enough for navigation links. */
const STALE_MS = 10 * 60_000;

let cached: { eras: EraSummary[]; fetchedAt: number } | null = null;
let pending: Promise<EraSummary[]> | null = null;

function toSummaries(data: unknown): EraSummary[] {
  if (!Array.isArray(data)) throw new ApiServerError('API response for /eras is not a list', '/eras');
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
 * The eras in catalog order (the API's order). Serves a copy younger than a minute, else refreshes it (concurrent
 * callers share one request). If the refresh fails, a copy up to ten minutes old is returned with `stale: true`;
 * without one the API error is thrown.
 */
export async function loadEraSummaries(): Promise<{ eras: EraSummary[]; stale: boolean }> {
  const startedAt = Date.now();
  if (cached && startedAt - cached.fetchedAt < FRESH_MS) return { eras: cached.eras, stale: false };
  try {
    pending ??= fetchJson<unknown>('/eras')
      .then(({ data }) => toSummaries(data))
      .finally(() => {
        pending = null;
      });
    const eras = await pending;
    cached = { eras, fetchedAt: Date.now() };
    return { eras, stale: false };
  } catch (error) {
    if (cached && startedAt - cached.fetchedAt < STALE_MS) return { eras: cached.eras, stale: true };
    throw error;
  }
}
