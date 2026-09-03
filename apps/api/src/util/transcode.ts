import { existsSync } from 'node:fs';
import { Readable } from 'node:stream';
import { Converter } from 'ffmpeg-stream';
import { HTTPException } from 'hono/http-exception';
export async function transcode(
  inputPath: string,
  quality: string = '128k',
  signal?: AbortSignal,
): Promise<ReadableStream> {
  if (!existsSync(inputPath)) throw new HTTPException(404, { message: 'Song file not found' });
  const converter = new Converter();
  converter.createInputFromFile(inputPath);
  const converterOutput = converter.createOutputStream({
    f: 'ogg',
    'c:a': 'libopus',
    'b:a': quality,
    map_metadata: '0',
  });

  const onAbort = () => {
    converter.kill();
    converterOutput.destroy();
  };
  if (signal?.aborted) onAbort();
  else signal?.addEventListener('abort', onAbort, { once: true });
  // Drop the abort listener once the output settles so completed/failed
  // transcodes don't pin the request signal (and its closures) in memory.
  converterOutput.once('close', () => signal?.removeEventListener('abort', onAbort));

  const running = converter.run();
  running.catch((error) => {
    if (!signal?.aborted) console.error('transcode failed', error);
    converterOutput.destroy(error instanceof Error ? error : new Error('transcode failed'));
  });

  // Readable.toWeb returns a node:stream/web ReadableStream whose type
  // parameters differ from the DOM lib's; the runtime shape is compatible
  // with what hono's streaming helper consumes.
  return Readable.toWeb(converterOutput) as unknown as ReadableStream;
}
