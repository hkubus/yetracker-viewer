import type { FastifyReply, FastifyRequest } from 'fastify';
import { erasTable } from '../../db/schema.ts';

export const routes = {
  get: {
    handler: async (req: FastifyRequest, res: FastifyReply) => {
      const basic = (req.query as { basic?: string }).basic === 'true';
      const eras = await req.db.select().from(erasTable);
      if (basic) {
        res.send(eras.map((e) => ({ id: e.id, name: e.name })));
      } else {
        res.send(eras);
      }
    },
  },
};
