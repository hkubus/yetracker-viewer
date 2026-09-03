import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const run = promisify(execFile);

export async function getBitrate(path: string): Promise<number | null> {
  let stdout: string;
  try {
    ({ stdout } = await run(
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
    ));
  } catch {
    // No audio stream (or unreadable file): ffprobe exits non-zero.
    return null;
  }
  const bitrate = Number.parseInt(stdout.trim(), 10);
  return Number.isFinite(bitrate) && bitrate > 0 ? bitrate : null;
}
