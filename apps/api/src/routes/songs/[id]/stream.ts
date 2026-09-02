import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import { Readable } from 'node:stream';
import { eq } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { stream } from 'hono/streaming';
import { maxConcurrentTranscodes } from '../../../config.ts';
import { db } from '../../../db/client.ts';
import { filesTable, songsTable } from '../../../db/schema.ts';
import { positiveInteger } from '../../../util/request.ts';
import { storedSongPath } from '../../../util/storedFile.ts';
import { transcode } from '../../../util/transcode.ts';

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
        let fileSize = 0;
        try {
          const file = await stat(path);
          if (!file.isFile() || file.size === 0) {
            throw new HTTPException(404, { message: 'Song file not found' });
          }
          fileSize = file.size;
        } catch (error) {
          if (error instanceof HTTPException) throw error;
          throw new HTTPException(404, { message: 'Song file not found' });
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
            throw new HTTPException(503, { message: 'Transcoding capacity reached; try again shortly' });
          }
          // const fileBitrate = await getBitrate(path);
          // if (fileBitrate / 1000 < qualityParsed) {
          //   const data = await readFile(path);
          //   return c.body(data);
          // }
          c.header('Content-Type', 'audio/opus');
          return stream(c, async (stream) => {
            activeTranscodes++;
            try {
              const response = await transcode(path, `${qualityParsed}k`);
              // @ts-expect-error
              await stream.pipe(response);
            } finally {
              activeTranscodes--;
            }
          });
        }
        let mimetype = '';
        switch (filename.split('.').at(-1)) {
          case 'mp3':
            mimetype = 'audio/mpeg';
            break;
          case 'opus':
            mimetype = 'audio/opus';
            break;
          case 'ogg':
            mimetype = 'audio/ogg';
            break;
          case 'flac':
            mimetype = 'audio/flac';
            break;
          case 'wav':
            mimetype = 'audio/wav';
            break;
          case 'aif':
            mimetype = 'audio/aiff';
            break;
          default:
            mimetype = 'application/octet-stream';
        }

        c.header('Content-Type', mimetype);
        const rangeHeader = c.req.header('range');

        c.header('Accept-Ranges', 'bytes');

        if (rangeHeader) {
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
          return stream(c, async (output) => {
            const input = createReadStream(path, { start, end });
            await output.pipe(Readable.toWeb(input) as ReadableStream);
          });
        }

        c.header('Content-Length', String(fileSize));
        return stream(c, async (output) => {
          const input = createReadStream(path);
          await output.pipe(Readable.toWeb(input) as ReadableStream);
        });
      } catch (e) {
        if (e instanceof HTTPException) {
          throw e;
        }
        console.log('failed to stream song', e);
        throw new HTTPException(500, { message: 'Could not stream song' });
      }
    },
  },
};
