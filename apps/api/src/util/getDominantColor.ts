import { execFile } from 'node:child_process';
import { stat } from 'node:fs/promises';
import { promisify } from 'node:util';

const run = promisify(execFile);

const colorCache = new Map<string, { mtimeMs: number; result: Promise<[number, number, number]> }>();
const MAX_CACHED_COLORS = 500;

/**
 * Average color of the right-hand strip of an image file, sampled with
 * ffmpeg so the bulky canvas native library never has to load.
 */
export async function getDominantColor(path: string): Promise<[number, number, number]> {
  let mtimeMs = -1;
  try {
    mtimeMs = (await stat(path)).mtimeMs;
  } catch {
    // Fall through and let ffmpeg surface the missing-file error.
  }
  const cached = colorCache.get(path);
  if (cached && cached.mtimeMs === mtimeMs) {
    colorCache.delete(path);
    colorCache.set(path, cached);
    return cached.result;
  }
  const result = (async () => {
    const { stdout } = (await run(
      'ffmpeg',
      [
        '-v',
        'error',
        '-i',
        path,
        '-vf',
        'crop=iw*0.2:ih:iw*0.8:0,scale=1:1:flags=area',
        '-pix_fmt',
        'rgb24',
        '-f',
        'rawvideo',
        'pipe:1',
      ],
      { timeout: 30_000, maxBuffer: 64, encoding: 'buffer' },
    )) as unknown as { stdout: Buffer };
    if (stdout.length < 3) throw new Error(`Could not sample color from ${path}`);
    return [stdout[0], stdout[1], stdout[2]] as [number, number, number];
  })();
  result.catch(() => {
    colorCache.delete(path);
  });
  colorCache.set(path, { mtimeMs, result });
  while (colorCache.size > MAX_CACHED_COLORS) {
    const oldest = colorCache.keys().next();
    if (oldest.done) break;
    colorCache.delete(oldest.value);
  }
  return result;
}
