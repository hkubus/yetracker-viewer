import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { FastifyReply, FastifyRequest } from 'fastify';
import { storagePath } from '../../../config.ts';

export const routes = {
  get: {
    handler: async (req: FastifyRequest, res: FastifyReply) => {
      const { id } = req.params as { id: string };
      const path = join(storagePath, 'covers', `${id}.avif`);
      const image = await readFile(path);
      res.header('Content-Type', 'image/avif');
      res.send(image);
    },
  },
};
