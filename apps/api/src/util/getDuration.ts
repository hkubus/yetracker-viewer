import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const run = promisify(execFile);
// Bounded LRU: every probed path used to stay in this Map forever,
// leaking a promise (and its closures) per unique file.
const MAX_CACHED_DURATIONS = 500;
const durationCache = new Map<string, Promise<number | null>>();

export async function getDuration(path: string) {
  const cached = durationCache.get(path);
  if (cached) {
    // Refresh recency.
    durationCache.delete(path);
    durationCache.set(path, cached);
    return cached;
  }

  const result = run(
    'ffprobe',
    ['-v', 'error', '-show_entries', 'format=duration', '-of', 'default=noprint_wrappers=1:nokey=1', path],
    { timeout: 10_000 },
  )
    .then(({ stdout }) => {
      const duration = Number.parseFloat(stdout);
      return Number.isFinite(duration) && duration > 0 ? duration : null;
    })
    .catch((error) => {
      durationCache.delete(path);
      throw error;
    });
  durationCache.set(path, result);
  while (durationCache.size > MAX_CACHED_DURATIONS) {
    // Evict the least recently used entry (Maps iterate in insertion order).
    const oldest = durationCache.keys().next();
    if (oldest.done) break;
    durationCache.delete(oldest.value);
  }
  return result;
}
