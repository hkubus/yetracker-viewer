import { eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { db } from '../../../db/client.ts';
import { songsTable } from '../../../db/schema.ts';
import { positiveInteger } from '../../../util/request.ts';
export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = positiveInteger(c.req.param('id'), 'song id');
      const song = await db.select().from(songsTable).where(eq(songsTable.id, id)).limit(1);
      return c.json(song);
    },
  },
};
