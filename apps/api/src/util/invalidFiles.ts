import { execFile } from 'node:child_process';
import { stat, unlink } from 'node:fs/promises';
import { basename, join } from 'node:path';
import { promisify } from 'node:util';
import { eq } from 'drizzle-orm';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { songsPath } from '../config.ts';
import { filesTable } from '../db/schema.ts';
import { getDuration } from './getDuration.ts';
import { setSongPlayable } from './playableFiles.ts';

const run = promisify(execFile);

type Db = ReturnType<typeof drizzle>;

export type InvalidReason = 'missing' | 'empty' | 'no-duration' | 'no-audio-stream' | 'unreadable' | 'unsafe-name';

export function isSafeFilename(filename: string): boolean {
  return (
    filename.length > 0 &&
    filename.length <= 255 &&
    basename(filename) === filename &&
    !filename.includes('/') &&
    !filename.includes('\\') &&
    !filename.includes('\0')
  );
}

/**
 * A file is valid only if it exists, is non-empty, ffprobe can read a
 * duration from it, and it contains an audio stream.
 *
 * Note: stream `bit_rate` is deliberately NOT used as a signal — VBR codecs
 * such as Opus report `bit_rate=N/A`, so requiring a bitrate would flag
 * perfectly good files as invalid. Stream presence is checked via
 * `codec_name` instead.
 */
export async function probeAudioFile(filename: string): Promise<{
  valid: boolean;
  reason?: InvalidReason;
  mtimeMs?: number;
}> {
  if (!isSafeFilename(filename)) return { valid: false, reason: 'unsafe-name' };
  const path = join(songsPath, filename);
  let mtimeMs: number;
  try {
    const details = await stat(path);
    if (!details.isFile()) return { valid: false, reason: 'missing' };
    if (details.size === 0) return { valid: false, reason: 'empty', mtimeMs: details.mtimeMs };
    mtimeMs = details.mtimeMs;
  } catch {
    return { valid: false, reason: 'missing' };
  }
  let duration: number | null;
  try {
    duration = await getDuration(path, mtimeMs);
  } catch (error) {
    // Fail open when ffprobe itself is unavailable (spawn ENOENT): the file
    // may be perfectly fine, so never delete in that case.
    if ((error as NodeJS.ErrnoException)?.code === 'ENOENT') return { valid: true, mtimeMs };
    return { valid: false, reason: 'unreadable', mtimeMs };
  }
  if (duration == null) return { valid: false, reason: 'no-duration', mtimeMs };
  const audio = await hasAudioStream(path);
  if (audio === null) return { valid: true, mtimeMs };
  if (!audio) return { valid: false, reason: 'no-audio-stream', mtimeMs };
  return { valid: true, mtimeMs };
}

/**
 * True when the file has at least one audio stream, false when it does not,
 * null when ffprobe itself could not run (fail open — never delete then).
 */
async function hasAudioStream(path: string): Promise<boolean | null> {
  try {
    const { stdout } = await run(
      'ffprobe',
      [
        '-v',
        'error',
        '-select_streams',
        'a:0',
        '-show_entries',
        'stream=codec_name',
        '-of',
        'default=noprint_wrappers=1:nokey=1',
        path,
      ],
      { timeout: 10_000 },
    );
    return stdout.trim().length > 0;
  } catch (error) {
    if ((error as NodeJS.ErrnoException)?.code === 'ENOENT') return null;
    return false;
  }
}

/**
 * Remove an invalid file from disk, drop it from the playable cache, and
 * reset its DB row (`downloaded = 0`, `duration = NULL`) so the downloader
 * retries it on the next sync instead of serving a broken file forever.
 */
export async function deleteInvalidFile(
  db: Db,
  file: { filename: string; url?: string | null },
  reason: InvalidReason,
): Promise<void> {
  const { filename, url } = file;
  if (isSafeFilename(filename)) {
    try {
      await unlink(join(songsPath, filename));
    } catch {
      // Already gone — still reset the caches/DB below.
    }
    setSongPlayable(filename, false);
  }
  if (url) {
    try {
      await db.update(filesTable).set({ downloaded: 0, duration: null }).where(eq(filesTable.url, url)).execute();
    } catch (error) {
      console.error(`failed to reset db row after deleting invalid file ${filename}`, error);
    }
  }
  console.error(`deleted invalid file ${filename} (reason: ${reason})`);
}
