import { and, eq, gte, lte, type SQL, sql } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { PRIMARY_CATALOG_ID } from '../../catalogs.ts';
import { db } from '../../db/client.ts';
import { erasTable, filesTable, songsTable } from '../../db/schema.ts';
import { isSongPlayable } from '../../util/playableFiles.ts';
import { rankSongSearch } from '../../util/rankSongSearch.ts';
import { paginationValue, positiveInteger } from '../../util/request.ts';

const QUALITY_FILTERS = new Set([
  'Low Quality',
  'High Quality',
  'CD Quality',
  'Lossless',
  'Not Available',
  'Recording',
]);
const AVAILABILITY_FILTERS = new Set([
  'Full',
  'Snippet',
  'Confirmed',
  'Beat Only',
  'Partial',
  'Tagged',
  'OG File',
  'Stem Bounce',
  'Rumored',
  'Conflicting Sources',
]);

function enumFilter(value: string | undefined, allowedValues: Set<string>, label: string) {
  if (!value) return undefined;
  if (!allowedValues.has(value)) {
    throw new HTTPException(400, { message: `Invalid ${label} filter` });
  }
  return value;
}

export const routes = {
  get: {
    handler: async (c: Context) => {
      const { limit, offset, q, era, eraFrom, eraTo, quality, availability, playable } = c.req.query() as {
        limit: string;
        offset: string;
        q?: string;
        era?: string;
        eraFrom?: string;
        eraTo?: string;
        quality?: string;
        availability?: string;
        playable?: string;
      };
      const query = q?.trim().replaceAll(/\s+/g, ' ').toLowerCase();
      // NOTE: SQLite lower() is ASCII-only; both sides use plain
      // toLowerCase() (not locale-aware) so matching stays consistent.
      if (query && query.length > 100) {
        throw new HTTPException(400, { message: 'Search query is too long' });
      }
      const eraId = era ? positiveInteger(era, 'era filter') : undefined;
      const eraFromId = eraFrom ? positiveInteger(eraFrom, 'starting era filter') : undefined;
      const eraToId = eraTo ? positiveInteger(eraTo, 'ending era filter') : undefined;
      if (eraFromId !== undefined && eraToId !== undefined && eraFromId > eraToId) {
        throw new HTTPException(400, { message: 'Starting era must not be after ending era' });
      }
      const qualityFilter = enumFilter(quality, QUALITY_FILTERS, 'quality');
      const availabilityFilter = enumFilter(availability, AVAILABILITY_FILTERS, 'availability');
      if (playable && !['true', 'false'].includes(playable)) {
        throw new HTTPException(400, { message: 'Invalid playable filter' });
      }
      const playableFilter = playable ? playable === 'true' : undefined;
      const hasFilters =
        eraId !== undefined ||
        eraFromId !== undefined ||
        eraToId !== undefined ||
        qualityFilter !== undefined ||
        availabilityFilter !== undefined ||
        playableFilter !== undefined;

      if (query || hasFilters) {
        const searchableText = sql<string>`lower(
          replace(
            replace(
              coalesce(${songsTable.name}, '') || ' ' ||
              coalesce(${songsTable.notes}, '') || ' ' ||
              coalesce(${erasTable.name}, '') || ' ' ||
              coalesce(${songsTable.quality}, '') || ' ' ||
              coalesce(${songsTable.availableLength}, ''),
              char(13),
              ' '
            ),
            char(10),
            ' '
          )
        )`;
        const matchesQuery = query ? sql<boolean>`instr(${searchableText}, ${query}) > 0` : undefined;
        const eraPosition = sql<number>`row_number() over (partition by ${songsTable.eraId} order by ${songsTable.id})`;
        const requestedLimit = paginationValue(limit, 50, 50, 'limit');
        const conditions: SQL[] = [eq(songsTable.catalogId, PRIMARY_CATALOG_ID)];
        if (query && matchesQuery) conditions.push(matchesQuery);
        if (eraId !== undefined) conditions.push(eq(songsTable.eraId, eraId));
        if (eraFromId !== undefined) conditions.push(gte(songsTable.eraId, eraFromId));
        if (eraToId !== undefined) conditions.push(lte(songsTable.eraId, eraToId));
        if (qualityFilter !== undefined) conditions.push(eq(songsTable.quality, qualityFilter));
        if (availabilityFilter !== undefined) {
          conditions.push(eq(songsTable.availableLength, availabilityFilter));
        }
        const databaseMatches = await db
          .select({
            id: songsTable.id,
            eraId: songsTable.eraId,
            name: songsTable.name,
            notes: songsTable.notes,
            quality: songsTable.quality,
            availableLength: songsTable.availableLength,
            eraName: erasTable.name,
            dominantColor: erasTable.dominantColor,
            filename: filesTable.filename,
            eraPosition,
          })
          .from(songsTable)
          .leftJoin(erasTable, eq(songsTable.eraId, erasTable.id))
          .leftJoin(filesTable, eq(songsTable.url, filesTable.url))
          .where(and(...conditions))
          // Cap the candidate set before JS ranking: full-catalog scans
          // deserialized thousands of rows per keystroke otherwise.
          .limit(1000);
        let matches = databaseMatches.map(({ filename, ...song }) => ({
          ...song,
          playable: isSongPlayable(filename),
        }));
        if (playableFilter !== undefined) {
          matches = matches.filter((song) => song.playable === playableFilter);
        }
        const rankedMatches = query ? rankSongSearch(matches, query, requestedLimit) : matches.slice(0, requestedLimit);
        if (rankedMatches.length === 0) {
          c.header('Cache-Control', 'public, max-age=60, s-maxage=300, stale-while-revalidate=600');
          return c.json({ songs: [], total: matches.length });
        }
        const positionedMatches = rankedMatches.map((song) => ({
          ...song,
          eraPosition: song.eraPosition ?? 1,
        }));

        c.header('Cache-Control', 'public, max-age=60, s-maxage=300, stale-while-revalidate=600');
        return c.json({ songs: positionedMatches, total: matches.length });
      }

      const requestedLimit = paginationValue(limit, 100, 500, 'limit');
      const requestedOffset = paginationValue(offset, 0, 10_000, 'offset');
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
        })
        .from(songsTable)
        .where(eq(songsTable.catalogId, PRIMARY_CATALOG_ID))
        .limit(requestedLimit)
        .offset(requestedOffset);
      c.header('Cache-Control', 'public, max-age=60, s-maxage=300, stale-while-revalidate=600');
      return c.json(songs);
    },
  },
};
