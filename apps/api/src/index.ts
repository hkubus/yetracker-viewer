import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { serve } from '@hono/node-server';
import { sql } from 'drizzle-orm';
import { Hono } from 'hono';
import { cors } from 'hono/cors';
import { HTTPException } from 'hono/http-exception';
import { secureHeaders } from 'hono/secure-headers';
import { apiHost, apiPort, corsOrigins, syncOnStart } from './config.ts';
import { closeDb, db } from './db/client.ts';
import { downloadCovers, downloadSongs } from './scraper/downloader.ts';
import { importData } from './scraper/importer.ts';
import { backfillDurations } from './util/backfillDurations.ts';
import { loadRoutes } from './util/loadRoutes.ts';
import { refreshPlayableFiles } from './util/playableFiles.ts';
import { repairEraDuplicates } from './util/repairEras.ts';

export { db } from './db/client.ts';

// AbortController for background loops: signalled on shutdown so chained
// background phases stop scheduling new work.
const shutdownController = new AbortController();

const app = new Hono();
// CORP 'cross-origin' (instead of the default 'same-origin') so audio/covers
// served here remain embeddable by the web frontend on a different origin.
// Revisit if the API ever serves untrusted HTML that needs stricter isolation.
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
try {
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

  const mainDir = dirname(fileURLToPath(import.meta.url));
  await loadRoutes(join(mainDir, 'routes'), app);
  if (syncOnStart) {
    await importData(db);
    // Sequential background sync with a single error boundary: each phase is
    // awaited in order (no floating promise chains with an unhandled
    // rejection window), and phases bail out early once shutdown is requested.
    const backgroundSync = (async () => {
      try {
        await downloadCovers(db);
        if (shutdownController.signal.aborted) return;
        await backfillDurations(db);
        if (shutdownController.signal.aborted) return;
        await downloadSongs(db);
        if (shutdownController.signal.aborted) return;
        await backfillDurations(db);
      } catch (error) {
        if (!shutdownController.signal.aborted) console.error('background song processing failed', error);
      }
    })();
    // Safety net: the body above already catches; this guards against an
    // unexpected throw escaping the handler itself.
    backgroundSync.catch((error) => console.error('background song processing failed', error));
  }
} catch (error) {
  console.error('API startup failed', error);
  try {
    closeDb();
  } catch (closeError) {
    console.error('failed to close database during startup failure', closeError);
  }
  process.exit(1);
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

let shuttingDown = false;

function shutDown(signal: string) {
  if (shuttingDown) {
    console.error(`${signal} received during shutdown, forcing exit`);
    process.exit(1);
  }
  shuttingDown = true;
  console.log(`${signal} received, closing HTTP server`);
  // Stop background loops from scheduling further phases.
  shutdownController.abort();
  server.close((error) => {
    if (error) {
      console.error('failed to close HTTP server', error);
      process.exitCode = 1;
    }
    try {
      closeDb();
    } catch (dbError) {
      console.error('failed to close database', dbError);
      process.exitCode = 1;
    }
  });
}

process.on('SIGINT', () => shutDown('SIGINT'));
process.on('SIGTERM', () => shutDown('SIGTERM'));
