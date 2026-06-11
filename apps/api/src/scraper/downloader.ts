import { exec } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir, readdir, unlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { loadImage } from '@napi-rs/canvas';
import { eq, isNull } from 'drizzle-orm';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { storagePath } from '../config.ts';
import { erasTable, filesTable } from '../db/schema.ts';
import { getDominantColor } from '../util/getDominantColor.ts';

const run = promisify(exec);
export async function downloadCovers(db: ReturnType<typeof drizzle>) {
  const eras = await db.select({ id: erasTable.id, imageUrl: erasTable.imageUrl }).from(erasTable);
  for (const era of eras) {
    if (!era.imageUrl) return console.log('no image url for era id ', era.id);
    const req = await fetch(era.imageUrl);
    const data = await req.bytes();
    const imageCanvas = await loadImage(data);
    const dominantColor = await getDominantColor(imageCanvas);
    const dominantColorHex = dominantColor.map((e) => e.toString(16).padStart(2, '0')).join('');
    // const image = await sharp(data).resize(512, 512).avif({ effort: 8, quality: 70 }).toBuffer();\
    const tempPath = join(storagePath, 'covers', `${era.id}.temp`);
    await writeFile(tempPath, data);
    await run(
      `ffmpeg -i ${tempPath} -vf scale=512:512 -c:v libsvtav1 -crf 18 -preset 1 -still-picture 1 ${join(storagePath, 'covers', `${era.id}.avif`)}`,
    );
    await unlink(tempPath);
    db.update(erasTable).set({ dominantColor: dominantColorHex }).where(eq(erasTable.id, era.id));
  }
}

export async function downloadSongs(db: ReturnType<typeof drizzle>) {
  const songsPath = join(storagePath, 'songs');
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
          const data = await fetch(`https://api.pillows.su/api/download/${hash}`);
          const buffer = await data.arrayBuffer();
          const fileExtension = data.headers.get('content-disposition')?.split('.').at(-1)?.slice(0, -1);
          filename = `${filename}.${fileExtension}`;
          await writeFile(join(songsPath, filename), Buffer.from(buffer));
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
    } catch (e) {
      console.log(e, file.url);
    }
    if (filename)
      await db.update(filesTable).set({ downloaded: 1, filename }).where(eq(filesTable.url, file.url)).execute();
    i++;
  }
}
async function downloadYtdlp(url: URL, filename: string) {
  const path = join(storagePath, 'songs');
  console.log('test');
  const outputPath = join(path, filename);
  const timestamp = url?.toString()?.split('t=')[1] || undefined;
  await run(
    `yt-dlp -x --audio-quality 0 --audio-format opus -o ${outputPath} ${timestamp ? `--download-sections "*${timestamp}-inf"` : ''} ${url.toString()} && mv ${outputPath}.opus ${outputPath}.ogg`,
  );
}
