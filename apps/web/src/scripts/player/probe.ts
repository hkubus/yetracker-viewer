/**
 * `<audio>` hides HTTP statuses: every failed request surfaces as the same media error. After a failure the player
 * asks the API about the same URL with a HEAD request (cheap: it never starts a transcode) to decide between
 * retrying, falling back to the original file and giving up.
 */

export type Probe =
  /** The URL works now. `cached` = a finished file (known length, seekable), not a live transcode. */
  | { kind: 'ok'; cached: boolean }
  /** No audio file for this song. */
  | { kind: 'missing' }
  /** Temporarily overloaded (503 with `Retry-After`, in seconds). */
  | { kind: 'busy'; retryAfter: number }
  /** Not available at all right now (503 without `Retry-After`, e.g. no transcoder installed). */
  | { kind: 'unavailable' }
  | { kind: 'server'; status: number }
  /** No usable answer (offline, timeout, CORS). */
  | { kind: 'network' };

interface HeaderReader {
  get(name: string): string | null;
}

/** `Retry-After` as seconds (delta-seconds or an HTTP date); null when absent or invalid. */
export function parseRetryAfter(value: string | null | undefined, now = Date.now()): number | null {
  const text = value?.trim() ?? '';
  if (/^\d+$/.test(text)) return Number(text);
  if (!text) return null;
  const date = Date.parse(text);
  return Number.isFinite(date) ? Math.max(0, Math.ceil((date - now) / 1000)) : null;
}

export function classifyResponse(status: number, headers: HeaderReader): Probe {
  if (status >= 200 && status < 300) return { kind: 'ok', cached: headers.get('content-length') !== null };
  if (status === 404 || status === 410) return { kind: 'missing' };
  if (status === 503) {
    const retryAfter = parseRetryAfter(headers.get('retry-after'));
    return retryAfter === null ? { kind: 'unavailable' } : { kind: 'busy', retryAfter };
  }
  return { kind: 'server', status };
}

/** An AbortSignal that aborts when any of `signals` does (`AbortSignal.any` where available). */
export function anySignal(signals: AbortSignal[]): AbortSignal {
  if (typeof AbortSignal.any === 'function') return AbortSignal.any(signals);
  const controller = new AbortController();
  for (const signal of signals) {
    if (signal.aborted) {
      controller.abort(signal.reason);
      break;
    }
    signal.addEventListener('abort', () => controller.abort(signal.reason), { once: true });
  }
  return controller.signal;
}

/** Resolves after `ms`, or early when `signal` aborts (it never rejects: callers check `signal.aborted`). */
export function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) return resolve();
    const timer = setTimeout(done, ms);
    signal.addEventListener('abort', done, { once: true });
    function done() {
      clearTimeout(timer);
      signal.removeEventListener('abort', done);
      resolve();
    }
  });
}

export async function probe(url: string, signal: AbortSignal, timeoutMs = 5_000): Promise<Probe> {
  try {
    const response = await fetch(url, {
      method: 'HEAD',
      cache: 'no-store',
      signal: anySignal([signal, AbortSignal.timeout(timeoutMs)]),
    });
    return classifyResponse(response.status, response.headers);
  } catch {
    return { kind: 'network' };
  }
}

/**
 * Requests `url` with GET only so that the API starts producing it (a transcode runs to its end on the server even
 * when nobody reads it), and classifies the answer like `probe()`. The body is discarded. The API may hold the
 * request while it waits for a free transcoder, hence the longer timeout.
 */
export async function startStream(url: string, signal: AbortSignal, timeoutMs = 20_000): Promise<Probe> {
  try {
    const response = await fetch(url, {
      cache: 'no-store',
      signal: anySignal([signal, AbortSignal.timeout(timeoutMs)]),
    });
    void response.body?.cancel().catch(() => undefined);
    return classifyResponse(response.status, response.headers);
  } catch {
    return { kind: 'network' };
  }
}

/**
 * How waiting for a live transcode ended: the stream in the player `reached` the wanted point by itself, the
 * transcode is `cached` (a finished, seekable file), or the wait `gave-up` (timeout, no audio file, aborted).
 */
export type StreamWait = 'reached' | 'cached' | 'gave-up';

/**
 * Polls `url` until the API serves it as a finished file (a transcode that has landed in the server's cache), or
 * until `reached()` reports that what the player already loaded is enough, or `timeoutMs` passes.
 */
export async function waitForStream(
  url: string,
  signal: AbortSignal,
  reached: () => boolean = () => false,
  timeoutMs = 15_000,
  intervalMs = 1_000,
): Promise<StreamWait> {
  const deadline = Date.now() + timeoutMs;
  while (!signal.aborted) {
    if (reached()) return 'reached';
    const result = await probe(url, signal);
    if (signal.aborted) break;
    if (result.kind === 'ok' && result.cached) return 'cached';
    if (reached()) return 'reached';
    if (result.kind === 'missing' || Date.now() + intervalMs > deadline) return 'gave-up';
    await sleep(intervalMs, signal);
  }
  return 'gave-up';
}
