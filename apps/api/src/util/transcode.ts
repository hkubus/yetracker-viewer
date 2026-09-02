import { existsSync } from 'node:fs';
import { Readable } from 'node:stream';
import { Converter } from 'ffmpeg-stream';
export async function transcode(inputPath: string, quality: string = '128k') {
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
  converter.run();
  return Readable.toWeb(converterOutput);
  // return converterOutput;
}
