import { stat } from 'node:fs/promises';
import { join } from 'node:path';
import type { Context } from 'hono';
import { HTTPException } from 'hono/http-exception';
import { storagePath } from '../../../config.ts';
import { getCoverVersion } from '../../../util/coverVersion.ts';
import { positiveInteger } from '../../../util/request.ts';
import { streamFile } from '../../../util/serveFile.ts';

export const routes = {
  get: {
    handler: async (c: Context) => {
      const id = positiveInteger(c.req.param('id'), 'era id');
      const path = join(storagePath, 'covers', `${id}.avif`);
      try {
        const file = await stat(path);
        if (!file.isFile()) {
          throw new HTTPException(404, { message: 'Cover not found' });
        }
        const etag = `"${getCoverVersion(`${id}`)}-${file.size.toString(16)}-${Math.trunc(file.mtimeMs).toString(16)}"`;
        c.header('Cache-Control', 'public, max-age=86400, immutable');
        c.header('ETag', etag);
        c.header('Last-Modified', file.mtime.toUTCString());
        if (c.req.header('If-None-Match') === etag) {
          return c.body(null, 304);
        }
        c.header('Content-Type', 'image/avif');
        c.header('Content-Length', String(file.size));
        c.header('Accept-Ranges', 'bytes');
        return streamFile(c, path);
      } catch (error) {
        if (error instanceof HTTPException) throw error;
        if ((error as NodeJS.ErrnoException)?.code === 'ENOENT') {
          throw new HTTPException(404, { message: 'Cover not found' });
        }
        console.error('failed to read cover', error);
        throw new HTTPException(500, { message: 'Could not load cover' });
      }
    },
  },
};
