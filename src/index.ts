import { join } from 'node:path';
import { drizzle } from 'drizzle-orm/node-sqlite';
import Fastify from 'fastify';
import { relations } from './db/relations.ts';
import { downloader } from './scraper/downloader.ts';
import { importData } from './scraper/importer.ts';
import { routesPlugin } from './util/loadRoutes.ts';

const app = Fastify();
const db = drizzle(process.env.DB_FILENAME as string, { relations });

app.addHook('onRequest', (req, _res, done) => {
  req.db = db;
  done();
});
const mainDir = import.meta.url.replace('file://', '').split('/').slice(0, -1).join('/');
app.register(routesPlugin, { path: join(mainDir, 'routes') });
await importData(db);
downloader(db);
app
  .listen({
    port: 3000,
  })
  .then(() => console.log('Server running on http://localhost:3000'));
