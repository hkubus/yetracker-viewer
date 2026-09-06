import { stat } from 'node:fs/promises';
import { extname } from 'node:path';
import { eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { db } from '../../../db/client.ts';
import { filesTable, songsTable } from '../../../db/schema.ts';
import { deleteInvalidFile } from '../../../util/invalidFiles.ts';
import { getFileMeta } from '../../../util/playableFiles.ts';
import { positiveInteger } from '../../../util/request.ts';
import { streamFile } from '../../../util/serveFile.ts';
import { storedSongPath } from '../../../util/storedFile.ts';

function downloadName(name: string | null, id: number, extension: string) {
  const safeName = (name ?? `song-${id}`)
    .replaceAll(/[\p{Cc}<>:"/\\|?*]/gu, ' ')
    .replaceAll(/\s+/g, ' ')
    .trim()
    .slice(0, 120);
  return `${safeName || `song-${id}`}${extension}`;
}

function encodeHeaderFilename(filename: string) {
  return encodeURIComponent(filename).replaceAll(
    /[!'()*]/g,
    (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

export const routes = {
  get: {
    handler: async (c: Context) => {
      const songId = positiveInteger(c.req.param('id'), 'song id');

      const [song] = await db
        .select({
          name: songsTable.name,
          filename: filesTable.filename,
          url: filesTable.url,
          duration: filesTable.duration,
        })
        .from(songsTable)
        .leftJoin(filesTable, eq(songsTable.url, filesTable.url))
        .where(eq(songsTable.id, songId))
        .limit(1);

      if (!song) {
        throw new HTTPException(404, { message: 'Song not found' });
      }
      if (!song.filename) {
        throw new HTTPException(404, { message: 'Song file not found' });
      }

      const path = storedSongPath(song.filename);
      if (song.duration === 0) {
        // Legacy marker for files that failed probing — remove the broken
        // file so it is re-downloaded instead of served forever.
        await deleteInvalidFile(db, { filename: song.filename, url: song.url }, 'no-duration');
        throw new HTTPException(404, { message: 'Song file not found' });
      }
      const cachedMeta = getFileMeta(song.filename);
      let fileSize = cachedMeta?.size ?? 0;
      let mtimeMs = cachedMeta?.mtimeMs ?? 0;
      if (!cachedMeta) {
        try {
          const file = await stat(path);
          if (!file.isFile() || file.size === 0) {
            await deleteInvalidFile(db, { filename: song.filename, url: song.url }, 'empty');
            throw new HTTPException(404, { message: 'Song file not found' });
          }
          fileSize = file.size;
          mtimeMs = file.mtimeMs;
        } catch (error) {
          if (error instanceof HTTPException) throw error;
          if ((error as NodeJS.ErrnoException)?.code === 'ENOENT') {
            // File is gone but the row claims it is downloaded — reset so
            // the downloader retries it instead of 404ing forever.
            await deleteInvalidFile(db, { filename: song.filename, url: song.url }, 'missing');
          }
          throw new HTTPException(404, { message: 'Song file not found' });
        }
      } else if (fileSize === 0) {
        await deleteInvalidFile(db, { filename: song.filename, url: song.url }, 'empty');
        throw new HTTPException(404, { message: 'Song file not found' });
      }

      const etag = `"${fileSize.toString(16)}-${Math.trunc(mtimeMs).toString(16)}"`;
      if (c.req.header('If-None-Match') === etag) {
        return c.body(null, 304);
      }

      const filename = downloadName(song.name, songId, extname(song.filename));
      c.header('Content-Type', 'application/octet-stream');
      c.header('ETag', etag);
      c.header('Accept-Ranges', 'bytes');
      c.header('Cache-Control', 'public, max-age=31536000, immutable');
      c.header(
        'Content-Disposition',
        `attachment; filename="song-${songId}${extname(song.filename)}"; filename*=UTF-8''${encodeHeaderFilename(filename)}`,
      );

      const rangeHeader = c.req.header('range');
      if (rangeHeader) {
        const ifRange = c.req.header('if-range');
        if (!ifRange || ifRange === etag) {
          const match = /^bytes=(\d*)-(\d*)$/.exec(rangeHeader);
          if (match) {
            const [, startText, endText] = match;
            let start = startText ? Number.parseInt(startText, 10) : 0;
            let end = endText ? Number.parseInt(endText, 10) : fileSize - 1;
            if (!startText && endText) {
              const suffixLength = Number.parseInt(endText, 10);
              start = Math.max(0, fileSize - suffixLength);
              end = fileSize - 1;
            }
            if (
              Number.isSafeInteger(start) &&
              Number.isSafeInteger(end) &&
              start >= 0 &&
              start < fileSize &&
              end >= start
            ) {
              end = Math.min(end, fileSize - 1);
              c.header('Content-Range', `bytes ${start}-${end}/${fileSize}`);
              c.header('Content-Length', String(end - start + 1));
              c.status(206);
              return streamFile(c, path, { start, end });
            }
          }
        }
      }

      c.header('Content-Length', String(fileSize));
      return streamFile(c, path);
    },
  },
};
