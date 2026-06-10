import { eq, getColumns } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { erasTable } from '../../../db/schema.ts';
import { db } from '../../../index.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = c.req.param('id') as string;
      const { imageUrl, ...rest } = getColumns(erasTable);

      const era = await db
        .select(rest)
        .from(erasTable)
        .where(eq(erasTable.id, parseInt(id, 10)))
        .limit(1);
      if (!era) {
        throw new HTTPException(404, { message: 'Era does not exist' });
      }
      return c.json(era[0]);
    },
  },
};
