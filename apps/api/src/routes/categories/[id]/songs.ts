import { and, asc, eq, or, type SQL, sql } from 'drizzle-orm';
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
      const requestedLimit = paginationValue(limit, 100, 500, 'limit');
      const requestedOffset = paginationValue(offset, 0, 10_000, 'offset');
      const conditions: SQL[] = [eq(songsTable.catalogId, id)];
      if (normalizedQuery) {
        const pattern = `%${escapeLikePattern(normalizedQuery.toLowerCase())}%`;
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
      const songs = await db
        .select({
          id: songsTable.id,
          eraId: songsTable.eraId,
          catalogId: songsTable.catalogId,
          name: songsTable.name,
          notes: songsTable.notes,
          fileDate: songsTable.fileDate,
          leakDate: songsTable.leakDate,
          availableLength: songsTable.availableLength,
          trackLength: songsTable.trackLength,
          quality: songsTable.quality,
          url: songsTable.url,
          downloaded: filesTable.downloaded,
          filename: filesTable.filename,
          fileDuration: filesTable.duration,
          total: sql<number>`count(*) over()`,
        })
        .from(songsTable)
        .leftJoin(filesTable, eq(songsTable.url, filesTable.url))
        .where(categorySongs)
        .orderBy(asc(songsTable.id))
        .limit(requestedLimit)
        .offset(requestedOffset);

      const total = songs.length > 0 ? songs[0].total : 0;
      c.header('X-Total-Count', String(total));
      c.header('Cache-Control', 'public, max-age=60, s-maxage=300, stale-while-revalidate=600');
      return c.json(
        songs.map(({ filename, fileDuration, total: _total, ...song }) => {
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
