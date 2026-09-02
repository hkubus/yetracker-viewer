import { and, asc, count, eq, getColumns } from 'drizzle-orm';
import type { Context } from 'hono';
import { PRIMARY_CATALOG_ID } from '../../../catalogs.ts';
import { db } from '../../../db/client.ts';
import { filesTable, songsTable } from '../../../db/schema.ts';
import { isSongPlayable } from '../../../util/playableFiles.ts';
import { paginationValue, positiveInteger } from '../../../util/request.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = positiveInteger(c.req.param('id'), 'era id');
      const { limit, offset } = c.req.query() as { limit: string; offset: string };
      const requestedLimit = paginationValue(limit, 10_000, 10_000, 'limit');
      const requestedOffset = paginationValue(offset, 0, 1_000_000, 'offset');
      const songData = getColumns(songsTable);
      const mainSongs = and(eq(songsTable.eraId, id), eq(songsTable.catalogId, PRIMARY_CATALOG_ID));
      const [{ total }] = await db.select({ total: count() }).from(songsTable).where(mainSongs);
      const songs = await db
        .select({
          ...songData,
          downloaded: filesTable.downloaded,
          filename: filesTable.filename,
          fileDuration: filesTable.duration,
        })
        .from(songsTable)
        .leftJoin(filesTable, eq(songsTable.url, filesTable.url))
        .where(mainSongs)
        .orderBy(asc(songsTable.id))
        .limit(requestedLimit)
        .offset(requestedOffset);
      c.header('X-Total-Count', String(total));
      const songsWithPlayback = songs.map(({ filename, fileDuration, ...song }) => {
        const playable = isSongPlayable(filename);
        return {
          ...song,
          playable,
          duration: playable ? (fileDuration ?? null) : null,
        };
      });
      return c.json(songsWithPlayback);
    },
  },
};
