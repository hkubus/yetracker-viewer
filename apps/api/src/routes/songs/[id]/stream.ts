import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { stream } from 'hono/streaming';
import { storagePath } from '../../../config.ts';
import { filesTable, songsTable } from '../../../db/schema.ts';
import { db } from '../../../index.ts';
import { getBitrate } from '../../../util/getBitrate.ts';
import { transcode } from '../../../util/transcode.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      console.log('test');
      const id = c.req.param('id') as string;
      const quality = c.req.query('quality') as string;
      const songId = Number.parseInt(id, 10);

      if (Number.isNaN(songId)) {
        throw new HTTPException(400, { message: 'Invalid song id' });
      }

      const [song] = await db
        .select({
          id: songsTable.id,
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
        throw new HTTPException(500, { message: 'Could not find file for song' });
      }

      const { filename } = song;
      const path = join(storagePath, 'songs', filename);
      if (quality) {
        const qualityParsed = parseInt(quality, 10);
        if (Number.isNaN(qualityParsed) || qualityParsed < 8 || qualityParsed > 320) {
          throw new HTTPException(404, { message: 'Invalid quality for file' });
        }
        const fileBitrate = await getBitrate(path);
        console.log(fileBitrate);
        if (fileBitrate / 1000 < qualityParsed) {
          const data = await readFile(path);
          return c.body(data);
        }
        return stream(c, async (stream) => {
          const response = await transcode(path, `${qualityParsed}k`);
          // @ts-expect-error
          await stream.pipe(response);
        });
      }
      let mimetype = '';
      switch (filename.split('.').at(-1)) {
        case 'mp3':
          mimetype = 'audio/mpeg';
          break;
        case 'opus':
          mimetype = 'audio/opus';
          break;
        case 'flac':
          mimetype = 'audio/flac';
          break;
        case 'wav':
          mimetype = 'audio/wav';
          break;
        case 'aif':
          mimetype = 'audio/aiff';
          break;
        default:
          mimetype = 'application/octet-stream';
      }

      c.header('Content-Type', mimetype);
      const data = await readFile(path);
      return c.body(data);
      // return res.send(stream);
    },
  },
};
