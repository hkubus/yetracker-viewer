import { and, count, eq, getColumns } from 'drizzle-orm';
import type { Context } from 'hono';
import { PRIMARY_CATALOG_ID } from '../../catalogs.ts';
import { db } from '../../db/client.ts';
import { erasTable, songsTable } from '../../db/schema.ts';
import { getCoverVersion } from '../../util/coverVersion.ts';
export const routes = {
  get: {
    handler: async (c: Context) => {
      const { imageUrl, isMain, ...rest } = getColumns(erasTable);
      const eras = await db
        .select({
          ...rest,
          coverSource: imageUrl,
          songsCount: count(songsTable.id),
        })
        .from(erasTable)
        .leftJoin(songsTable, and(eq(erasTable.id, songsTable.eraId), eq(songsTable.catalogId, PRIMARY_CATALOG_ID)))
        .where(eq(erasTable.isMain, 1))
        .groupBy(erasTable.id);
      return c.json(
        eras.map(({ coverSource, ...era }) => ({
          ...era,
          coverVersion: getCoverVersion(coverSource),
        })),
      );
    },
  },
};
