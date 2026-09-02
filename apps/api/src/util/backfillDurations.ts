import { stat } from 'node:fs/promises';
import { and, eq, isNotNull, isNull } from 'drizzle-orm';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { filesTable } from '../db/schema.ts';
import { getDuration } from './getDuration.ts';
import { storedSongPath } from './storedFile.ts';

const BACKFILL_CONCURRENCY = 2;

export async function cacheFileDuration(db: ReturnType<typeof drizzle>, file: { url: string; filename: string }) {
  const path = storedSongPath(file.filename);
  const details = await stat(path);
  if (!details.isFile() || details.size === 0) return;

  const duration = await getDuration(path);
  if (duration) {
    db.update(filesTable).set({ duration }).where(eq(filesTable.url, file.url)).run();
  }
}

export async function backfillDurations(db: ReturnType<typeof drizzle>) {
  const files = await db
    .select({ url: filesTable.url, filename: filesTable.filename })
    .from(filesTable)
    .where(and(eq(filesTable.downloaded, 1), isNotNull(filesTable.filename), isNull(filesTable.duration)));

  if (files.length === 0) return;
  console.log('backfilling duration of', files.length, 'files');

  let nextIndex = 0;
  async function worker() {
    while (nextIndex < files.length) {
      const file = files[nextIndex++];
      if (!file.filename) continue;
      try {
        await cacheFileDuration(db, { url: file.url, filename: file.filename });
      } catch {
        // Missing or invalid media stays uncached and does not delay API requests.
      }
    }
  }

  await Promise.all(Array.from({ length: Math.min(BACKFILL_CONCURRENCY, files.length) }, () => worker()));
}
