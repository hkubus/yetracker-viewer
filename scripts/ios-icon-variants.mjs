#!/usr/bin/env node
// Derives the dark and tinted iOS app icons from AppIcon.png and writes them next to it in the asset catalog:
//   AppIcon-Dark.png    the record on a transparent background (the system draws the dark backdrop behind it)
//   AppIcon-Tinted.png  grayscale on black, tone-mapped so the record still reads once tinted (white = full tint)
// The art is geometric: a vertical grey gradient behind a record centred on the 1024 px canvas (disc r=404, label
// r=150, spindle hole r=17), every edge anti-aliased over one pixel. That lets the record be cut out exactly, with no
// guesswork. The script checks this model against AppIcon.png first and writes nothing if the art no longer matches
// it (redraw the variants then). Usage: `node scripts/ios-icon-variants.mjs` (Node built-ins only).
import { readFileSync, writeFileSync } from 'node:fs';
import { crc32, deflateSync, inflateSync } from 'node:zlib';

const ICONSET = new URL('../apps/ios/YeTracker/Assets.xcassets/AppIcon.appiconset/', import.meta.url);
const SIZE = 1024;
const CENTER = 512;
const DISC_R = 404;
const LABEL_R = 150;
const HOLE_R = 17;
// Luma -> tinted grey. 13 is the bare vinyl, 58 the brightest sheen, 162..245 the label's gradient. The vinyl becomes a
// mid grey (a plain grayscale copy leaves it near black, so only the label would show up in tint).
const TINT_CURVE = [
  [0, 0],
  [13, 72],
  [58, 165],
  [162, 215],
  [245, 255],
  [255, 255],
];

/** 8-bit, non-interlaced grey/RGB/RGBA PNG -> { width, height, channels, data } (rows of unfiltered samples). */
function decodePng(bytes) {
  let header;
  const idat = [];
  for (let off = 8; off < bytes.length; ) {
    const length = bytes.readUInt32BE(off);
    const type = bytes.toString('latin1', off + 4, off + 8);
    const body = bytes.subarray(off + 8, off + 8 + length);
    if (type === 'IHDR') header = body;
    else if (type === 'IDAT') idat.push(body);
    off += 12 + length;
  }
  const width = header.readUInt32BE(0);
  const height = header.readUInt32BE(4);
  const channels = { 0: 1, 2: 3, 6: 4 }[header[9]];
  if (header[8] !== 8 || !channels || header[12] !== 0) throw new Error('expected an 8-bit, non-interlaced PNG');
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const data = Buffer.alloc(height * stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const row = y * stride;
    for (let i = 0; i < stride; i++) {
      const left = i >= channels ? data[row + i - channels] : 0;
      const up = y > 0 ? data[row - stride + i] : 0;
      const upLeft = i >= channels && y > 0 ? data[row - stride + i - channels] : 0;
      const p = left + up - upLeft;
      const [pl, pu, pul] = [Math.abs(p - left), Math.abs(p - up), Math.abs(p - upLeft)];
      const paeth = pl <= pu && pl <= pul ? left : pu <= pul ? up : upLeft;
      const prediction = [0, left, up, (left + up) >> 1, paeth][filter];
      data[row + i] = (raw[y * (stride + 1) + 1 + i] + prediction) & 0xff;
    }
  }
  return { width, height, channels, data };
}

