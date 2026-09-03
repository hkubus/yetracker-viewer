import { execFile } from 'node:child_process';
import { stat } from 'node:fs/promises';
import { promisify } from 'node:util';

const run = promisify(execFile);
// Bounded LRU: every probed path used to stay in this Map forever,
// leaking a promise (and its closures) per unique file.
//
// Cache entries are keyed by path and validated against the file's mtime:
// if the file changed since the probe, the stale entry is discarded and the
// file is re-probed.
const MAX_CACHED_DURATIONS = 500;
const durationCache = new Map<string, { mtimeMs: number; result: Promise<number | null> }>();

async function mtimeOf(path: string): Promise<number | null> {
  try {
    return (await stat(path)).mtimeMs;
  } catch {
    return null;
  }
}

export async function getDuration(path: string, knownMtimeMs?: number) {
  const mtimeMs = knownMtimeMs ?? (await mtimeOf(path));
  const cached = durationCache.get(path);
  if (cached && mtimeMs !== null && cached.mtimeMs === mtimeMs) {
    // Refresh recency.
    durationCache.delete(path);
    durationCache.set(path, cached);
    return cached.result;
  }
  if (mtimeMs === null) durationCache.delete(path);

  const entry = {
    mtimeMs: mtimeMs ?? -1,
    result: run(
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
      }),
  };
  durationCache.set(path, entry);
  while (durationCache.size > MAX_CACHED_DURATIONS) {
    // Evict the least recently used entry (Maps iterate in insertion order).
    const oldest = durationCache.keys().next();
    if (oldest.done) break;
    durationCache.delete(oldest.value);
  }
  return entry.result;
}
