import { stat } from 'node:fs/promises';
import { and, eq, isNotNull, isNull } from 'drizzle-orm';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { filesTable } from '../db/schema.ts';
import { getDuration } from './getDuration.ts';
import { storedSongPath } from './storedFile.ts';

const BACKFILL_CONCURRENCY = Number(process.env.BACKFILL_CONCURRENCY ?? 8) || 8;

export async function cacheFileDuration(db: ReturnType<typeof drizzle>, file: { url: string; filename: string }) {
  const path = storedSongPath(file.filename);
  let details: Awaited<ReturnType<typeof stat>>;
  try {
    details = await stat(path);
  } catch (error) {
    console.error(`backfill: missing file for ${file.url} (${file.filename})`, error);
    // Mark as not downloaded so boot backfill stops retrying a file that
    // will never resolve.
    await db.update(filesTable).set({ downloaded: 0 }).where(eq(filesTable.url, file.url)).execute();
    return;
  }
  if (!details.isFile() || details.size === 0) {
    console.error(`backfill: zero-size file for ${file.url} (${file.filename}), marking duration 0`);
    // Record a zero duration so this row is excluded from future backfills
    // (`duration IS NULL`) instead of being retried on every boot.
    await db.update(filesTable).set({ duration: 0 }).where(eq(filesTable.url, file.url)).execute();
    return;
  }

  const duration = await getDuration(path, details.mtimeMs);
  if (duration) {
    await db.update(filesTable).set({ duration }).where(eq(filesTable.url, file.url)).execute();
  } else {
    console.error(`backfill: could not probe duration for ${file.url} (${file.filename}), marking duration 0`);
    await db.update(filesTable).set({ duration: 0 }).where(eq(filesTable.url, file.url)).execute();
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
      } catch (error) {
        // Missing or invalid media is marked inside cacheFileDuration; log
        // here so unexpected failures are visible instead of swallowed.
        console.error(`backfill failed for ${file.url} (${file.filename})`, error);
      }
    }
  }

  await Promise.all(Array.from({ length: Math.min(BACKFILL_CONCURRENCY, files.length) }, () => worker()));
}
