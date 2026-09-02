import { join } from 'node:path';
import { serve } from '@hono/node-server';
import { sql } from 'drizzle-orm';
import { Hono } from 'hono';
import { cors } from 'hono/cors';
import { HTTPException } from 'hono/http-exception';
import { secureHeaders } from 'hono/secure-headers';
import { apiHost, apiPort, corsOrigins, syncOnStart } from './config.ts';
import { db } from './db/client.ts';
import { downloadCovers, downloadSongs } from './scraper/downloader.ts';
import { importData } from './scraper/importer.ts';
import { backfillDurations } from './util/backfillDurations.ts';
import { loadRoutes } from './util/loadRoutes.ts';
import { refreshPlayableFiles } from './util/playableFiles.ts';
import { repairEraDuplicates } from './util/repairEras.ts';

export { db } from './db/client.ts';

const app = new Hono();
app.use('*', secureHeaders({ crossOriginResourcePolicy: 'cross-origin' }));
app.use(
  '*',
  cors({
    origin: (origin) => {
      if (corsOrigins.includes('*')) return '*';
      return corsOrigins.includes(origin) ? origin : undefined;
    },
    allowMethods: ['GET', 'HEAD', 'OPTIONS'],
    exposeHeaders: ['X-Total-Count'],
  }),
);
app.get('/health', (c) => c.json({ status: 'ok' }));
db.run(sql`CREATE TABLE IF NOT EXISTS eras (
  id INTEGER PRIMARY KEY,
  name TEXT,
  notes TEXT,
  image_url TEXT,
  description TEXT,
  dominant_color TEXT,
  is_main INTEGER NOT NULL DEFAULT 1
)`);
db.run(sql`CREATE TABLE IF NOT EXISTS songs (
  id INTEGER PRIMARY KEY,
  era INTEGER,
  catalog_id TEXT NOT NULL DEFAULT 'unreleased',
  name TEXT,
  notes TEXT,
  file_date INTEGER,
  leak_date INTEGER,
  available_length TEXT,
  track_length INTEGER,
  quality TEXT,
  url TEXT
)`);
db.run(sql`CREATE TABLE IF NOT EXISTS files (
  url TEXT PRIMARY KEY,
  downloaded INTEGER,
  filename TEXT,
  duration REAL
)`);
db.run(sql`CREATE INDEX IF NOT EXISTS songs_era_index ON songs (era)`);
db.run(sql`CREATE INDEX IF NOT EXISTS songs_url_index ON songs (url)`);
const fileColumns = db.all<{ name: string }>(sql`PRAGMA table_info(files)`);
if (!fileColumns.some((column) => column.name === 'duration')) {
  db.run(sql`ALTER TABLE files ADD COLUMN duration REAL`);
}
const eraColumns = db.all<{ name: string }>(sql`PRAGMA table_info(eras)`);
if (!eraColumns.some((column) => column.name === 'is_main')) {
  db.run(sql`ALTER TABLE eras ADD COLUMN is_main INTEGER NOT NULL DEFAULT 1`);
}
const songColumns = db.all<{ name: string }>(sql`PRAGMA table_info(songs)`);
if (!songColumns.some((column) => column.name === 'catalog_id')) {
  db.run(sql`ALTER TABLE songs ADD COLUMN catalog_id TEXT NOT NULL DEFAULT 'unreleased'`);
}
db.run(sql`CREATE INDEX IF NOT EXISTS songs_catalog_index ON songs (catalog_id)`);
db.run(sql`UPDATE eras SET dominant_color = '666666' WHERE dominant_color IS NULL OR trim(dominant_color) = ''`);
await repairEraDuplicates(db);
await refreshPlayableFiles();

const mainDir = import.meta.url.replace('file://', '').split('/').slice(0, -1).join('/');
await loadRoutes(join(mainDir, 'routes'), app);
// app.register(routesPlugin, { path: join(mainDir, 'routes') });
if (syncOnStart) {
  await importData(db);
  void downloadCovers(db).catch((error) => console.error('cover download failed', error));
  const initialDurationBackfill = backfillDurations(db);
  void downloadSongs(db)
    .then(() => initialDurationBackfill)
    .then(() => backfillDurations(db))
    .catch((error) => console.error('background song processing failed', error));
}

app.notFound((c) => c.json({ error: 'Not found' }, 404));
app.onError((error, c) => {
  if (error instanceof HTTPException) return error.getResponse();
  console.error('request failed', error);
  return c.json({ error: 'Internal server error' }, 500);
});

const server = serve({ fetch: app.fetch, hostname: apiHost, port: apiPort }, (info) => {
  console.log(`API listening on http://${apiHost}:${info.port}`);
});

function shutDown(signal: string) {
  console.log(`${signal} received, closing HTTP server`);
  server.close((error) => {
    if (error) {
      console.error('failed to close HTTP server', error);
      process.exitCode = 1;
    }
  });
}

process.once('SIGINT', () => shutDown('SIGINT'));
process.once('SIGTERM', () => shutDown('SIGTERM'));
