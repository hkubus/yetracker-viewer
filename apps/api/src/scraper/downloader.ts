import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createWriteStream, existsSync } from 'node:fs';
import { mkdir, readdir, rename, unlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { promisify } from 'node:util';
import { loadImage } from '@napi-rs/canvas';
import { eq, isNull } from 'drizzle-orm';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { songsPath, storagePath } from '../config.ts';
import { erasTable, filesTable } from '../db/schema.ts';
import { getDominantColor } from '../util/getDominantColor.ts';
import { refreshSongPlayable, setSongPlayable } from '../util/playableFiles.ts';

const run = promisify(execFile);
export async function downloadCovers(db: ReturnType<typeof drizzle>) {
  const eras = await db.select().from(erasTable).where(eq(erasTable.isMain, 1));
  for (const era of eras) {
    try {
      if (!era.imageUrl) {
        console.log('no image url for era id ', era.id);
        continue;
      }
      const req = await fetch(era.imageUrl, { signal: AbortSignal.timeout(30_000) });
      if (!req.ok) throw new Error(`Failed to download cover ${era.id}: HTTP ${req.status}`);
      const contentLength = Number(req.headers.get('content-length') ?? 0);
      if (contentLength > 20 * 1024 * 1024) {
        throw new Error(`Cover ${era.id} exceeds the 20 MiB size limit`);
      }
      const data = await req.bytes();
      if (data.byteLength > 20 * 1024 * 1024) {
        throw new Error(`Cover ${era.id} exceeds the 20 MiB size limit`);
      }
      const tempPath = join(storagePath, 'covers', `${era.id}.source.tmp`);
      const encodedPath = join(storagePath, 'covers', `${era.id}.tmp.avif`);
      const coverPath = join(storagePath, 'covers', `${era.id}.avif`);

      try {
        await writeFile(tempPath, data);
        await run('ffmpeg', [
          '-y',
          '-i',
          tempPath,
          '-vf',
          'scale=512:512',
          '-c:v',
          'libsvtav1',
          '-crf',
          '18',
          '-preset',
          '3',
          '-still-picture',
          '1',
          encodedPath,
        ]);
        await rename(encodedPath, coverPath);
      } finally {
        await unlink(tempPath).catch(() => undefined);
        await unlink(encodedPath).catch(() => undefined);
      }

      let dominantColorHex = '666666';
      try {
        const imageCanvas = await loadImage(data);
        const dominantColor = await getDominantColor(imageCanvas);
        const extractedColor = dominantColor.map((e) => e.toString(16).padStart(2, '0')).join('');
        if (/^[\da-f]{6}$/i.test(extractedColor)) dominantColorHex = extractedColor;
      } catch (error) {
        console.error(`dominant color extraction failed for era ${era.id} (${era.name})`, error);
      }

      // workaround for late registration
      if (era.name === 'Late Registration') dominantColorHex = '5a240a';
      await db.update(erasTable).set({ dominantColor: dominantColorHex }).where(eq(erasTable.id, era.id)).execute();
    } catch (error) {
      console.error(`cover processing failed for era ${era.id} (${era.name})`, error);
    }
  }
}

export async function downloadSongs(db: ReturnType<typeof drizzle>) {
  if (!existsSync(songsPath)) await mkdir(songsPath, { recursive: true });

  const dirContents = await readdir(songsPath);
  const hashToExtension = new Map<string, string>();
  for (const f of dirContents) {
    const [hash, extension] = f.split('.');
    hashToExtension.set(hash, extension);
  }

  const files = await db.select().from(filesTable).where(isNull(filesTable.downloaded)).execute();
  console.log('starting download of', files.length, 'files');
  let i = 0;
  for (const file of files) {
    if (i % 50 === 0) console.log('downloaded', i, 'of', files.length, 'files');
    let filename = createHash('sha256').update(file.url).digest('hex');
    if (hashToExtension.has(filename)) {
      filename = `${filename}.${hashToExtension.get(filename)}`;
      await db.update(filesTable).set({ downloaded: 1, filename }).where(eq(filesTable.url, file.url)).execute();
      await refreshSongPlayable(filename);
      i++;
      continue;
    }
    try {
      console.log(file.url);
      const url = new URL(file.url);
      const domain = url.host;
      switch (domain) {
        case 'pillows.su': {
          // continue;
          const hash = url.pathname.split('/').at(-1);
          const data = await fetch(`https://api.pillows.su/api/download/${hash}`, {
            signal: AbortSignal.timeout(30 * 60_000),
          });
          if (!data.ok || !data.body) {
            throw new Error(`Failed to download ${file.url}: HTTP ${data.status}`);
          }
          const reportedExtension = data.headers.get('content-disposition')?.split('.').at(-1)?.slice(0, -1);
          const fileExtension = reportedExtension?.toLowerCase().match(/^[a-z0-9]{1,8}$/)?.[0] ?? 'bin';
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
          // console.log(`Downloaded ${path}`);

          break;
        }
        case 'youtu.be': {
          console.log('Hi');
          await downloadYtdlp(url, filename);
          filename = `${filename}.ogg`;
          break;
        }
        case 'www.youtube.com': {
          await downloadYtdlp(url, filename);
          filename = `${filename}.ogg`;
          break;
        }
        case 'www.instagram.com': {
          await downloadYtdlp(url, filename);
          filename = `${filename}.ogg`;
          break;
        }
        case 'twitter.com': {
          await downloadYtdlp(url, filename);
          filename = `${filename}.ogg`;
          break;
        }
        default:
          console.log('unknown host', domain);
          break;
      }
    } catch {
      await db.update(filesTable).set({ downloaded: 0, filename }).where(eq(filesTable.url, file.url)).execute();
      if (filename) setSongPlayable(filename, false);
      filename = '';
    }
    if (filename !== '') {
      await db.update(filesTable).set({ downloaded: 1, filename }).where(eq(filesTable.url, file.url)).execute();
      await refreshSongPlayable(filename);
    }
    i++;
  }
}
async function downloadYtdlp(url: URL, filename: string) {
  const outputPath = join(songsPath, filename);
  const timestamp = url.searchParams.get('t');
  const args = ['-x', '--audio-quality', '0', '--audio-format', 'opus', '-o', outputPath];
  if (timestamp) args.push('--download-sections', `*${timestamp}-inf`);
  args.push(url.toString());
  await run('yt-dlp', args, { timeout: 30 * 60_000 });
  await rename(`${outputPath}.opus`, `${outputPath}.ogg`);
}
