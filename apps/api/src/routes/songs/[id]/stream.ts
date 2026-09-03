import { stat } from 'node:fs/promises';
import { extname } from 'node:path';
import { eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { stream } from 'hono/streaming';
import { maxConcurrentTranscodes } from '../../../config.ts';
import { db } from '../../../db/client.ts';
import { filesTable, songsTable } from '../../../db/schema.ts';
import { getFileMeta } from '../../../util/playableFiles.ts';
import { positiveInteger } from '../../../util/request.ts';
import { streamFile } from '../../../util/serveFile.ts';
import { storedSongPath } from '../../../util/storedFile.ts';
import { transcode } from '../../../util/transcode.ts';

const MIME_BY_EXT: Record<string, string> = {
  mp3: 'audio/mpeg',
  opus: 'audio/opus',
  ogg: 'audio/ogg',
  flac: 'audio/flac',
  wav: 'audio/wav',
  aif: 'audio/aiff',
  aiff: 'audio/aiff',
  m4a: 'audio/mp4',
  aac: 'audio/aac',
  mp4: 'video/mp4',
  webm: 'video/webm',
};

let activeTranscodes = 0;

export const routes = {
  get: {
    handler: async (c: Context) => {
      try {
        const quality = c.req.query('quality') as string;
        const songId = positiveInteger(c.req.param('id'), 'song id');

        const [song] = await db
          .select({
            id: songsTable.id,
            filename: filesTable.filename,
          })
          .from(songsTable)
          .leftJoin(filesTable, eq(songsTable.url, filesTable.url))
          .where(eq(songsTable.id, songId))
          .limit(1);

        if (!song) {
          throw new HTTPException(404, { message: 'Song not found' });
        }

        if (!song.filename) {
          throw new HTTPException(500, { message: 'Could not find file for song' });
        }

        const { filename } = song;
        const path = storedSongPath(filename);
        const cachedMeta = getFileMeta(filename);
        let fileSize = cachedMeta?.size ?? 0;
        let mtimeMs = cachedMeta?.mtimeMs ?? 0;
        if (!cachedMeta) {
          try {
            const file = await stat(path);
            if (!file.isFile() || file.size === 0) {
              throw new HTTPException(404, { message: 'Song file not found' });
            }
            fileSize = file.size;
            mtimeMs = file.mtimeMs;
          } catch (error) {
            if (error instanceof HTTPException) throw error;
            throw new HTTPException(404, { message: 'Song file not found' });
          }
        } else if (fileSize === 0) {
          throw new HTTPException(404, { message: 'Song file not found' });
        }

        const etag = `"${fileSize.toString(16)}-${Math.trunc(mtimeMs).toString(16)}"`;
        if (c.req.header('If-None-Match') === etag && !quality) {
          return c.body(null, 304);
        }

        if (quality) {
          if (!/^\d+$/.test(quality)) {
            throw new HTTPException(400, { message: 'Invalid quality for file' });
          }
          const qualityParsed = Number(quality);
          if (!Number.isSafeInteger(qualityParsed) || qualityParsed < 8 || qualityParsed > 320) {
            throw new HTTPException(400, { message: 'Invalid quality for file' });
          }
          if (activeTranscodes >= maxConcurrentTranscodes) {
            c.header('Retry-After', '5');
            throw new HTTPException(503, { message: 'Transcoding capacity reached; try again shortly' });
          }
          // Claim the slot synchronously (no awaits between the check and
          // the increment) to close the check-then-act race; release it in
          // the streaming callback's finally.
          activeTranscodes++;
          try {
            c.header('Content-Type', 'audio/opus');
            c.header('Accept-Ranges', 'none');
            c.header('Cache-Control', 'no-store');
            return stream(c, async (output) => {
              try {
                const response = await transcode(path, `${qualityParsed}k`, c.req.raw.signal);
                await output.pipe(response);
              } catch (error) {
                if (!c.req.raw.signal.aborted) console.error('failed to transcode song', error);
                throw error;
              } finally {
                activeTranscodes--;
              }
            });
          } catch (error) {
            // stream() threw synchronously before the callback ran; release the slot.
            activeTranscodes--;
            throw error;
          }
        }
        let mimetype = '';
        const ext = extname(filename).slice(1).toLowerCase();
        mimetype = MIME_BY_EXT[ext] ?? 'application/octet-stream';

        c.header('Content-Type', mimetype);
        c.header('ETag', etag);
        c.header('Cache-Control', 'public, max-age=31536000, immutable');
        const rangeHeader = c.req.header('range');

        c.header('Accept-Ranges', 'bytes');

        if (rangeHeader) {
          const ifRange = c.req.header('if-range');
          if (ifRange && ifRange !== etag) {
            c.header('Content-Length', String(fileSize));
            return streamFile(c, path);
          }
          const match = /^bytes=(\d*)-(\d*)$/.exec(rangeHeader);
          if (!match) {
            c.header('Content-Range', `bytes */${fileSize}`);
            return c.body(null, 416);
          }

          const [, startText, endText] = match;
          let start = startText ? Number.parseInt(startText, 10) : 0;
          let end = endText ? Number.parseInt(endText, 10) : fileSize - 1;

          if (!startText && endText) {
            const suffixLength = Number.parseInt(endText, 10);
            start = Math.max(0, fileSize - suffixLength);
            end = fileSize - 1;
          }

          if (
            !Number.isSafeInteger(start) ||
            !Number.isSafeInteger(end) ||
            start < 0 ||
            start >= fileSize ||
            end < start
          ) {
            c.header('Content-Range', `bytes */${fileSize}`);
            return c.body(null, 416);
          }

          end = Math.min(end, fileSize - 1);
          c.header('Content-Range', `bytes ${start}-${end}/${fileSize}`);
          c.header('Content-Length', String(end - start + 1));
          c.status(206);
          return streamFile(c, path, { start, end });
        }

        c.header('Content-Length', String(fileSize));
        return streamFile(c, path);
      } catch (e) {
        if (e instanceof HTTPException) {
          throw e;
        }
        console.error('failed to stream song', e);
        throw new HTTPException(500, { message: 'Could not stream song' });
      }
    },
  },
};
