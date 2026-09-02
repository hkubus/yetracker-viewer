import { mkdirSync } from 'node:fs';
import { isAbsolute, join, resolve } from 'node:path';

const workspaceRoot = resolve(import.meta.dirname, '../../..');
const storageDir = process.env.STORAGE_DIR ?? 'storage';

export const storagePath = isAbsolute(storageDir) ? storageDir : resolve(workspaceRoot, storageDir);
const songsDir = process.env.SONGS_DIR || join(storagePath, 'songs');
export const songsPath = isAbsolute(songsDir) ? songsDir : resolve(workspaceRoot, songsDir);

function readPort(value: string | undefined, fallback: number) {
  if (value === undefined || value === '') return fallback;
  if (!/^\d+$/.test(value)) throw new Error(`Invalid API port: ${value}`);

  const port = Number(value);
  if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) {
    throw new Error(`API port must be between 1 and 65535, received: ${value}`);
  }
  return port;
}

function readPositiveInteger(value: string | undefined, fallback: number, name: string) {
  if (value === undefined || value === '') return fallback;
  if (!/^\d+$/.test(value)) throw new Error(`${name} must be a positive integer`);

  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1) {
    throw new Error(`${name} must be a positive integer`);
  }
  return parsed;
}

function readOrigins(value: string | undefined) {
  const origins = (value ?? 'http://localhost:4321,http://127.0.0.1:4321')
    .split(',')
    .map((origin) => origin.trim())
    .filter(Boolean);

  for (const origin of origins) {
    if (origin === '*') continue;
    const parsed = new URL(origin);
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.origin !== origin) {
      throw new Error(`CORS_ORIGINS must contain exact HTTP(S) origins, received: ${origin}`);
    }
  }
  return origins;
}

export const apiHost = (process.env.API_HOST ?? process.env.HOST ?? '127.0.0.1').trim();
export const apiPort = readPort(process.env.API_PORT ?? process.env.PORT, 3000);
export const corsOrigins = readOrigins(process.env.CORS_ORIGINS);
export const syncOnStart = !['0', 'false', 'no'].includes((process.env.SYNC_ON_START ?? 'true').toLowerCase());
export const maxConcurrentTranscodes = readPositiveInteger(
  process.env.MAX_CONCURRENT_TRANSCODES,
  2,
  'MAX_CONCURRENT_TRANSCODES',
);

mkdirSync(storagePath, { recursive: true });
mkdirSync(songsPath, { recursive: true });
mkdirSync(join(storagePath, 'covers'), { recursive: true });
