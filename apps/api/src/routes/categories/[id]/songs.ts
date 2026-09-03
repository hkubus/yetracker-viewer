import { and, asc, count, eq, getColumns, or, type SQL, sql } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { getCatalog, PRIMARY_CATALOG_ID } from '../../../catalogs.ts';
import { db } from '../../../db/client.ts';
import { filesTable, songsTable } from '../../../db/schema.ts';
import { isSongPlayable } from '../../../util/playableFiles.ts';
import { paginationValue } from '../../../util/request.ts';

function escapeLikePattern(value: string) {
  return value.replaceAll(/[\\%_]/g, (character) => `\\${character}`);
}

export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = c.req.param('id') ?? '';
      const catalog = getCatalog(id);
      if (!catalog || catalog.id === PRIMARY_CATALOG_ID) {
        throw new HTTPException(404, { message: 'Category does not exist' });
      }

      const { limit, offset, q } = c.req.query() as { limit: string; offset: string; q?: string };
      const normalizedQuery = q?.trim().replaceAll(/\s+/g, ' ');
      if (normalizedQuery && normalizedQuery.length > 100) {
        throw new HTTPException(400, { message: 'Search query is too long' });
      }
      const requestedLimit = paginationValue(limit, 100, 10_000, 'limit');
      const requestedOffset = paginationValue(offset, 0, 1_000_000, 'offset');
      const conditions: SQL[] = [eq(songsTable.catalogId, id)];
      if (normalizedQuery) {
        const pattern = `%${escapeLikePattern(normalizedQuery.toLocaleLowerCase())}%`;
        const likeEscape = sql`'\\'`;
        conditions.push(
          or(
            sql`${sql`lower(coalesce(${songsTable.name}, ''))`} like ${pattern} escape ${likeEscape}`,
            sql`${sql`lower(coalesce(${songsTable.notes}, ''))`} like ${pattern} escape ${likeEscape}`,
            sql`${sql`lower(coalesce(${songsTable.quality}, ''))`} like ${pattern} escape ${likeEscape}`,
            sql`${sql`lower(coalesce(${songsTable.availableLength}, ''))`} like ${pattern} escape ${likeEscape}`,
          ) as SQL,
        );
      }
      const categorySongs = and(...conditions);
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
