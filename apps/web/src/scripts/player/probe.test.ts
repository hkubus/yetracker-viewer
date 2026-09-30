import assert from 'node:assert/strict';
import { afterEach, describe, test } from 'node:test';
import { classifyResponse, parseRetryAfter, sleep, startStream, waitForStream } from './probe.ts';

const headers = (values: Record<string, string>) => new Headers(values);

describe('parseRetryAfter', () => {
  test('delta seconds and HTTP dates', () => {
    const now = Date.parse('2026-09-30T10:00:00Z');
    assert.equal(parseRetryAfter('5'), 5);
    assert.equal(parseRetryAfter(' 0 '), 0);
    assert.equal(parseRetryAfter('Wed, 30 Sep 2026 10:00:07 GMT', now), 7);
    assert.equal(parseRetryAfter('Wed, 30 Sep 2026 09:59:00 GMT', now), 0);
    assert.equal(parseRetryAfter(null), null);
    assert.equal(parseRetryAfter(''), null);
    assert.equal(parseRetryAfter('soon'), null);
  });
});

describe('classifyResponse', () => {
  test('maps statuses to what the player does next', () => {
    assert.deepEqual(classifyResponse(200, headers({ 'content-length': '12' })), { kind: 'ok', cached: true });
    assert.deepEqual(classifyResponse(206, headers({})), { kind: 'ok', cached: false });
    assert.deepEqual(classifyResponse(404, headers({})), { kind: 'missing' });
    assert.deepEqual(classifyResponse(410, headers({})), { kind: 'missing' });
    assert.deepEqual(classifyResponse(503, headers({ 'retry-after': '5' })), { kind: 'busy', retryAfter: 5 });
    assert.deepEqual(classifyResponse(503, headers({})), { kind: 'unavailable' });
    assert.deepEqual(classifyResponse(500, headers({})), { kind: 'server', status: 500 });
    assert.deepEqual(classifyResponse(400, headers({})), { kind: 'server', status: 400 });
  });
});

describe('sleep', () => {
  test('resolves early when aborted', async () => {
    const controller = new AbortController();
    const started = Date.now();
    const done = sleep(10_000, controller.signal);
    controller.abort();
    await done;
    assert.ok(Date.now() - started < 1_000);
  });
});

describe('waitForStream', () => {
  const realFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = realFetch;
  });

  test('polls HEAD until the response has a Content-Length', async () => {
    const methods: string[] = [];
    let calls = 0;
    globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
      methods.push(init?.method ?? 'GET');
      calls += 1;
      return new Response(null, { status: 200, headers: calls >= 3 ? { 'content-length': '100' } : {} });
    }) as typeof fetch;
    const signal = new AbortController().signal;
    assert.equal(await waitForStream('/songs/1/stream?quality=128', signal, undefined, 5_000, 10), 'cached');
    assert.equal(calls, 3);
    assert.deepEqual(new Set(methods), new Set(['HEAD']));
  });

  test('stops as soon as the player has loaded enough', async () => {
    let calls = 0;
    globalThis.fetch = (async () => {
      calls += 1;
      return new Response(null, { status: 200 });
    }) as typeof fetch;
    const signal = new AbortController().signal;
    assert.equal(await waitForStream('/x', signal, () => calls >= 2, 5_000, 10), 'reached');
    assert.equal(calls, 2);
    assert.equal(await waitForStream('/x', signal, () => true, 5_000, 10), 'reached');
    assert.equal(calls, 2, 'no request when the stream is already there');
  });

  test('gives up on 404 and after the timeout', async () => {
    globalThis.fetch = (async () => new Response(null, { status: 404 })) as typeof fetch;
    assert.equal(await waitForStream('/x', new AbortController().signal, undefined, 5_000, 10), 'gave-up');
    let calls = 0;
    globalThis.fetch = (async () => {
      calls += 1;
      return new Response(null, { status: 200 });
    }) as typeof fetch;
    assert.equal(await waitForStream('/x', new AbortController().signal, undefined, 50, 10), 'gave-up');
    assert.ok(calls >= 2 && calls <= 8, String(calls));
  });

  test('network errors count as not cached (and stop when aborted)', async () => {
    const controller = new AbortController();
    globalThis.fetch = (async () => {
      controller.abort();
      throw new TypeError('Failed to fetch');
    }) as typeof fetch;
    assert.equal(await waitForStream('/x', controller.signal, undefined, 5_000, 10), 'gave-up');
  });
});

describe('startStream', () => {
  const realFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = realFetch;
  });

  test('sends a GET, discards the body and classifies the answer', async () => {
    const seen: string[] = [];
    let cancelled = false;
    globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
      seen.push(init?.method ?? 'GET');
      const body = new ReadableStream({
        cancel() {
          cancelled = true;
        },
      });
      return new Response(body, { status: 200 });
    }) as typeof fetch;
    assert.deepEqual(await startStream('/songs/1/stream?quality=64', new AbortController().signal), {
      kind: 'ok',
      cached: false,
    });
    assert.deepEqual(seen, ['GET']);
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(cancelled, true);
    globalThis.fetch = (async () =>
      new Response(null, { status: 503, headers: { 'retry-after': '5' } })) as typeof fetch;
    assert.deepEqual(await startStream('/x', new AbortController().signal), { kind: 'busy', retryAfter: 5 });
    globalThis.fetch = (async () => {
      throw new TypeError('Failed to fetch');
    }) as typeof fetch;
    assert.deepEqual(await startStream('/x', new AbortController().signal), { kind: 'network' });
  });
});
