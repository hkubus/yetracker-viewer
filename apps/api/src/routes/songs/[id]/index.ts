import { eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { db } from '../../../db/client.ts';
import { songsTable } from '../../../db/schema.ts';
import { positiveInteger } from '../../../util/request.ts';
export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = positiveInteger(c.req.param('id'), 'song id');
      const song = await db.select().from(songsTable).where(eq(songsTable.id, id)).limit(1);
      if (song.length === 0) {
        throw new HTTPException(404, { message: 'Song not found' });
      }
      c.header('Cache-Control', 'public, max-age=60, s-maxage=300, stale-while-revalidate=600');
      return c.json(song[0]);
    },
  },
};
