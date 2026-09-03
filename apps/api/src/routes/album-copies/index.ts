import { asc, eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { db } from '../../db/client.ts';
import { erasTable, filesTable, songsTable } from '../../db/schema.ts';
import { getCoverVersion } from '../../util/coverVersion.ts';
import { isSongPlayable } from '../../util/playableFiles.ts';

const CATALOG_ID = 'album-copies';

function normalizeName(value: string) {
  return value.trim().replace(/\s+/g, ' ').toLowerCase();
}

export const routes = {
  get: {
    handler: async (c: Context) => {
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
          eraName: erasTable.name,
          eraImageUrl: erasTable.imageUrl,
          filename: filesTable.filename,
          fileDuration: filesTable.duration,
        })
        .from(songsTable)
        .leftJoin(erasTable, eq(songsTable.eraId, erasTable.id))
        .leftJoin(filesTable, eq(songsTable.url, filesTable.url))
        .where(eq(songsTable.catalogId, CATALOG_ID))
        .orderBy(asc(songsTable.id));

      const groups = new Map<
        string,
        {
          name: string;
          copies: Array<Record<string, unknown>>;
        }
      >();

      for (const { filename, fileDuration, eraName, eraImageUrl, ...song } of songs) {
        const name = song.name?.trim() || 'Untitled album copy';
        const key = normalizeName(name);
        const group = groups.get(key) ?? { name, copies: [] };
        const playable = isSongPlayable(filename);
        group.copies.push({
          ...song,
          eraName,
          coverVersion: getCoverVersion(eraImageUrl),
          playable,
          duration: playable ? (fileDuration ?? null) : null,
        });
        groups.set(key, group);
      }

      c.header('Cache-Control', 'public, max-age=60, s-maxage=300, stale-while-revalidate=600');
      return c.json(Array.from(groups.values()));
    },
  },
};
