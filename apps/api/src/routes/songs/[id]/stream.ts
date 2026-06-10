import { createReadStream } from 'node:fs';
import { join } from 'node:path';
import { eq } from 'drizzle-orm';
import type { FastifyReply, FastifyRequest } from 'fastify';
import { storagePath } from '../../../config.ts';
import { filesTable, songsTable } from '../../../db/schema.ts';
import { transcode } from '../../../util/transcode.ts';

export const routes = {
  get: {
    handler: async (req: FastifyRequest, res: FastifyReply) => {
      const { id } = req.params as { id: string };
      const { quality } = req.query as { quality?: string };
      const songId = Number.parseInt(id, 10);

      if (Number.isNaN(songId)) {
        return res.code(400).send({ error: 'Invalid song id' });
      }

      const [song] = await req.db
        .select({
          id: songsTable.id,
          filename: filesTable.filename,
        })
        .from(songsTable)
        .leftJoin(filesTable, eq(songsTable.url, filesTable.url))
        .where(eq(songsTable.id, songId))
        .limit(1);

      if (!song) {
        return res.code(404).send({ error: 'Song not found' });
      }

      if (!song.filename) {
        return res.code(404).send({ error: 'File not found for song' });
      }

      const { filename } = song;
      const path = join(storagePath, 'songs', filename);
      if (quality) {
        const qualityParsed = parseInt(quality, 10);
        if (Number.isNaN(qualityParsed)) {
          return res.code(400).send({ error: 'Invalid quality' });
        }
        return res.send(await transcode(path, `${qualityParsed}k`));
      }
      // let mimetype = '';
      // switch (filename.split('.').at(-1)) {
      //   case 'mp3':
      //     mimetype = 'audio/mpeg';
      //     break;
      //   case 'opus':
      //     mimetype = 'audio/opus';
      //     break;
      //   case 'flac':
      //     mimetype = 'audio/flac';
      //     break;
      //   case 'wav':
      //     mimetype = 'audio/wav';
      //     break;
      //   case 'aif':
      //     mimetype = 'audio/aiff';
      //     break;
      //   default:
      //     mimetype = 'application/octet-stream';
      // }
      const stream = createReadStream(path);
      // res.header('Content-Type', mimetype);
      // const stream = await transcode(path);
      return res.send(stream);
    },
  },
};
