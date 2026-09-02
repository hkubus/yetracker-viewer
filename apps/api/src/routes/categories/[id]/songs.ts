import { asc, count, eq, getColumns } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { getCatalog, PRIMARY_CATALOG_ID } from '../../../catalogs.ts';
import { db } from '../../../db/client.ts';
import { filesTable, songsTable } from '../../../db/schema.ts';
import { isSongPlayable } from '../../../util/playableFiles.ts';
import { paginationValue } from '../../../util/request.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = c.req.param('id') ?? '';
      const catalog = getCatalog(id);
      if (!catalog || catalog.id === PRIMARY_CATALOG_ID) {
        throw new HTTPException(404, { message: 'Category does not exist' });
      }

      const { limit, offset } = c.req.query() as { limit: string; offset: string };
      const requestedLimit = paginationValue(limit, 100, 10_000, 'limit');
      const requestedOffset = paginationValue(offset, 0, 1_000_000, 'offset');
      const categorySongs = eq(songsTable.catalogId, id);
      const [{ total }] = await db.select({ total: count() }).from(songsTable).where(categorySongs);
      const songData = getColumns(songsTable);
      const songs = await db
        .select({
          ...songData,
          downloaded: filesTable.downloaded,
          filename: filesTable.filename,
          fileDuration: filesTable.duration,
        })
        .from(songsTable)
        .leftJoin(filesTable, eq(songsTable.url, filesTable.url))
        .where(categorySongs)
        .orderBy(asc(songsTable.id))
        .limit(requestedLimit)
        .offset(requestedOffset);

      c.header('X-Total-Count', String(total));
      return c.json(
        songs.map(({ filename, fileDuration, ...song }) => {
          const playable = isSongPlayable(filename);
          return {
            ...song,
            playable,
            duration: playable ? (fileDuration ?? null) : null,
          };
        }),
      );
    },
  },
};
