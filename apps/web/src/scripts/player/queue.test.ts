import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { type Continuation, continuationFrom, listParams, type PageLoader, Queue, tracksFromPage } from './queue.ts';
import type { Track } from './track.ts';

const ERA = { eraName: 'DONDA 2 [V1]', color: '666666', hasCover: false, coverVersion: null };

function track(id: number): Track {
  return {
    id,
    title: `Song ${id}`,
    eraId: 31,
    eraName: 'DONDA 2 [V1]',
    eraPosition: id - 6324,
    length: 200,
    color: '666666',
    hasCover: false,
    coverVersion: null,
    pageHref: null,
  };
}

function song(id: number, playable: boolean) {
  return { id, eraId: 31, name: `Song ${id}`, playable, trackLength: 180 };
}

describe('listParams', () => {
  test('keeps only q, category and sort', () => {
    assert.equal(
      listParams('q=nebraska&page=5&offset=400&sort=name&limit=100&category=best-of'),
      'q=nebraska&category=best-of&sort=name',
    );
    assert.equal(listParams('q=++&sort='), '');
    assert.equal(listParams(undefined), '');
  });
});

describe('continuationFrom', () => {
  test('next page starts after the rows on this page', () => {
    assert.deepEqual(continuationFrom({ eraId: '31', offset: '400', total: '956', params: '' }, 100, ERA), {
      era: { ...ERA, eraId: 31 },
      offset: 500,
      total: 956,
      params: '',
    });
  });

  test('null when attributes are missing/invalid or the list is complete', () => {
    assert.equal(continuationFrom({ offset: '0', total: '10' }, 10, ERA), null);
    assert.equal(continuationFrom({ eraId: '31', total: '10' }, 10, ERA), null);
    assert.equal(continuationFrom({ eraId: '31', offset: '900', total: '956' }, 56, ERA), null);
    assert.equal(continuationFrom({ eraId: '31', offset: '0', total: '956' }, 0, ERA), null);
    assert.equal(continuationFrom({ eraId: '031', offset: '0' }, 10, ERA), null);
  });

  test('offset 0 and unknown totals are fine', () => {
    assert.deepEqual(continuationFrom({ eraId: '31', offset: '0', params: 'q=x' }, 100, ERA)?.offset, 100);
    assert.equal(continuationFrom({ eraId: '31', offset: '0', params: 'q=x' }, 100, ERA)?.total, null);
  });
});

describe('tracksFromPage', () => {
  const continuation: Continuation = { era: { ...ERA, eraId: 31 }, offset: 500, total: 956, params: '' };

  test('catalog order: playable songs with positions derived from the offset (older payloads)', () => {
    const tracks = tracksFromPage([song(1, false), song(2, true), song(3, true)], continuation);
    assert.deepEqual(
      tracks.map((t) => [t.id, t.eraPosition, t.pageHref]),
      [
        [2, 502, null],
        [3, 503, null],
      ],
    );
  });

  test('filtered lists link older payloads to the filtered list page', () => {
    const tracks = tracksFromPage([song(2, true)], { ...continuation, offset: 100, params: 'q=love' });
    assert.equal(tracks[0]?.eraPosition, null);
    assert.equal(tracks[0]?.pageHref, '/eras/31?q=love&page=2');
  });

  test('payload eraPosition wins', () => {
    const tracks = tracksFromPage([{ ...song(2, true), eraPosition: 7 }], { ...continuation, params: 'q=love' });
    assert.equal(tracks[0]?.eraPosition, 7);
    assert.equal(tracks[0]?.pageHref, null);
  });
});

