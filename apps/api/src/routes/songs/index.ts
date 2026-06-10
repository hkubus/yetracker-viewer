import type { FastifyReply, FastifyRequest } from 'fastify';
import { songsTable } from '../../db/schema.ts';

export const routes = {
  get: {
    handler: async (req: FastifyRequest, res: FastifyReply) => {
      const { limit, offset } = req.query as { limit: string; offset: string };
      const songs = await req.db
        .select()
        .from(songsTable)
        .limit(parseInt(limit || '10000000', 10))
        .offset(parseInt(offset || '0', 10));
      res.send(songs);
    },
  },
};
