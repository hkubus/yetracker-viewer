import { eq } from 'drizzle-orm';
import type { FastifyReply, FastifyRequest } from 'fastify';
import { erasTable } from '../../../db/schema.ts';

export const routes = {
  get: {
    handler: async (req: FastifyRequest, res: FastifyReply) => {
      const { id } = req.params as { id: string };
      const era = await req.db
        .select()
        .from(erasTable)
        .where(eq(erasTable.id, parseInt(id, 10)))
        .limit(1);
      if (!era) {
        return res.status(404).send({ message: 'Era not found' });
      }
      return res.send(era[0]);
    },
  },
};
