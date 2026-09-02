import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import { extname } from 'node:path';
import { Readable } from 'node:stream';
import { eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { stream } from 'hono/streaming';
import { db } from '../../../db/client.ts';
import { filesTable, songsTable } from '../../../db/schema.ts';
import { positiveInteger } from '../../../util/request.ts';
import { storedSongPath } from '../../../util/storedFile.ts';

function downloadName(name: string | null, id: number, extension: string) {
  const safeName = (name ?? `song-${id}`)
    .replaceAll(/[\p{Cc}<>:"/\\|?*]/gu, ' ')
    .replaceAll(/\s+/g, ' ')
    .trim()
    .slice(0, 120);
  return `${safeName || `song-${id}`}${extension}`;
}

function encodeHeaderFilename(filename: string) {
  return encodeURIComponent(filename).replaceAll(
    /[!'()*]/g,
    (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

export const routes = {
  get: {
    handler: async (c: Context) => {
      const songId = positiveInteger(c.req.param('id'), 'song id');

      const [song] = await db
        .select({
          name: songsTable.name,
          filename: filesTable.filename,
        })
        .from(songsTable)
        .leftJoin(filesTable, eq(songsTable.url, filesTable.url))
        .where(eq(songsTable.id, songId))
        .limit(1);

      if (!song) {
        throw new HTTPException(404, { message: 'Song not found' });
      }
      if (!song.filename) {
        throw new HTTPException(404, { message: 'Song file not found' });
      }

      const path = storedSongPath(song.filename);
      try {
        const file = await stat(path);
        if (!file.isFile() || file.size === 0) {
          throw new HTTPException(404, { message: 'Song file not found' });
        }
        c.header('Content-Length', String(file.size));
      } catch (error) {
        if (error instanceof HTTPException) throw error;
        throw new HTTPException(404, { message: 'Song file not found' });
      }

      const filename = downloadName(song.name, songId, extname(song.filename));
      c.header('Content-Type', 'application/octet-stream');
      c.header(
        'Content-Disposition',
        `attachment; filename="song-${songId}${extname(song.filename)}"; filename*=UTF-8''${encodeHeaderFilename(filename)}`,
      );

      return stream(c, async (output) => {
        const input = createReadStream(path);
        await output.pipe(Readable.toWeb(input) as ReadableStream);
      });
    },
  },
};
