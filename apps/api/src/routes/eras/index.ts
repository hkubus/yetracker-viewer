import { count, eq, getColumns } from 'drizzle-orm';
import type { FastifyReply, FastifyRequest } from 'fastify';
import { erasTable, songsTable } from '../../db/schema.ts';

export const routes = {
  get: {
    handler: async (req: FastifyRequest, res: FastifyReply) => {
      console.log('test?');
      const { imageUrl, ...rest } = getColumns(erasTable);
      const eras = await req.db
        .select({
          ...rest,
          songsCount: count(songsTable.id),
        })
        .from(erasTable)
        .leftJoin(songsTable, eq(erasTable.id, songsTable.eraId))
        .groupBy(erasTable.id);
      res.send(eras);
    },
  },
};
