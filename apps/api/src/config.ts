import { mkdirSync } from 'node:fs';
import { isAbsolute, join, resolve } from 'node:path';

const workspaceRoot = resolve(import.meta.dirname, '../../..');
const storageDir = process.env.STORAGE_DIR ?? 'storage';

export const storagePath = isAbsolute(storageDir) ? storageDir : resolve(workspaceRoot, storageDir);

mkdirSync(storagePath, { recursive: true });
mkdirSync(join(storagePath, 'songs'), { recursive: true });
mkdirSync(join(storagePath, 'covers'), { recursive: true });
