import { join } from 'node:path';
import { drizzle } from 'drizzle-orm/node-sqlite';
import Fastify from 'fastify';
import { storagePath } from './config.ts';
import { relations } from './db/relations.ts';
import { downloadCovers, downloadSongs } from './scraper/downloader.ts';
import { importData } from './scraper/importer.ts';
import { routesPlugin } from './util/loadRoutes.ts';

const app = Fastify();
const db = drizzle(join(storagePath, 'db.sqlite3'), { relations });

app.addHook('onRequest', (req, _res, done) => {
  req.db = db;
  console.log('test');
  done();
});
const mainDir = import.meta.url.replace('file://', '').split('/').slice(0, -1).join('/');
app.register(routesPlugin, { path: join(mainDir, 'routes') });
await importData(db);
downloadCovers(db);
downloadSongs(db);
app
  .listen({
    port: 3000,
  })
  .then(() => console.log('Server running on http://localhost:3000'));
