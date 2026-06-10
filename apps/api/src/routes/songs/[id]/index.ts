import { eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { songsTable } from '../../../db/schema.ts';
import { db } from '../../../index.ts';
export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = c.req.param('id') as string;
      const song = await db
        .select()
        .from(songsTable)
        .where(eq(songsTable.id, parseInt(id, 10)))
        .limit(1);
      return c.json(song);
    },
  },
};
