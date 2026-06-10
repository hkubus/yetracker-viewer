import { Canvas, type Image } from '@napi-rs/canvas';

export async function getDominantColor(image: Image) {
  const canvas = new Canvas(64, 64);
  const ctx = canvas.getContext('2d');
  ctx.filter = 'blur(64px)';
  ctx.drawImage(image, image.width * 0.9, 0, image.width * 0.1, image.height, 0, 0, 64, 64);
  return Array.from(ctx.getImageData(32, 0, 1, 1).data.slice(0, 3));
}
