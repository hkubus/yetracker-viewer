import { createHash } from 'node:crypto';

const coverVersionCache = new Map<string, string>();
const MAX_COVER_VERSIONS = 1000;

export function getCoverVersion(imageUrl: string | null | undefined) {
  const key = imageUrl ?? '';
  const cached = coverVersionCache.get(key);
  if (cached !== undefined) {
    // Refresh recency for LRU.
    coverVersionCache.delete(key);
    coverVersionCache.set(key, cached);
    return cached;
  }
  const version = createHash('sha1').update(key).digest('hex').slice(0, 12);
  coverVersionCache.set(key, version);
  if (coverVersionCache.size > MAX_COVER_VERSIONS) {
    const oldest = coverVersionCache.keys().next();
    if (!oldest.done) coverVersionCache.delete(oldest.value);
  }
  return version;
}
