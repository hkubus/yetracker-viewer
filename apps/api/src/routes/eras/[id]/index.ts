import { eq, getColumns } from 'drizzle-orm';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { db } from '../../../db/client.ts';
import { erasTable } from '../../../db/schema.ts';
import { getCoverVersion } from '../../../util/coverVersion.ts';
import { positiveInteger } from '../../../util/request.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = positiveInteger(c.req.param('id'), 'era id');
      const { imageUrl, isMain, ...rest } = getColumns(erasTable);

      const era = await db
        .select({
          ...rest,
          coverSource: imageUrl,
        })
        .from(erasTable)
        .where(eq(erasTable.id, id))
        .limit(1);
      if (era.length === 0) {
        throw new HTTPException(404, { message: 'Era does not exist' });
      }
      const { coverSource, ...eraData } = era[0];
      return c.json({
        ...eraData,
        coverVersion: getCoverVersion(coverSource),
      });
    },
  },
};
