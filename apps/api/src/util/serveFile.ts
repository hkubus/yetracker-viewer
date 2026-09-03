import { createReadStream } from 'node:fs';
import { Readable } from 'node:stream';
import type { Context } from 'hono';
import { stream } from 'hono/streaming';

/**
 * Pipe a file to the response, releasing the file handle when the client
 * disconnects. Without the abort hook, a cancelled download/stream leaves
 * the fs stream (and its buffers) alive until GC finalizes it.
 */
export function streamFile(c: Context, path: string, options?: { start?: number; end?: number }) {
  return stream(c, async (output) => {
    const input = createReadStream(path, { ...options });
    // Surface fs errors to the streaming pipeline instead of emitting an
    // unhandled 'error' event that crashes the process.
    const onError = (error: Error) => {
      input.destroy(error);
    };
    input.once('error', onError);
    const signal = c.req.raw.signal;
    const onAbort = () => input.destroy();
    if (signal?.aborted) {
      input.destroy();
      return;
    }
    signal?.addEventListener('abort', onAbort, { once: true });
    try {
      await output.pipe(Readable.toWeb(input) as ReadableStream);
    } finally {
      signal?.removeEventListener('abort', onAbort);
      input.removeListener('error', onError);
      input.destroy();
    }
  });
}
