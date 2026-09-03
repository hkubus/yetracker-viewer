import { createHash } from 'node:crypto';

const coverVersionCache = new Map<string, string>();

export function getCoverVersion(imageUrl: string | null | undefined) {
  const key = imageUrl ?? '';
  const cached = coverVersionCache.get(key);
  if (cached !== undefined) return cached;
  const version = createHash('sha1').update(key).digest('hex').slice(0, 12);
  coverVersionCache.set(key, version);
  return version;
}
