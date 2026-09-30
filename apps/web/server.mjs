/**
 * Production entry point (`pnpm start`, after `pnpm build`). It validates the environment, then serves the built app
 * (`dist/`) through its own HTTP server: the @astrojs/node handler does the routing and static files; this file adds
 * gzip/brotli compression, long-lived caching of the hashed `/_astro/*` assets, and a graceful shutdown.
 *
 * The build bundles every dependency (`dist/server` imports only `node:*` modules), so at runtime it needs only
 * `dist/`, this file and `env.mjs`: no `src/`, no `node_modules` (`package.json` just provides `pnpm start`).
 *
 * Environment: WEB_HOST/WEB_PORT (falling back to HOST/PORT, then 127.0.0.1:4321), PUBLIC_API_URL,
 * API_INTERNAL_URL, SITE_URL, TRUST_PROXY (see env.mjs), and optionally SERVER_CERT_PATH + SERVER_KEY_PATH to serve
 * HTTPS directly.
 */
import fs from 'node:fs';
import http from 'node:http';
import https from 'node:https';
import zlib from 'node:zlib';
import { EnvError, readRuntimeEnv, readServerEnv, runtimeEnvWarnings } from './env.mjs';

const SHUTDOWN_TIMEOUT_MS = 10_000;
const IMMUTABLE_CACHE_CONTROL = 'public, max-age=31536000, immutable';
const PUBLIC_FILE_CACHE_CONTROL = 'public, max-age=86400';
/** Bodies smaller than this are sent as is: compression would save little and cost a round of CPU. */
const MIN_COMPRESSIBLE_BYTES = 1024;
const COMPRESSIBLE_TYPE =
  /^\s*(?:text\/|application\/(?:javascript|ecmascript|json|xml|manifest\+json|[\w.-]+\+(?:json|xml))|image\/(?:svg\+xml|x-icon|vnd\.microsoft\.icon))/i;

const log = (message) => console.log(`[web] ${message}`);
const logWarning = (message) => console.warn(`[web] warning: ${message}`);
const logError = (message, error) => console.error(`[web] ${message}`, ...(error === undefined ? [] : [error]));

// 1. Configuration — fail fast, before the app is loaded, reporting every problem at once.
let listen;
let runtimeEnv;
let tls;
const configProblems = [];
for (const readConfig of [
  () => {
    listen = readServerEnv(process.env);
  },
  () => {
    runtimeEnv = readRuntimeEnv(process.env, { production: true });
  },
  () => {
    tls = readTlsEnv(process.env);
  },
]) {
  try {
    readConfig();
  } catch (error) {
    if (!(error instanceof EnvError)) throw error;
    configProblems.push(...error.problems);
  }
}
if (configProblems.length > 0) {
  logError(new EnvError(configProblems).message);
  process.exit(1);
}
for (const warning of runtimeEnvWarnings(runtimeEnv)) logWarning(warning);

/** Both SERVER_CERT_PATH and SERVER_KEY_PATH, or neither. */
function readTlsEnv(env) {
  const cert = env.SERVER_CERT_PATH?.trim();
  const key = env.SERVER_KEY_PATH?.trim();
  if (!cert && !key) return null;
  if (!cert || !key) throw new EnvError(['SERVER_CERT_PATH and SERVER_KEY_PATH must be set together.']);
  try {
    return { cert: fs.readFileSync(cert), key: fs.readFileSync(key) };
  } catch (error) {
    throw new EnvError([`Could not read the TLS certificate or key: ${error.message}`]);
  }
}

// 2. Crash handling. After an uncaught exception the process state is unknown: stop accepting work, let open
// requests finish (bounded by the shutdown deadline) and exit non-zero.
let server;
let shuttingDown = false;

process.on('uncaughtException', (error) => {
  logError('uncaught exception:', error);
  if (server) shutdown('uncaughtException', 1);
  else process.exit(1);
});
// Astro handles rejections raised while rendering a request; anything else is logged here instead of vanishing.
process.on('unhandledRejection', (reason) => {
  logError('unhandled promise rejection:', reason);
});

