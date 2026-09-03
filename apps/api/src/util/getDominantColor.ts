import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const run = promisify(execFile);

/**
 * Average color of the right-hand strip of an image file, sampled with
 * ffmpeg so the bulky canvas native library never has to load.
 */
export async function getDominantColor(path: string): Promise<[number, number, number]> {
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
  return [stdout[0], stdout[1], stdout[2]];
}
