import { join } from 'node:path';
import { drizzle } from 'drizzle-orm/node-sqlite';
import { storagePath } from '../config.ts';
import { relations } from './relations.ts';

export const db = drizzle(join(storagePath, 'db.sqlite3'), { relations });
