import { stat } from 'node:fs/promises';
import { and, eq, isNotNull, isNull } from 'drizzle-orm';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { filesTable } from '../db/schema.ts';
import { getDuration } from './getDuration.ts';
import { deleteInvalidFile } from './invalidFiles.ts';
import { setSongPlayable } from './playableFiles.ts';
import { storedSongPath } from './storedFile.ts';

const BACKFILL_CONCURRENCY = Number(process.env.BACKFILL_CONCURRENCY ?? 8) || 8;

export async function cacheFileDuration(db: ReturnType<typeof drizzle>, file: { url: string; filename: string }) {
  const path = storedSongPath(file.filename);
  let details: Awaited<ReturnType<typeof stat>>;
  try {
    details = await stat(path);
  } catch (error) {
    console.error(`backfill: missing file for ${file.url} (${file.filename}), resetting for re-download`, error);
    // The file is gone but the row claims it is downloaded — reset so the
    // downloader retries it instead of serving a 404 forever.
    setSongPlayable(file.filename, false);
    await db.update(filesTable).set({ downloaded: 0, duration: null }).where(eq(filesTable.url, file.url)).execute();
    return;
  }
  if (!details.isFile() || details.size === 0) {
    console.error(`backfill: empty file for ${file.url} (${file.filename}), deleting for re-download`);
    await deleteInvalidFile(db, { filename: file.filename, url: file.url }, 'empty');
    return;
  }

  let duration: number | null;
  try {
    duration = await getDuration(path, details.mtimeMs);
  } catch (error) {
    // Fail open when ffprobe itself is missing (spawn ENOENT): leave the row
    // alone so a broken environment never mass-deletes the library.
    if ((error as NodeJS.ErrnoException)?.code === 'ENOENT') {
      console.error(`backfill: ffprobe unavailable, skipping ${file.url} (${file.filename})`, error);
      return;
    }
    console.error(`backfill: unreadable file for ${file.url} (${file.filename}), deleting for re-download`, error);
    await deleteInvalidFile(db, { filename: file.filename, url: file.url }, 'unreadable');
    return;
  }
  if (duration) {
    await db.update(filesTable).set({ duration }).where(eq(filesTable.url, file.url)).execute();
  } else {
    console.error(`backfill: unprobable file for ${file.url} (${file.filename}), deleting for re-download`);
    await deleteInvalidFile(db, { filename: file.filename, url: file.url }, 'no-duration');
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
