import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createWriteStream } from 'node:fs';
import { access, mkdir, readdir, rename, stat, unlink } from 'node:fs/promises';
import { join } from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { promisify } from 'node:util';
import { eq, isNull, or } from 'drizzle-orm';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { songsPath, storagePath, youtubeDownload } from '../config.ts';
import { erasTable, filesTable } from '../db/schema.ts';
import { getDominantColor } from '../util/getDominantColor.ts';
import { deleteInvalidFile, probeAudioFile } from '../util/invalidFiles.ts';
import { refreshSongPlayable, setSongPlayable } from '../util/playableFiles.ts';

const run = promisify(execFile);

const COVER_FETCH_TIMEOUT_MS = 30_000;
const COVER_MAX_BYTES = 20 * 1024 * 1024;
const PILLOW_DOWNLOAD_TIMEOUT_MS = 30 * 60_000;
const YTDLP_TIMEOUT_MS = 30 * 60_000;
const YTDLP_MAX_BUFFER_BYTES = 10 * 1024 * 1024;
const FETCH_USER_AGENT = 'yetracker-viewer/1.0 (+https://yetracker.net)';
const COVER_CONCURRENCY = 4;
const SONG_CONCURRENCY = 5;

// Fallback cover color sampled from `getDominantColor` is unreliable for this
// era's artwork, so pin it to a known-good value.
const LATE_REGISTRATION_DOMINANT_COLOR = '5a240a';
const LATE_REGISTRATION_ERA_NAME = 'Late Registration';

// Tiny concurrency limiter so covers/songs download in parallel without
// pulling in a new dependency.
async function limitedMap<T, R>(items: T[], limit: number, fn: (item: T, index: number) => Promise<R>): Promise<R[]> {
  const results = new Array<R>(items.length);
  let nextIndex = 0;
  const workers = Array.from({ length: Math.min(Math.max(limit, 1), items.length) }, async () => {
    while (nextIndex < items.length) {
      const index = nextIndex++;
      results[index] = await fn(items[index], index);
    }
  });
  await Promise.all(workers);
  return results;
}

export async function downloadCovers(db: ReturnType<typeof drizzle>) {
  const eras = await db.select().from(erasTable).where(eq(erasTable.isMain, 1));
  await limitedMap(eras, COVER_CONCURRENCY, async (era) => {
    try {
      if (!era.imageUrl) {
        console.error('no image url for era id ', era.id);
        return;
      }
      const coverPath = join(storagePath, 'covers', `${era.id}.avif`);
      // Skip re-download/re-encode when the cover and color are already fresh.
      if (era.dominantColor) {
        try {
          const existing = await stat(coverPath);
          if (existing.isFile() && existing.size > 0) return;
        } catch {
          // Missing cover — fall through to download.
        }
      }
      const req = await fetch(era.imageUrl, {
        headers: { 'User-Agent': FETCH_USER_AGENT },
        signal: AbortSignal.timeout(COVER_FETCH_TIMEOUT_MS),
      });
      if (!req.ok) throw new Error(`Failed to download cover ${era.id}: HTTP ${req.status}`);
      const contentLength = Number(req.headers.get('content-length') ?? 0);
      if (contentLength > COVER_MAX_BYTES) {
        throw new Error(`Cover ${era.id} exceeds the 20 MiB size limit`);
      }
      if (!req.body) throw new Error(`Cover ${era.id} has no response body`);
      const tempPath = join(storagePath, 'covers', `${era.id}.source.tmp`);
      const encodedPath = join(storagePath, 'covers', `${era.id}.tmp.avif`);

      try {
        // Stream to disk instead of buffering up to 20MiB in heap.
        await pipeline(
          Readable.fromWeb(req.body as unknown as import('node:stream/web').ReadableStream),
          createWriteStream(tempPath),
        );
        await run(
          'ffmpeg',
          [
            '-y',
            '-i',
            tempPath,
            '-vf',
            'scale=512:512:force_original_aspect_ratio=increase,crop=512:512,setsar=1',
            '-c:v',
            'libsvtav1',
            '-crf',
            '18',
            '-preset',
            '3',
            '-still-picture',
            '1',
            encodedPath,
          ],
          { timeout: 120_000, maxBuffer: YTDLP_MAX_BUFFER_BYTES },
        );
        await rename(encodedPath, coverPath);
      } finally {
        await unlink(tempPath).catch(() => undefined);
        await unlink(encodedPath).catch(() => undefined);
      }

      let dominantColorHex = '666666';
      try {
        const dominantColor = await getDominantColor(coverPath);
        const extractedColor = dominantColor.map((e) => e.toString(16).padStart(2, '0')).join('');
        if (/^[\da-f]{6}$/i.test(extractedColor)) dominantColorHex = extractedColor;
      } catch (error) {
        console.error(`dominant color extraction failed for era ${era.id} (${era.name})`, error);
      }

      // workaround for late registration
      if (era.name === LATE_REGISTRATION_ERA_NAME) dominantColorHex = LATE_REGISTRATION_DOMINANT_COLOR;
      await db.update(erasTable).set({ dominantColor: dominantColorHex }).where(eq(erasTable.id, era.id)).execute();
    } catch (error) {
      console.error(`cover processing failed for era ${era.id} (${era.name})`, error);
    }
  });
}