// 3. The app. The adapter's standalone mode would start its own server on import; this file owns the server.
process.env.ASTRO_NODE_AUTOSTART = 'disabled';
let astroHandler;
let clientDir;
try {
  ({ handler: astroHandler } = await import('./dist/server/entry.mjs'));
  if (typeof astroHandler !== 'function') throw new Error('dist/server/entry.mjs does not export a request handler');
  clientDir = new URL('./dist/client/', import.meta.url);
} catch (error) {
  logError('could not load the built app (run `pnpm build` first):', error);
  process.exit(1);
}

/** Top-level files copied from public/ (favicons…): cacheable for a day, they are not content-hashed. */
const publicFiles = new Set(
  fs
    .readdirSync(clientDir, { withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => `/${entry.name}`),
);

function pathnameOf(url = '/') {
  const end = url.search(/[?#]/);
  return end === -1 ? url : url.slice(0, end);
}

function appendVary(res, value) {
  const current = res.getHeader('vary');
  const list = Array.isArray(current) ? current.join(', ') : current ? String(current) : '';
  const tokens = list.split(',').map((token) => token.trim().toLowerCase());
  if (tokens.includes('*') || tokens.includes(value.toLowerCase())) return;
  res.setHeader('Vary', list ? `${list}, ${value}` : value);
}

/** The best coding the client accepts: `br`, then `gzip`, else `null` (identity). */
function negotiateEncoding(acceptEncoding) {
  if (!acceptEncoding) return null;
  const qualities = new Map();
  for (const part of String(acceptEncoding).split(',')) {
    const [name, ...params] = part.split(';');
    const coding = name.trim().toLowerCase();
    if (!coding) continue;
    let quality = 1;
    for (const param of params) {
      const [key, value] = param.split('=');
      if (key.trim().toLowerCase() === 'q') {
        const parsed = Number(value);
        quality = Number.isFinite(parsed) ? parsed : 0;
      }
    }
    qualities.set(coding, quality);
  }
  const qualityOf = (coding) => qualities.get(coding) ?? qualities.get('*') ?? 0;
  const br = qualityOf('br');
  const gzip = Math.max(qualityOf('gzip'), qualities.get('x-gzip') ?? 0);
  if (br > 0 && br >= gzip) return 'br';
  if (gzip > 0) return 'gzip';
  return null;
}

function createEncoder(encoding, sizeHint) {
  if (encoding === 'br') {
    const params = {
      [zlib.constants.BROTLI_PARAM_MODE]: zlib.constants.BROTLI_MODE_TEXT,
      // Quality 5: close to the best ratio for text at a fraction of the CPU cost of the default (11).
      [zlib.constants.BROTLI_PARAM_QUALITY]: 5,
    };
    if (sizeHint > 0) params[zlib.constants.BROTLI_PARAM_SIZE_HINT] = sizeHint;
    return zlib.createBrotliCompress({ params });
  }
  return zlib.createGzip({ level: 6 });
}

/** Copies the headers given to `writeHead()` onto the response so they can be inspected and changed first. */
function mergeWriteHeadHeaders(res, headers) {
  if (!headers) return;
  if (Array.isArray(headers)) {
    if (Array.isArray(headers[0])) {
      for (const [name, value] of headers) res.setHeader(name, value);
    } else {
      for (let index = 0; index + 1 < headers.length; index += 2) res.setHeader(headers[index], headers[index + 1]);
    }
    return;
  }
  for (const [name, value] of Object.entries(headers)) {
    if (value !== undefined) res.setHeader(name, value);
  }
}

/**
 * Wraps `res` so that, once the status and headers are known, text responses (HTML, CSS, JS, JSON, SVG…) are
 * compressed with the best coding the client accepts. Also sets the caching headers of static files.
 */
function prepareResponse(req, res) {
  const pathname = pathnameOf(req.url);
  const isAsset = pathname.startsWith('/_astro/');
  const isPublicFile = publicFiles.has(pathname);
  const encoding = req.method === 'HEAD' ? null : negotiateEncoding(req.headers['accept-encoding']);

  const writeHead = res.writeHead;
  const write = res.write;
  const end = res.end;
  let decided = false;
  let encoder = null;
  let flushPending = false;

  function decide() {
    decided = true;
    const status = res.statusCode;
    if ((isAsset || isPublicFile) && (status === 200 || status === 206 || status === 304)) {
      res.setHeader('Cache-Control', isAsset ? IMMUTABLE_CACHE_CONTROL : PUBLIC_FILE_CACHE_CONTROL);
    }
    const type = String(res.getHeader('content-type') ?? '');
    const compressible = COMPRESSIBLE_TYPE.test(type);
    if (compressible || status === 304) appendVary(res, 'Accept-Encoding');
    if (!compressible || !encoding) return;
    if (status < 200 || status === 204 || status === 206 || status === 304) return;
    if (res.hasHeader('content-encoding')) return;
    if (/\bno-transform\b/i.test(String(res.getHeader('cache-control') ?? ''))) return;
    const length = Number(res.getHeader('content-length'));
    if (res.hasHeader('content-length') && length < MIN_COMPRESSIBLE_BYTES) return;

    // A strong ETag names the identity bytes; the compressed body is a different representation.
    const etag = res.getHeader('etag');
    if (typeof etag === 'string' && !etag.startsWith('W/')) res.setHeader('ETag', `W/${etag}`);
    res.removeHeader('content-length');
    res.setHeader('Content-Encoding', encoding);

    encoder = createEncoder(encoding, Number.isFinite(length) ? length : 0);
    encoder.on('data', (chunk) => {
      if (write.call(res, chunk) === false) encoder.pause();
    });
    encoder.on('end', () => end.call(res));
    encoder.on('error', (error) => {
      logError(`compression failed for ${req.url}:`, error);
      res.destroy(error);
    });
    encoder.on('drain', () => res.emit('drain'));
    res.on('drain', () => encoder.resume());
    res.on('close', () => encoder.destroy());
  }

  // Streamed pages (SSR) are written in many small chunks: flush the encoder whenever the writer pauses, so the
  // browser gets the <head> early instead of waiting for zlib's buffer to fill.
  function scheduleFlush() {
    if (flushPending) return;
    flushPending = true;
    setImmediate(() => {
      flushPending = false;
      if (encoder && !encoder.destroyed && !encoder.writableEnded) {
        encoder.flush(encoding === 'gzip' ? zlib.constants.Z_SYNC_FLUSH : zlib.constants.BROTLI_OPERATION_FLUSH);
      }
    });
  }

  res.writeHead = function patchedWriteHead(statusCode, ...rest) {
    if (decided) return writeHead.call(this, statusCode, ...rest);
    const [reasonOrHeaders, maybeHeaders] = rest;
    const reason = typeof reasonOrHeaders === 'string' ? reasonOrHeaders : undefined;
    mergeWriteHeadHeaders(this, reason === undefined ? reasonOrHeaders : maybeHeaders);
    this.statusCode = statusCode;
    decide();
    return reason === undefined ? writeHead.call(this, statusCode) : writeHead.call(this, statusCode, reason);
  };

  res.write = function patchedWrite(chunk, chunkEncoding, callback) {
    if (!this.headersSent) this.writeHead(this.statusCode);
    if (!encoder) return write.call(this, chunk, chunkEncoding, callback);
    scheduleFlush();
    return encoder.write(chunk, chunkEncoding, callback);
  };

  res.end = function patchedEnd(chunk, chunkEncoding, callback) {
    if (typeof chunk === 'function') {
      callback = chunk;
      chunk = undefined;
      chunkEncoding = undefined;
    } else if (typeof chunkEncoding === 'function') {
      callback = chunkEncoding;
      chunkEncoding = undefined;
    }
    if (!this.headersSent) {
      // A body sent in one piece has a known size: small ones are not worth compressing.
      if (chunk != null && !this.hasHeader('content-length') && !this.hasHeader('transfer-encoding')) {
        this.setHeader('Content-Length', Buffer.byteLength(chunk, chunkEncoding));
      }
      this.writeHead(this.statusCode);
    }
    if (!encoder) return end.call(this, chunk, chunkEncoding, callback);
    if (callback) this.once('finish', callback);
    if (chunk != null) encoder.end(chunk, chunkEncoding);
    else encoder.end();
    return this;
  };
}

/**
 * Baseline headers for every response. SSR pages get the full set (CSP with the API origin, HSTS…) from
 * src/middleware.ts, which overrides these; static files and Astro's own redirects only get these.
 */
const BASELINE_HEADERS = {
  'X-Content-Type-Options': 'nosniff',
  'X-Frame-Options': 'DENY',
  'Referrer-Policy': 'strict-origin-when-cross-origin',
  'Cross-Origin-Opener-Policy': 'same-origin',
  // Static files are scripts, styles, fonts and images: none of them needs to load anything as a document.
  'Content-Security-Policy': "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'",
};

function onRequest(req, res) {
  // Requests that arrive on kept-alive connections while shutting down are served, then the connection closes.
  if (shuttingDown) res.setHeader('Connection', 'close');
  for (const [name, value] of Object.entries(BASELINE_HEADERS)) res.setHeader(name, value);
  // The site is read-only: nothing but GET and HEAD reaches the app (or its cache policy).
  if (req.method !== 'GET' && req.method !== 'HEAD') {
    res.writeHead(405, {
      Allow: 'GET, HEAD',
      'Cache-Control': 'no-store',
      'Content-Type': 'text/plain; charset=utf-8',
    });
    res.end('Method Not Allowed\n');
    req.resume();
    return;
  }
  prepareResponse(req, res);
  astroHandler(req, res);
}

// 4. Server.
const serverOptions = {
  // Longer than the usual 60 s idle timeout of load balancers, so they never reuse a connection Node just closed.
  keepAliveTimeout: 65_000,
  headersTimeout: 30_000,
  requestTimeout: 60_000,
  // Recycle long-lived connections now and then so one client cannot pin a socket forever.
  maxRequestsPerSocket: 1000,
};
server = tls
  ? https.createServer({ ...serverOptions, cert: tls.cert, key: tls.key }, onRequest)
  : http.createServer(serverOptions, onRequest);

server.on('error', (error) => {
  logError(`could not listen on ${listen.host}:${listen.port}: ${error.message}`);
  process.exit(1);
});
server.listen(listen.port, listen.host, () => {
  const address = server.address();
  const port = typeof address === 'object' && address ? address.port : listen.port;
  const host = listen.host.includes(':') ? `[${listen.host}]` : listen.host;
  log(`listening on ${tls ? 'https' : 'http'}://${host}:${port}`);
});

// 5. Graceful shutdown: stop accepting connections, let open requests finish, close idle keep-alive connections,
// and give up after SHUTDOWN_TIMEOUT_MS. A second signal exits immediately.
function shutdown(reason, exitCode = 0) {
  if (exitCode !== 0) process.exitCode = exitCode;
  if (shuttingDown) return;
  shuttingDown = true;
  log(`${reason}: shutting down (waiting up to ${SHUTDOWN_TIMEOUT_MS / 1000} s for open requests)`);
  const exit = () => process.exit(process.exitCode ?? 0);
  server.close(() => {
    log('all connections closed');
    exit();
  });
  server.closeIdleConnections();
  setInterval(() => server.closeIdleConnections(), 250).unref();
  setTimeout(() => {
    log('shutdown deadline reached: closing the remaining connections');
    server.closeAllConnections();
    exit();
  }, SHUTDOWN_TIMEOUT_MS).unref();
}

for (const signal of ['SIGTERM', 'SIGINT']) {
  process.on(signal, () => {
    if (shuttingDown) {
      log(`${signal} again: exiting now`);
      process.exit(process.exitCode ?? 1);
    }
    shutdown(signal);
  });
}
