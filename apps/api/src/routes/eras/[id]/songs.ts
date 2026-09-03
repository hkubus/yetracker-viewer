import { and, asc, eq, or, type SQL, sql } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { PRIMARY_CATALOG_ID } from '../../../catalogs.ts';
import { db } from '../../../db/client.ts';
import { erasTable, filesTable, songsTable } from '../../../db/schema.ts';
import { isSongPlayable } from '../../../util/playableFiles.ts';
import { paginationValue, positiveInteger } from '../../../util/request.ts';

function escapeLikePattern(value: string) {
  return value.replaceAll(/[\\%_]/g, (character) => `\\${character}`);
}

export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = positiveInteger(c.req.param('id'), 'era id');
      const era = await db.select({ id: erasTable.id }).from(erasTable).where(eq(erasTable.id, id)).limit(1);
      if (era.length === 0) {
        throw new HTTPException(404, { message: 'Era does not exist' });
      }
      const { limit, offset, q } = c.req.query() as { limit: string; offset: string; q?: string };
      const normalizedQuery = q?.trim().replaceAll(/\s+/g, ' ');
      if (normalizedQuery && normalizedQuery.length > 100) {
        throw new HTTPException(400, { message: 'Search query is too long' });
      }
      const requestedLimit = paginationValue(limit, 100, 500, 'limit');
      const requestedOffset = paginationValue(offset, 0, 10_000, 'offset');
      const conditions: SQL[] = [eq(songsTable.eraId, id), eq(songsTable.catalogId, PRIMARY_CATALOG_ID)];
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
      const where = and(...conditions);
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
        .where(where)
        .orderBy(asc(songsTable.id))
        .limit(requestedLimit)
        .offset(requestedOffset);
      const total = songs.length > 0 ? songs[0].total : 0;
      c.header('X-Total-Count', String(total));
      c.header('Cache-Control', 'public, max-age=60, s-maxage=300, stale-while-revalidate=600');
      const songsWithPlayback = songs.map(({ filename, fileDuration, total: _total, ...song }) => {
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
