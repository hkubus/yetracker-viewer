process.env.HOST ??= process.env.WEB_HOST ?? '127.0.0.1';
process.env.PORT ??= process.env.WEB_PORT ?? '4321';

await import('./dist/server/entry.mjs');
