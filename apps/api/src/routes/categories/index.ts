import { count } from 'drizzle-orm';
import type { Context } from 'hono';
import { catalogSourceUrl, getCategoryCatalogs } from '../../catalogs.ts';
import { db } from '../../db/client.ts';
import { songsTable } from '../../db/schema.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      const counts = await db
        .select({ catalogId: songsTable.catalogId, songsCount: count(songsTable.id) })
        .from(songsTable)
        .groupBy(songsTable.catalogId);
      const countsByCatalog = new Map(counts.map((entry) => [entry.catalogId, entry.songsCount]));

      return c.json(
        getCategoryCatalogs().map((catalog) => ({
          id: catalog.id,
          name: catalog.name,
          description: catalog.description,
          songsCount: countsByCatalog.get(catalog.id) ?? 0,
          sourceUrl: catalogSourceUrl(catalog.gid),
        })),
      );
    },
  },
};