function splitFilename(filename: string): { hash: string; extension: string } | null {
  const dot = filename.lastIndexOf('.');
  if (dot <= 0 || dot === filename.length - 1) return null;
  return { hash: filename.slice(0, dot), extension: filename.slice(dot + 1) };
}

function parseContentDispositionExtension(header: string | null): string {
  if (!header) return 'bin';
  const match = /filename\*?=(?:UTF-8'' )?"?([^";]+)"?/.exec(header);
  if (!match) return 'bin';
  let candidate = match[1].trim();
  try {
    if (/^UTF-8''/i.test(header) || candidate.includes('%')) candidate = decodeURIComponent(candidate);
  } catch {
    // Keep the raw candidate if percent-decoding fails.
  }
  const dot = candidate.lastIndexOf('.');
  const extension = (dot >= 0 ? candidate.slice(dot + 1) : candidate).toLowerCase();
  return /^[a-z0-9]{1,8}$/.test(extension) ? extension : 'bin';
}

export async function downloadSongs(db: ReturnType<typeof drizzle>) {
  try {
    await access(songsPath);
  } catch {
    await mkdir(songsPath, { recursive: true });
  }

  const dirContents = await readdir(songsPath);
  const hashToExtension = new Map<string, string>();
  for (const f of dirContents) {
    if (f.endsWith('.tmp') || f.endsWith('.part')) continue;
    const split = splitFilename(f);
    if (!split) continue;
    hashToExtension.set(split.hash, split.extension);
  }

  const files = await db
    .select()
    .from(filesTable)
    .where(or(isNull(filesTable.downloaded), eq(filesTable.downloaded, 0)))
    .execute();
  console.log('starting download of', files.length, 'files');
  let completed = 0;
  await limitedMap(files, SONG_CONCURRENCY, async (file) => {
    if (completed % 50 === 0) console.log('downloaded', completed, 'of', files.length, 'files');
    let filename = createHash('sha256').update(file.url).digest('hex');
    if (hashToExtension.has(filename)) {
      const reused = `${filename}.${hashToExtension.get(filename)}`;
      const probe = await probeAudioFile(reused);
      if (probe.valid) {
        await db
          .update(filesTable)
          .set({ downloaded: 1, filename: reused })
          .where(eq(filesTable.url, file.url))
          .execute();
        await refreshSongPlayable(reused);
        completed++;
        return;
      }
      // A file with this hash is already on disk but it is corrupt or not
      // audio — remove it and fall through to download a fresh copy below.
      await deleteInvalidFile(db, { filename: reused, url: file.url }, probe.reason ?? 'unreadable');
    }
    try {
      const url = new URL(file.url);
      const domain = url.host;
      switch (domain) {
        case 'pillows.su': {
          const hash = url.pathname.split('/').at(-1);
          const data = await fetch(`https://api.pillows.su/api/download/${hash}`, {
            headers: { 'User-Agent': FETCH_USER_AGENT },
            signal: AbortSignal.timeout(PILLOW_DOWNLOAD_TIMEOUT_MS),
          });
          if (!data.ok || !data.body) {
            throw new Error(`Failed to download ${file.url}: HTTP ${data.status}`);
          }
          const fileExtension = parseContentDispositionExtension(data.headers.get('content-disposition'));
          filename = `${filename}.${fileExtension}`;
          // Stream the body straight to disk: buffering whole files with
          // arrayBuffer() (+ another copy via Buffer.from) keeps up to 2x
          // the file size on the heap per download.
          const tempFilename = `${filename}.tmp`;
          try {
            await pipeline(
              Readable.fromWeb(data.body as unknown as import('node:stream/web').ReadableStream),
              createWriteStream(join(songsPath, tempFilename)),
            );
            await rename(join(songsPath, tempFilename), join(songsPath, filename));
          } finally {
            await unlink(join(songsPath, tempFilename)).catch(() => undefined);
          }

          break;
        }
        case 'youtu.be':
        case 'www.youtube.com': {
          if (!youtubeDownload) {
            // Disabled via YOUTUBE_DOWNLOAD=false: leave the row pending
            // (downloaded = 0) so enabling it later resumes these downloads.
            // Already-downloaded files are untouched and keep serving.
            console.log(`skipping YouTube download (YOUTUBE_DOWNLOAD=false): ${file.url}`);
            filename = '';
            break;
          }
          await downloadYtdlp(url, filename);
          filename = `${filename}.ogg`;
          break;
        }
        case 'www.instagram.com':
        case 'twitter.com': {
          await downloadYtdlp(url, filename);
          filename = `${filename}.ogg`;
          break;
        }
        default:
          throw new Error(`unknown host ${domain} for ${file.url}`);
      }
    } catch (error) {
      console.error(`download failed for ${file.url}`, error);
      await db.update(filesTable).set({ downloaded: 0, filename }).where(eq(filesTable.url, file.url)).execute();
      if (filename) setSongPlayable(filename, false);
      filename = '';
    }
    if (filename !== '') {
      const probe = await probeAudioFile(filename);
      if (!probe.valid) {
        await deleteInvalidFile(db, { filename, url: file.url }, probe.reason ?? 'unreadable');
        completed++;
        return;
      }
      await db.update(filesTable).set({ downloaded: 1, filename }).where(eq(filesTable.url, file.url)).execute();
      await refreshSongPlayable(filename);
    }
    completed++;
  });
}

