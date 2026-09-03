import { stat } from 'node:fs/promises';
import { eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { db } from '../../../db/client.ts';
import { filesTable, songsTable } from '../../../db/schema.ts';
import { getDuration } from '../../../util/getDuration.ts';
import { positiveInteger } from '../../../util/request.ts';
import { storedSongPath } from '../../../util/storedFile.ts';

const inFlightDurations = new Map<string, Promise<number | null>>();

export const routes = {
  get: {
    handler: async (c: Context) => {
      const songId = positiveInteger(c.req.param('id'), 'song id');

      const [song] = await db
        .select({ filename: filesTable.filename, url: filesTable.url, duration: filesTable.duration })
        .from(songsTable)
        .leftJoin(filesTable, eq(songsTable.url, filesTable.url))
        .where(eq(songsTable.id, songId))
        .limit(1);

      if (!song) {
        throw new HTTPException(404, { message: 'Song not found' });
      }
      if (!song.filename) {
        throw new HTTPException(404, { message: 'Could not find file for song' });
      }

      try {
        if (song.duration != null) {
          c.header('Cache-Control', 'public, max-age=86400, immutable');
          return c.json({ duration: song.duration });
        }

        const filePath = storedSongPath(song.filename);
        let mtimeMs: number | undefined;
        try {
          const file = await stat(filePath);
          if (!file.isFile()) {
            throw new HTTPException(404, { message: 'Song file not found' });
          }
          mtimeMs = file.mtimeMs;
        } catch (error) {
          if (error instanceof HTTPException) throw error;
          if ((error as NodeJS.ErrnoException)?.code === 'ENOENT') {
            throw new HTTPException(404, { message: 'Song file not found' });
          }
          throw error;
        }

        let pending = inFlightDurations.get(filePath);
        if (!pending) {
          pending = getDuration(filePath, mtimeMs ?? undefined);
          inFlightDurations.set(filePath, pending);
          pending.finally(() => {
            if (inFlightDurations.get(filePath) === pending) inFlightDurations.delete(filePath);
          });
        }
        const duration = await pending;
        if (duration == null) {
          throw new HTTPException(422, { message: 'Could not determine file duration' });
        }
        if (song.url) {
          await db.update(filesTable).set({ duration }).where(eq(filesTable.url, song.url)).execute();
        }
        c.header('Cache-Control', 'public, max-age=86400, immutable');
        return c.json({ duration });
      } catch (error) {
        if (error instanceof HTTPException) throw error;
        if ((error as NodeJS.ErrnoException)?.code === 'ENOENT') {
          throw new HTTPException(404, { message: 'Song file not found' });
        }
        console.error('failed to read song duration', error);
        throw new HTTPException(500, { message: 'Could not determine file duration' });
      }
    },
  },
};
