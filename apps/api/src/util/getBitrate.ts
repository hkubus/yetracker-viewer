import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const run = promisify(execFile);

export async function getBitrate(path: string) {
  const info = await run('ffprobe', [
    '-v',
    'quiet',
    '-select_streams',
    'a:0',
    '-show_entries',
    'stream=bit_rate',
    '-of',
    'default=noprint_wrappers=1:nokey=1',
    path,
  ]);
  return parseInt(info.stdout, 10);
}