function encodePng({ width, height, channels, data }) {
  const stride = width * channels;
  const raw = Buffer.alloc(height * (stride + 1)); // filter byte 0 (none) on every row
  for (let y = 0; y < height; y++) data.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  const chunk = (type, body) => {
    const out = Buffer.alloc(body.length + 12);
    out.writeUInt32BE(body.length, 0);
    out.write(type, 4, 'latin1');
    body.copy(out, 8);
    out.writeUInt32BE(crc32(out.subarray(4, 8 + body.length)), 8 + body.length);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = { 3: 2, 4: 6 }[channels];
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

const distance = (x, y) => Math.hypot(x + 0.5 - CENTER, y + 0.5 - CENTER);
/** How much of the pixel at `d` a circle of radius `r` covers; the original's edges ramp over one pixel like this. */
const coverage = (d, r) => Math.min(1, Math.max(0, r + 0.5 - d));
const luma = (r, g, b) => 0.2126 * r + 0.7152 * g + 0.0722 * b;
const byte = (v) => Math.max(0, Math.min(255, Math.round(v)));
const tintCurve = (v) => {
  for (let i = 1; i < TINT_CURVE.length; i++) {
    const [[x0, y0], [x1, y1]] = [TINT_CURVE[i - 1], TINT_CURVE[i]];
    if (v <= x1) return y0 + ((v - x0) / (x1 - x0)) * (y1 - y0);
  }
  return 255;
};

const icon = decodePng(readFileSync(new URL('AppIcon.png', ICONSET)));
if (icon.width !== SIZE || icon.height !== SIZE || icon.channels !== 3) {
  throw new Error('expected a 1024x1024 RGB icon');
}
const pixel = (x, y) => [...icon.data.subarray((y * SIZE + x) * 3, (y * SIZE + x) * 3 + 3)];
const labelLuma = (y) => luma(...pixel(CENTER + 25, y)); // the label's gradient is vertical; x=537 clears the hole
const background = (y) => pixel(0, y); // the left column lies outside the record

// Label, the gap around it and the hole, probed on 16 rays.
for (let k = 0; k < 16; k++) {
  const t = (k * Math.PI) / 8;
  const probe = (r) => luma(...pixel(Math.floor(CENTER + r * Math.cos(t)), Math.floor(CENTER + r * Math.sin(t))));
  if (probe(HOLE_R - 6) > 40 || probe(HOLE_R + 6) < 150 || probe(LABEL_R - 4) < 150 || probe(LABEL_R + 5) > 20) {
    throw new Error('AppIcon.png no longer matches the record geometry this script assumes');
  }
}

// The record with an exact circular alpha. Edge pixels are blends with the background, so they take the colour of the
// first fully covered pixel further in on the same ray.
const record = Buffer.alloc(SIZE * SIZE * 4);
for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    const d = distance(x, y);
    const alpha = coverage(d, DISC_R);
    if (alpha === 0) continue;
    const k = alpha < 1 ? (d - (DISC_R - 1.5)) / d : 0;
    const rgb = pixel(Math.floor(x + 0.5 - (x + 0.5 - CENTER) * k), Math.floor(y + 0.5 - (y + 0.5 - CENTER) * k));
    record.set([...rgb, byte(alpha * 255)], (y * SIZE + x) * 4);
  }
}

// The cut-out laid back over the background must reproduce the icon.
let worst = 0;
for (let y = 0; y < SIZE; y++) {
  const bg = background(y);
  for (let x = 0; x < SIZE; x++) {
    const i = y * SIZE + x;
    const alpha = record[i * 4 + 3] / 255;
    for (let c = 0; c < 3; c++) {
      const composite = record[i * 4 + c] * alpha + bg[c] * (1 - alpha);
      worst = Math.max(worst, Math.abs(composite - icon.data[i * 3 + c]));
    }
  }
}
if (worst > 2) throw new Error(`AppIcon.png is not a record on a plain background (off by ${worst.toFixed(1)}/255)`);

const tinted = Buffer.alloc(SIZE * SIZE * 3);
for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    const i = y * SIZE + x;
    const d = distance(x, y);
    let v = tintCurve(luma(record[i * 4], record[i * 4 + 1], record[i * 4 + 2]));
    // The hole goes black (no tint) instead of the grey its dark fill would map to.
    if (d < HOLE_R + 1.5) v = tintCurve(labelLuma(y)) * (1 - coverage(d, HOLE_R));
    tinted.fill(byte((v * record[i * 4 + 3]) / 255), i * 3, i * 3 + 3);
  }
}

const outputs = [
  ['AppIcon-Dark.png', { width: SIZE, height: SIZE, channels: 4, data: record }],
  ['AppIcon-Tinted.png', { width: SIZE, height: SIZE, channels: 3, data: tinted }],
];
for (const [name, image] of outputs) {
  writeFileSync(new URL(name, ICONSET), encodePng(image));
  console.log(`wrote ${new URL(name, ICONSET).pathname}`);
}
console.log(`cut-out matches AppIcon.png to ${worst.toFixed(1)}/255`);
