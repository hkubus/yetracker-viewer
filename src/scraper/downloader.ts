import { existsSync } from 'node:fs';
import { mkdir, rename, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { eq, isNull } from 'drizzle-orm';
import type { drizzle } from 'drizzle-orm/node-sqlite';
import { youtubeDl } from 'youtube-dl-exec';
import { filesTable } from '../db/schema.ts';

export async function downloader(db: ReturnType<typeof drizzle>) {
  const path = join(import.meta.dirname, '../..', 'files');
  if (!existsSync(path)) await mkdir(path);

  const files = await db.select().from(filesTable).where(isNull(filesTable.downloaded)).execute();
  console.log('starting download of', files.length, 'files');
  let i = 0;
  for (const file of files) {
    if (i % 50 === 0) console.log('downloaded', i, 'of', files.length, 'files');
    let filename = file.filename;
    try {
      // console.log(file.url);
      const url = new URL(file.url);
      const domain = url.host;
      switch (domain) {
        case 'pillows.su': {
          // continue;
          const hash = url.pathname.split('/').at(-1);
          const data = await fetch(`https://api.pillows.su/api/download/${hash}`);
          const buffer = await data.arrayBuffer();
          const fileExtension = data.headers.get('content-disposition')?.split('.').at(-1)?.slice(0, -1);
          filename = `${hash}.${fileExtension}`;
          const path = join(import.meta.dirname, '../..', 'files', filename);
          await writeFile(path, Buffer.from(buffer));
          // console.log(`Downloaded ${path}`);

          break;
        }
        case 'youtu.be': {
          filename = `${filename}.ogg`;
          await downloadYtdlp(url, filename);
          break;
        }
        case 'www.youtube.com': {
          filename = `${filename}.ogg`;
          await downloadYtdlp(url, filename);
          break;
        }
        case 'www.instagram.com': {
          filename = `${filename}.ogg`;
          await downloadYtdlp(url, filename);
          break;
        }
        case 'twitter.com': {
          filename = `${filename}.ogg`;
          await downloadYtdlp(url, filename);
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
  const path = join(import.meta.dirname, '../..', 'files', filename);
  console.log(url.toString());
  const timestamp = url?.toString()?.split('t=')[1] || undefined;
  console.log(timestamp);
  await youtubeDl(url.toString(), {
    extractAudio: true,
    audioQuality: 0,
    audioFormat: 'opus',
    output: path,
    downloadSections: timestamp ? `*${timestamp}-inf` : undefined,
  });
  rename(`${path}.opus`, `${path}`);
}
