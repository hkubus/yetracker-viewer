import { Readable } from 'node:stream';
import { Converter } from 'ffmpeg-stream';
export async function transcode(inputPath: string, quality: string = '128k') {
  const converter = new Converter();
  converter.createInputFromFile(inputPath);
  const converterOutput = converter.createOutputStream({
    f: 'opus',
    'b:a': quality,
  });
  converter.run();
  return Readable.toWeb(converterOutput);
  // return converterOutput;
}
