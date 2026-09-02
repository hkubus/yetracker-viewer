import { basename, join } from 'node:path';
import { HTTPException } from 'hono/http-exception';
import { songsPath } from '../config.ts';

export function storedSongPath(filename: string) {
  if (filename.length === 0 || filename.length > 255 || basename(filename) !== filename || filename.includes('\0')) {
    throw new HTTPException(404, { message: 'Song file not found' });
  }
  return join(songsPath, filename);
}
