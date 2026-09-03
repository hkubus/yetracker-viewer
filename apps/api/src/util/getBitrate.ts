import { execFile } from 'node:child_process';
import { stat } from 'node:fs/promises';
import { promisify } from 'node:util';

const run = promisify(execFile);

const MAX_CACHED_BITRATES = 500;
const bitrateCache = new Map<string, { mtimeMs: number; result: Promise<number | null> }>();

export async function getBitrate(path: string, knownMtimeMs?: number): Promise<number | null> {
  let mtimeMs = knownMtimeMs;
  if (mtimeMs === undefined) {
    try {
      mtimeMs = (await stat(path)).mtimeMs;
    } catch {
      return null;
    }
  }
  const cached = bitrateCache.get(path);
  if (cached && cached.mtimeMs === mtimeMs) {
    bitrateCache.delete(path);
    bitrateCache.set(path, cached);
    return cached.result;
  }
  const entry = {
    mtimeMs,
    result: run(
      'ffprobe',
      [
        '-v',
        'quiet',
        '-select_streams',
        'a:0',
        '-show_entries',
        'stream=bit_rate',
        '-of',
        'default=noprint_wrappers=1:nokey=1',
        path,
      ],
      { timeout: 10_000, maxBuffer: 1024 * 1024 },
    )
      .then(({ stdout }) => {
        const bitrate = Number.parseInt(stdout.trim(), 10);
        return Number.isFinite(bitrate) && bitrate > 0 ? bitrate : null;
      })
      .catch(() => {
        // No audio stream (or unreadable file): ffprobe exits non-zero.
        bitrateCache.delete(path);
        return null;
      }),
  };
  bitrateCache.set(path, entry);
  while (bitrateCache.size > MAX_CACHED_BITRATES) {
    const oldest = bitrateCache.keys().next();
    if (oldest.done) break;
    bitrateCache.delete(oldest.value);
  }
  return entry.result;
}
