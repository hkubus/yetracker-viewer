import { eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { songsTable } from '../../../db/schema.ts';
import { db } from '../../../index.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = c.req.param('id') as string;
      const { limit, offset } = c.req.query() as { limit: string; offset: string };
      const songs = await db
        .select()
        .from(songsTable)
        .where(eq(songsTable.eraId, parseInt(id, 10)))
        .limit(parseInt(limit || '10000000', 10))
        .offset(parseInt(offset || '0', 10));
      return c.json(songs);
    },
  },
};
