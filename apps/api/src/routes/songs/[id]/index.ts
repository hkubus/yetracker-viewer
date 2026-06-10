import { eq } from 'drizzle-orm';
import type { FastifyReply, FastifyRequest } from 'fastify';
import { songsTable } from '../../../db/schema.ts';
export const routes = {
  get: {
    handler: async (req: FastifyRequest, res: FastifyReply) => {
      const { id } = req.params as { id: string };
      const song = await req.db
        .select()
        .from(songsTable)
        .where(eq(songsTable.id, parseInt(id, 10)))
        .limit(1);
      res.send(song);
    },
  },
};
