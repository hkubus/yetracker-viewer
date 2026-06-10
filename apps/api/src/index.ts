import { join } from 'node:path';
import { serve } from '@hono/node-server';
import { drizzle } from 'drizzle-orm/node-sqlite';
import { Hono } from 'hono';
import { storagePath } from './config.ts';
import { relations } from './db/relations.ts';
import { downloadCovers, downloadSongs } from './scraper/downloader.ts';
import { importData } from './scraper/importer.ts';
import { loadRoutes } from './util/loadRoutes.ts';

const app = new Hono();
export const db = drizzle(join(storagePath, 'db.sqlite3'), { relations });

const mainDir = import.meta.url.replace('file://', '').split('/').slice(0, -1).join('/');
loadRoutes(join(mainDir, 'routes'), app);
// app.register(routesPlugin, { path: join(mainDir, 'routes') });
await importData(db);
downloadCovers(db);
downloadSongs(db);
serve({ fetch: app.fetch, port: 3000 });
