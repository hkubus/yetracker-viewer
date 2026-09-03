import { existsSync } from 'node:fs';
import { Readable } from 'node:stream';
import { Converter } from 'ffmpeg-stream';
export async function transcode(inputPath: string, quality: string = '128k', signal?: AbortSignal) {
  if (!existsSync(inputPath)) return;
  console.log(inputPath);
  const converter = new Converter();
  converter.createInputFromFile(inputPath);
  const converterOutput = converter.createOutputStream({
    f: 'ogg',
    'c:a': 'libopus',
    'b:a': quality,
    map_metadata: '0',
  });

  if (signal?.aborted) converter.kill();
  else signal?.addEventListener('abort', () => converter.kill(), { once: true });

  const running = converter.run();
  running.catch((error) => {
    if (!signal?.aborted) console.error('transcode failed', error);
    converterOutput.destroy(error instanceof Error ? error : new Error('transcode failed'));
  });

  return Readable.toWeb(converterOutput);
}
