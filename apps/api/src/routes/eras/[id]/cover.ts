import { readFile, stat } from 'node:fs/promises';
import { join } from 'node:path';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { storagePath } from '../../../config.ts';
import { positiveInteger } from '../../../util/request.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = positiveInteger(c.req.param('id'), 'era id');
      const path = join(storagePath, 'covers', `${id}.avif`);
      try {
        const file = await stat(path);
        const etag = `"${file.size.toString(16)}-${Math.trunc(file.mtimeMs).toString(16)}"`;
        c.header('Cache-Control', 'public, max-age=0, must-revalidate');
        c.header('ETag', etag);
        c.header('Last-Modified', file.mtime.toUTCString());
        if (c.req.header('If-None-Match') === etag) {
          return c.body(null, 304);
        }
        const image = await readFile(path);
        c.header('Content-Type', 'image/avif');
        return c.body(image);
      } catch {
        throw new HTTPException(404, { message: 'Cover not found' });
      }
    },
  },
};
