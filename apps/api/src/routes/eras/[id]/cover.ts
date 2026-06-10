import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { Context } from 'hono';
import { storagePath } from '../../../config.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      console.log('test');
      const id = c.req.param('id') as string;
      const path = join(storagePath, 'covers', `${id}.avif`);
      const image = await readFile(path);
      c.header('Content-Type', 'image/avif');
      return c.body(image);
    },
  },
};
