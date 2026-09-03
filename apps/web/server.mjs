// Single source of truth for host/port precedence:
// WEB_HOST/WEB_PORT first, then generic HOST/PORT, then loopback defaults.
// Env vars are read (not mutated with ??=) so the effective values stay visible here.
const host = process.env.WEB_HOST ?? process.env.HOST ?? '127.0.0.1';
const port = process.env.WEB_PORT ?? process.env.PORT ?? '4321';

if (process.env.HOST === undefined) {
  process.env.HOST = host;
}
if (process.env.PORT === undefined) {
  process.env.PORT = port;
}

function onFatal(error) {
  console.error('[web] fatal error:', error);
  process.exitCode = 1;
}

process.on('unhandledRejection', onFatal);
process.on('uncaughtException', onFatal);

let shuttingDown = false;
function shutdown(signal) {
  if (shuttingDown) return;
  shuttingDown = true;
  console.log(`[web] received ${signal}, shutting down…`);
  // Give the standalone server a beat to close keep-alive connections.
  setTimeout(() => process.exit(0), 1000).unref();
}

process.on('SIGTERM', () => shutdown('SIGTERM'));
process.on('SIGINT', () => shutdown('SIGINT'));

try {
  await import('./dist/server/entry.mjs');
} catch (error) {
  onFatal(error);
}
