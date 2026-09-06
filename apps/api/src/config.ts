import { existsSync, mkdirSync } from 'node:fs';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function findWorkspaceRoot(startDir: string): string {
  let dir = startDir;
  while (true) {
    if (existsSync(join(dir, 'package.json'))) return dir;
    const parent = dirname(dir);
    if (parent === dir) return startDir;
    dir = parent;
  }
}

function resolveWorkspaceRoot(): string {
  try {
    // import.meta.dirname points at src/ in dev and dist/src/ after build;
    // walk up until package.json is found so both layouts resolve correctly.
    const here =
      typeof import.meta.dirname === 'string' ? import.meta.dirname : dirname(fileURLToPath(import.meta.url));
    return findWorkspaceRoot(resolve(here, '../../..'));
  } catch {
    return process.cwd();
  }
}

const workspaceRoot = resolveWorkspaceRoot();
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

function readPositiveInteger(value: string | undefined, fallback: number, name: string, max = 1000) {
  if (value === undefined || value === '') return fallback;
  if (!/^\d+$/.test(value)) throw new Error(`${name} must be a positive integer, received: ${value}`);

  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1 || parsed > max) {
    throw new Error(`${name} must be a positive integer between 1 and ${max}, received: ${value}`);
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
    let parsed: URL;
    try {
      parsed = new URL(origin);
    } catch {
      throw new Error(`CORS_ORIGINS must contain exact HTTP(S) origins, received: ${origin}`);
    }
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.origin !== origin) {
      throw new Error(`CORS_ORIGINS must contain exact HTTP(S) origins, received: ${origin}`);
    }
  }
  return origins;
}

const rawApiHost = process.env.API_HOST ?? process.env.HOST ?? '127.0.0.1';
export const apiHost = rawApiHost.trim();
if (apiHost.length === 0) {
  throw new Error('API_HOST must be a non-empty hostname or IP address');
}
export const apiPort = readPort(process.env.API_PORT ?? process.env.PORT, 3000);
export const corsOrigins = readOrigins(process.env.CORS_ORIGINS);
export const syncOnStart = !['0', 'false', 'no'].includes((process.env.SYNC_ON_START ?? 'true').trim().toLowerCase());
export const youtubeDownload = !['0', 'false', 'no'].includes(
  (process.env.YOUTUBE_DOWNLOAD ?? 'true').trim().toLowerCase(),
);
export const maxConcurrentTranscodes = readPositiveInteger(
  process.env.MAX_CONCURRENT_TRANSCODES,
  2,
  'MAX_CONCURRENT_TRANSCODES',
  100,
);

try {
  mkdirSync(storagePath, { recursive: true });
  mkdirSync(songsPath, { recursive: true });
  mkdirSync(join(storagePath, 'covers'), { recursive: true });
} catch (error) {
  throw new Error(
    `Failed to create storage directories (STORAGE_DIR=${storageDir} resolved to ${storagePath}): ${error instanceof Error ? error.message : String(error)}`,
  );
}
