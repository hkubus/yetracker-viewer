import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const run = promisify(execFile);
const durationCache = new Map<string, Promise<number | null>>();

export async function getDuration(path: string) {
  const cached = durationCache.get(path);
  if (cached) return cached;

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
  return result;
}
