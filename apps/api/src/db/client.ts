import { join } from 'node:path';
import { sql } from 'drizzle-orm';
import { drizzle } from 'drizzle-orm/node-sqlite';
import { storagePath } from '../config.ts';
import { relations } from './relations.ts';

export const db = drizzle(join(storagePath, 'db.sqlite3'), { relations });

// Concurrency/durability tuning for a multi-handle SQLite workload
// (HTTP handlers + background sync loops share this file).
db.run(sql`PRAGMA journal_mode = WAL`);
db.run(sql`PRAGMA busy_timeout = 5000`);
db.run(sql`PRAGMA synchronous = NORMAL`);
db.run(sql`PRAGMA foreign_keys = ON`);

export function closeDb() {
  db.$client.close();
}
