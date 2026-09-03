import { count, eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { catalogSourceUrl, getCatalog, PRIMARY_CATALOG_ID } from '../../../catalogs.ts';
import { db } from '../../../db/client.ts';
import { songsTable } from '../../../db/schema.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = c.req.param('id') ?? '';
      const catalog = getCatalog(id);
      if (!catalog || catalog.id === PRIMARY_CATALOG_ID) {
        throw new HTTPException(404, { message: 'Category does not exist' });
      }

      const [{ songsCount }] = await db
        .select({ songsCount: count(songsTable.id) })
        .from(songsTable)
        .where(eq(songsTable.catalogId, id));

      c.header('Cache-Control', 'public, max-age=60, s-maxage=300, stale-while-revalidate=600');
      return c.json({
        id: catalog.id,
        name: catalog.name,
        description: catalog.description,
        songsCount,
        sourceUrl: catalogSourceUrl(catalog.gid),
      });
    },
  },
};
