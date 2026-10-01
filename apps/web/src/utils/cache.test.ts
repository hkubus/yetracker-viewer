import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { cachedLoader, deepFreeze } from './cache.ts';

function clock(start = 0) {
  let time = start;
  return { now: () => time, advance: (ms: number) => (time += ms) };
}

describe('cachedLoader', () => {
  it('serves a fresh copy without loading again', async () => {
    const time = clock();
    let loads = 0;
    const load = cachedLoader(async () => ++loads, { freshMs: 1000, staleMs: 5000, now: time.now });
    assert.deepEqual(await load(), { data: 1, stale: false });
    time.advance(999);
    assert.deepEqual(await load(), { data: 1, stale: false });
    time.advance(1);
    assert.deepEqual(await load(), { data: 2, stale: false });
  });

  it('shares one refresh between concurrent callers', async () => {
    let loads = 0;
    let release: (value: number) => void = () => {};
    const load = cachedLoader(
      () => {
        loads++;
        return new Promise<number>((resolve) => {
          release = resolve;
        });
      },
      { freshMs: 1000, staleMs: 5000 },
    );
    const first = load();
    const second = load();
    release(7);
    assert.deepEqual(await Promise.all([first, second]), [
      { data: 7, stale: false },
      { data: 7, stale: false },
    ]);
    assert.equal(loads, 1);
  });

  it('falls back to a copy within the stale window, and never caches failures', async () => {
    const time = clock();
    let fail = false;
    let loads = 0;
    const load = cachedLoader(
      async () => {
        loads++;
        if (fail) throw new Error('down');
        return 'ok';
      },
      { freshMs: 1000, staleMs: 5000, now: time.now },
    );
    await assert.rejects(
      cachedLoader(
        async () => {
          throw new Error('down');
        },
        { freshMs: 1, staleMs: 1 },
      )(),
      /down/,
    );

    assert.deepEqual(await load(), { data: 'ok', stale: false });
    fail = true;
    time.advance(2000);
    assert.deepEqual(await load(), { data: 'ok', stale: true });
    // Every call past the fresh period tries again.
    assert.deepEqual(await load(), { data: 'ok', stale: true });
    assert.equal(loads, 3);
    time.advance(3000);
    await assert.rejects(load(), /down/);
    fail = false;
    assert.deepEqual(await load(), { data: 'ok', stale: false });
  });
});

describe('deepFreeze', () => {
  it('freezes nested objects and arrays', () => {
    const value = deepFreeze({ list: [{ id: 1 }], name: 'x' });
    assert.ok(Object.isFrozen(value));
    assert.ok(Object.isFrozen(value.list));
    assert.ok(Object.isFrozen(value.list[0]));
    assert.throws(() => {
      (value.list[0] as { id: number }).id = 2;
    }, TypeError);
    assert.equal(deepFreeze(null), null);
    assert.equal(deepFreeze(3), 3);
  });
});
