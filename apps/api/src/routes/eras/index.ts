import { count, eq, getColumns } from 'drizzle-orm';
import type { Context } from 'hono';
import { erasTable, songsTable } from '../../db/schema.ts';
import { db } from '../../index.ts';
export const routes = {
  get: {
    handler: async (c: Context) => {
      const { imageUrl, ...rest } = getColumns(erasTable);
      const eras = await db
        .select({
          ...rest,
          songsCount: count(songsTable.id),
        })
        .from(erasTable)
        .leftJoin(songsTable, eq(erasTable.id, songsTable.eraId))
        .groupBy(erasTable.id);
      return c.json(eras);
    },
  },
};