async function downloadYtdlp(url: URL, filename: string) {
  const outputPath = join(songsPath, filename);
  const timestamp = url.searchParams.get('t');
  const args = ['-x', '--audio-quality', '0', '--audio-format', 'opus', '-o', outputPath];
  if (timestamp !== null) {
    // Reject option injection: only a plain numeric timestamp is allowed.
    if (!/^\d+$/.test(timestamp)) {
      throw new Error(`Refusing to pass non-numeric timestamp to yt-dlp: ${timestamp}`);
    }
    args.push('--download-sections', `*${timestamp}-inf`);
  }
  args.push(url.toString());
  let delayMs = 1000;
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      await run('yt-dlp', args, { timeout: YTDLP_TIMEOUT_MS, maxBuffer: YTDLP_MAX_BUFFER_BYTES });
      break;
    } catch (error) {
      if (attempt === 2) {
        console.error(`yt-dlp failed for ${url.toString()}`, error);
        throw error;
      }
      await new Promise((resolve) => setTimeout(resolve, delayMs + Math.random() * 500));
      delayMs *= 2;
    }
  }
  // yt-dlp appends the real container extension; handle .opus and .ogg.
  const opusPath = `${outputPath}.opus`;
  const oggPath = `${outputPath}.ogg`;
  try {
    await access(opusPath);
    if (opusPath !== oggPath) await rename(opusPath, oggPath);
  } catch {
    try {
      await access(oggPath);
    } catch {
      throw new Error(`yt-dlp produced neither ${opusPath} nor ${oggPath}`);
    }
  }
}
