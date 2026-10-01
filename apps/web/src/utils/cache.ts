/**
 * A loader result kept in memory for a short while: SSR data that every visitor sees the same way (the era list, the
 * catalog status) is fetched once per period per process instead of once per page view.
 */

export interface Cached<T> {
  data: T;
  /** The refresh failed and this is an older copy: the page should stay out of shared caches. */
  stale: boolean;
}

export interface CacheOptions {
  /** A copy younger than this is served without asking `load`. */
  freshMs: number;
  /** When a refresh fails, a copy up to this old is served (`stale: true`) instead of the error. */
  staleMs: number;
  now?: () => number;
}

/**
 * Wraps `load` in a cache: a fresh copy is served as is; otherwise one refresh runs (concurrent callers share it) and
 * its result is served. Failures are never cached: without a copy young enough to fall back on, the error is thrown.
 */
export function cachedLoader<T>(
  load: () => Promise<T>,
  { freshMs, staleMs, now = Date.now }: CacheOptions,
): () => Promise<Cached<T>> {
  let cached: { data: T; fetchedAt: number } | null = null;
  let pending: Promise<T> | null = null;
  return async () => {
    const startedAt = now();
    if (cached && startedAt - cached.fetchedAt < freshMs) return { data: cached.data, stale: false };
    try {
      pending ??= load()
        .then((data) => {
          cached = { data, fetchedAt: now() };
          return data;
        })
        .finally(() => {
          pending = null;
        });
      return { data: await pending, stale: false };
    } catch (error) {
      if (cached && startedAt - cached.fetchedAt < staleMs) return { data: cached.data, stale: true };
      throw error;
    }
  };
}

/** Freezes `value` and everything reachable from it: cached data is shared by every request that reads it. */
export function deepFreeze<T>(value: T): T {
  if (typeof value === 'object' && value !== null && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const child of Object.values(value)) deepFreeze(child);
  }
  return value;
}