describe('Queue', () => {
  test('navigation flags', () => {
    const queue = new Queue([track(1), track(2)], 0, null);
    assert.equal(queue.current?.id, 1);
    assert.equal(queue.hasPrevious(), false);
    assert.equal(queue.hasNext(), true);
    queue.index = 1;
    assert.equal(queue.hasNext(), false);
    assert.equal(queue.needsExtension(1), false);
  });

  test('extend fetches pages until a playable song is found, then stops at the end of the list', async () => {
    const calls: number[] = [];
    const pages: Record<number, unknown[]> = {
      500: Array.from({ length: 100 }, (_, i) => song(7000 + i, false)),
      600: [song(7100, false), song(6965, true), ...Array.from({ length: 98 }, (_, i) => song(7200 + i, false))],
      700: [song(7300, false)],
    };
    const load: PageLoader = async (continuation) => {
      calls.push(continuation.offset);
      return { songs: pages[continuation.offset] ?? [], total: 701 };
    };
    const queue = new Queue([track(6765)], 0, { era: { ...ERA, eraId: 31 }, offset: 500, total: 956, params: '' });
    assert.equal(queue.needsExtension(1), true);
    assert.equal(await queue.extend(load, new AbortController().signal), 'added');
    assert.deepEqual(calls, [500, 600]);
    assert.deepEqual(
      queue.tracks.map((t) => t.id),
      [6765, 6965],
    );
    assert.equal(queue.tracks[1]?.eraPosition, 602);
    assert.deepEqual(queue.continuation?.offset, 700);
    queue.index = 1;
    // The last page holds nothing playable and the total (from the response) is reached.
    assert.equal(await queue.extend(load, new AbortController().signal), 'none');
    assert.equal(queue.continuation, null);
    assert.equal(queue.hasNext(), false);
  });

  test('extend keeps the continuation when a page fails, and shares concurrent calls', async () => {
    let calls = 0;
    const failing: PageLoader = async () => {
      calls += 1;
      throw new Error('offline');
    };
    const queue = new Queue([track(1)], 0, { era: { ...ERA, eraId: 31 }, offset: 100, total: null, params: '' });
    const signal = new AbortController().signal;
    const [a, b] = await Promise.all([queue.extend(failing, signal), queue.extend(failing, signal)]);
    assert.deepEqual([a, b, calls], ['failed', 'failed', 1]);
    assert.notEqual(queue.continuation, null);
    assert.equal(queue.hasNext(), true, 'Next stays available to retry');
  });

  test('extend skips songs already in the queue', async () => {
    const queue = new Queue([track(1), track(2)], 1, { era: { ...ERA, eraId: 31 }, offset: 2, total: 4, params: '' });
    const load: PageLoader = async () => ({ songs: [song(2, true), song(3, true)], total: 4 });
    assert.equal(await queue.extend(load, new AbortController().signal), 'added');
    assert.deepEqual(
      queue.tracks.map((t) => t.id),
      [1, 2, 3],
    );
    assert.equal(queue.continuation, null);
  });

  test('toJSON/fromJSON round-trip, bounded around the current track', () => {
    const tracks = Array.from({ length: 1000 }, (_, i) => track(10_000 + i));
    const continuation: Continuation = { era: { ...ERA, eraId: 31 }, offset: 1000, total: 2000, params: 'q=x' };
    const saved = new Queue(tracks, 500, continuation).toJSON();
    assert.equal(saved.tracks.length, 300);
    assert.equal(saved.tracks[saved.index]?.id, 10_500);
    assert.equal(saved.continuation, null, 'a trimmed tail cannot continue from the stored offset');
    const restored = Queue.fromJSON(JSON.parse(JSON.stringify(saved)), track(10_500));
    assert.equal(restored?.current?.id, 10_500);
    assert.equal(restored?.tracks.length, 300);

    const small = new Queue([track(1), track(2)], 1, continuation).toJSON();
    assert.deepEqual(small.continuation, continuation);
    assert.deepEqual(Queue.fromJSON(JSON.parse(JSON.stringify(small)), track(2))?.continuation, continuation);
  });

  test('fromJSON rejects unusable data', () => {
    assert.equal(Queue.fromJSON(null, track(1)), null);
    assert.equal(Queue.fromJSON({ tracks: 'x' }, track(1)), null);
    assert.equal(Queue.fromJSON({ tracks: [track(2)], index: 0 }, track(1)), null);
    const fixed = Queue.fromJSON(
      { tracks: [track(2), track(1)], index: 0, continuation: { era: {}, offset: 1 } },
      track(1),
    );
    assert.equal(fixed?.index, 1);
    assert.equal(fixed?.continuation, null);
  });
});
