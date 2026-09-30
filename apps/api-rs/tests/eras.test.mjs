// Contract tests for the era routes, `/status`, `/health`, and the global route-table checks.
// See helpers.mjs for how to run the suite and what it expects from the server.

import assert from 'node:assert/strict';
import { before, describe, it } from 'node:test';

import {
  assertEra,
  assertError,
  assertNoStore,
  assertSong,
  categoryRank,
  discover,
  F,
  fold,
  JSON_CACHE,
  json,
  MISSING_ID,
  naturalKey,
  req,
  totalCount,
} from './helpers.mjs';

before(discover);

/** The era with the most songs (best coverage for sorting and filtering). */
const biggestEra = () => F.eras.reduce((best, era) => (era.songsCount > best.songsCount ? era : best), F.eras[0]);

async function eraSongs(eraId, query = '') {
  const res = await req(`/eras/${eraId}/songs${query}`);
  assert.equal(res.status, 200, `GET /eras/${eraId}/songs${query}`);
  return { res, total: totalCount(res), songs: await json(res) };
}

/** Every song of an era listing (all pages of 500), with extra query parameters `params`. */
async function allEraSongs(eraId, params = '') {
  const songs = [];
  let total = Number.POSITIVE_INFINITY;
  for (let offset = 0; offset < total; offset += 500) {
    const page = await eraSongs(eraId, `?limit=500&offset=${offset}${params}`);
    total = page.total;
    songs.push(...page.songs);
    if (page.songs.length === 0) break;
  }
  return { total, songs };
}

// ---------------------------------------------------------------------------
// Global / health / status
// ---------------------------------------------------------------------------

describe('global', () => {
  it('GET /health returns { status: "ok" } and is never cached', async () => {
    const res = await req('/health');
    assert.equal(res.status, 200);
    assertNoStore(res);
    assert.deepEqual(await json(res), { status: 'ok' });
  });

  it('GET /hello returns { hello: "world" }', async () => {
    const res = await req('/hello');
    assert.equal(res.status, 200);
    assert.deepEqual(await json(res), { hello: 'world' });
  });

  it('unknown route returns 404 JSON { error: "Not found" }', async () => {
    for (const init of [undefined, { method: 'POST' }]) {
      const res = await req('/does-not-exist-xyz', init);
      assert.equal(res.status, 404);
      assertNoStore(res);
      assert.deepEqual(await json(res), { error: 'Not found' });
    }
    // An empty id does not match /eras/{id}.
    const empty = await req('/eras/');
    assert.equal(empty.status, 404);
  });

  it('wrong methods on known routes return 405 with Allow', async () => {
    for (const [method, path] of [
      ['POST', '/health'],
      ['PUT', '/eras'],
      ['DELETE', `/eras/${F.eraId}`],
      ['PATCH', '/songs'],
      ['POST', `/songs/${F.songId}`],
    ]) {
      const res = await req(path, { method });
      assert.equal(res.status, 405, `${method} ${path}`);
      assert.equal(res.headers.get('allow'), 'GET, HEAD, OPTIONS');
      assertNoStore(res);
      assert.deepEqual(await json(res), { error: 'Method not allowed' });
    }
  });

  it('HEAD answers like GET without a body', async () => {
    const res = await req('/eras', { method: 'HEAD' });
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    assert.equal((await res.arrayBuffer()).byteLength, 0);
  });

  it('exposes X-Total-Count via CORS on paginated endpoints', async () => {
    const res = await req(`/eras/${F.eraId}/songs?limit=1`);
    assert.equal(res.status, 200);
    assert.match(res.headers.get('access-control-expose-headers') ?? '', /X-Total-Count/i);
  });

  it('JSON responses carry one weak ETag for every encoding and answer 304 to If-None-Match', async () => {
    const paths = ['/eras', `/eras/${F.eraId}`, `/eras/${F.eraId}/songs?limit=5`, '/status', '/songs?limit=50'];
    for (const path of paths) {
      const identity = await req(path, { headers: { 'Accept-Encoding': 'identity' } });
      assert.equal(identity.status, 200);
      assert.equal(identity.headers.get('content-encoding'), null);
      await identity.arrayBuffer();
      const etag = identity.headers.get('etag');
      // Weak: the gzip and the identity body are the same data, not the same bytes.
      assert.match(etag ?? '', /^W\/"[0-9a-f]{32}"$/, `weak ETag on ${path}`);
      const gzip = await req(path, { headers: { 'Accept-Encoding': 'gzip' } });
      await gzip.arrayBuffer();
      assert.equal(gzip.headers.get('etag'), etag, `${path}: one validator for both encodings`);
      for (const [encoding, tag] of [
        ['identity', etag],
        ['gzip', etag],
        ['gzip', etag.slice(2)],
      ]) {
        const again = await req(path, { headers: { 'Accept-Encoding': encoding, 'If-None-Match': tag } });
        assert.equal(again.status, 304, `304 for ${path} (${encoding}, If-None-Match: ${tag})`);
        assert.equal(again.headers.get('etag'), etag);
        await again.arrayBuffer();
      }
    }
    const big = await req('/songs?limit=50', { headers: { 'Accept-Encoding': 'gzip' } });
    assert.equal(big.headers.get('content-encoding'), 'gzip', 'a large JSON body is compressed');
    await big.arrayBuffer();
  });
});

describe('GET /status', () => {
  it('reports the last import and consistent counts', async () => {
    const res = await req('/status');
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const status = await json(res);
    assert.deepEqual(Object.keys(status).sort(), [
      'eras',
      'lastImportAt',
      'lastImportError',
      'lastImportOk',
      'playableSongs',
      'songs',
      'status',
    ]);
    assert.equal(status.status, 'ok');
    assert.ok(status.lastImportAt === null || Number.isSafeInteger(status.lastImportAt));
    assert.ok(status.lastImportOk === null || typeof status.lastImportOk === 'boolean');
    assert.ok(status.lastImportError === null || typeof status.lastImportError === 'string');
    if (status.lastImportOk === true) assert.equal(status.lastImportError, null);
    assert.equal(status.eras, F.eras.length);
    const listed = F.eras.reduce((sum, era) => sum + era.songsCount, 0);
    assert.ok(status.songs >= listed, 'every song of a listed era is counted');
    assert.equal(totalCount(await req('/songs?limit=1')), status.songs);
    assert.ok(Number.isSafeInteger(status.playableSongs) && status.playableSongs >= 0);
    assert.ok(status.playableSongs <= status.songs);
  });
});

// ---------------------------------------------------------------------------
// GET /eras
// ---------------------------------------------------------------------------

describe('GET /eras', () => {
  it('returns every main era in catalog order with the v2 shape', async () => {
    const res = await req('/eras');
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const eras = await json(res);
    assert.ok(Array.isArray(eras) && eras.length > 0);
    for (const era of eras) assertEra(era);
    for (let index = 1; index < eras.length; index += 1) {
      assert.ok(eras[index].position > eras[index - 1].position, `eras ordered by position at ${index}`);
    }
    assert.equal(new Set(eras.map((era) => era.id)).size, eras.length);
  });

  it('songsCount matches each era listing', async () => {
    for (const era of F.eras.slice(0, 5)) {
      const { total } = await eraSongs(era.id, '?limit=1');
      assert.equal(total, era.songsCount, `songsCount of era ${era.id}`);
    }
  });
});

// ---------------------------------------------------------------------------
// GET /eras/:id
// ---------------------------------------------------------------------------

describe('GET /eras/:id', () => {
  it('returns the same object as the list, songsCount included', async () => {
    for (const listed of [F.eras[0], F.eras.at(-1)]) {
      const res = await req(`/eras/${listed.id}`);
      assert.equal(res.status, 200);
      assert.equal(res.headers.get('cache-control'), JSON_CACHE);
      const era = await json(res);
      assertEra(era);
      assert.deepEqual(era, listed);
    }
  });

  it('400 on invalid ids, including leading zeros and undecodable segments', async () => {
    for (const bad of ['abc', '0', '-1', '1.5', '01', '007', '+1', '%20', '%FF', '9007199254740992']) {
      await assertError(await req(`/eras/${bad}`), 400, /^Invalid era id$/);
    }
    await assertError(await req('/eras/%FF/songs'), 400, /^Invalid era id$/);
  });

  it('404 on a missing era', async () => {
    await assertError(await req(`/eras/${MISSING_ID}`), 404, /^Era does not exist$/);
  });
});

// ---------------------------------------------------------------------------
// GET /eras/:id/songs
// ---------------------------------------------------------------------------

describe('GET /eras/:id/songs', () => {
  it('returns v2 songs in catalog order with the real X-Total-Count', async () => {
    const era = biggestEra();
    const { res, total, songs } = await eraSongs(era.id, '?limit=100');
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    assert.equal(total, era.songsCount);
    assert.ok(songs.length === Math.min(100, total));
    songs.forEach((song, index) => {
      assertSong(song);
      assert.equal(song.eraId, era.id);
      assert.equal(song.eraPosition, index + 1, 'eraPosition is the 1-based catalog position in the era');
    });
    // sort=catalog and its legacy alias sort=id are the default order.
    for (const sort of ['catalog', 'id']) {
      const sorted = await eraSongs(era.id, `?limit=100&sort=${sort}`);
      assert.deepEqual(
        sorted.songs.map((song) => song.id),
        songs.map((song) => song.id),
        `sort=${sort}`,
      );
    }
  });

  it('offset paginates and totals stay real past the end', async () => {
    const era = biggestEra();
    const first = await eraSongs(era.id, '?limit=3&offset=0');
    const second = await eraSongs(era.id, '?limit=3&offset=3');
    assert.equal(second.songs[0]?.eraPosition, 4);
    assert.ok(!second.songs.some((song) => first.songs.some((other) => other.id === song.id)));
    for (const offset of [era.songsCount, era.songsCount + 5, 5000, 10_000]) {
      const past = await eraSongs(era.id, `?limit=5&offset=${offset}`);
      assert.deepEqual(past.songs, []);
      assert.equal(past.total, era.songsCount, `X-Total-Count at offset ${offset}`);
    }
    const clamped = await eraSongs(era.id, '?limit=9999');
    assert.equal(clamped.songs.length, Math.min(500, era.songsCount), 'limit is clamped to 500');
  });

  it('q matches folded tokens (case, accents and punctuation insensitive)', async () => {
    const era = biggestEra();
    const { songs } = await eraSongs(era.id, '?limit=100');
    const sample = songs.find((song) => /\[V\d+\]/.test(song.title) && fold(song.title).split(' ').length >= 2);
    assert.ok(sample, 'need a versioned title to derive a query');
    const probe = fold(sample.title);
    const exact = await eraSongs(era.id, `?limit=100&q=${encodeURIComponent(probe)}`);
    assert.ok(
      exact.songs.some((song) => song.id === sample.id),
      `q=${probe} finds song ${sample.id}`,
    );
    for (const song of exact.songs) assertSong(song);
    // The same query typed differently finds the same songs.
    const shouted = await eraSongs(era.id, `?limit=100&q=${encodeURIComponent(`  ${sample.title.toUpperCase()}  `)}`);
    assert.equal(shouted.total, exact.total);
    // Blank q is no filter.
    const blank = await eraSongs(era.id, '?limit=1&q=%20%20');
    assert.equal(blank.total, era.songsCount);
    // Tokens combine with AND: a nonsense token empties the result.
    const none = await eraSongs(era.id, `?q=${encodeURIComponent(`${probe} zzqxjw`)}`);
    assert.equal(none.total, 0);
    assert.deepEqual(none.songs, []);
  });

  it('q matches what the songs themselves say, not the era name', async () => {
    // An era whose name has a word that some (but maybe not all) of its songs mention.
    const era = [...F.eras]
      .sort((a, b) => b.songsCount - a.songsCount)
      .find((candidate) =>
        fold(candidate.name)
          .split(' ')
          .some((word) => word.length >= 3 && !/^v\d+$/.test(word)),
      );
    assert.ok(era, 'need an era with a word in its name');
    const token = fold(era.name)
      .split(' ')
      .find((word) => word.length >= 3 && !/^v\d+$/.test(word));
    const all = await allEraSongs(era.id);
    const own = (song) =>
      fold([song.name, song.notes, song.subEra, song.quality, song.availableLength].filter(Boolean).join(' '));
    const expected = all.songs.filter((song) => own(song).includes(token)).map((song) => song.id);
    const found = await allEraSongs(era.id, `&q=${encodeURIComponent(token)}`);
    assert.equal(
      found.total,
      expected.length,
      `q=${token} on era ${era.id} (${era.name}) counts its own mentions only`,
    );
    assert.deepEqual(
      found.songs.map((song) => song.id),
      expected,
    );
  });

  it('a q without anything searchable is blank; category markers in q filter', async () => {
    const era = biggestEra();
    for (const blank of ['???', '%20-%20', '%E2%80%8B']) {
      const { total } = await eraSongs(era.id, `?limit=1&q=${blank}`);
      assert.equal(total, era.songsCount, `q=${blank} is no filter`);
    }
    for (const [category, marker] of [
      ['best-of', '⭐'],
      ['worst-of', '🗑️'],
    ]) {
      const byCategory = await allEraSongs(era.id, `&category=${category}`);
      const byMarker = await allEraSongs(era.id, `&q=${encodeURIComponent(marker)}`);
      assert.equal(byMarker.total, byCategory.total, `q=${marker}`);
      assert.deepEqual(
        byMarker.songs.map((song) => song.id),
        byCategory.songs.map((song) => song.id),
      );
    }
  });

  it('blank parameters count as absent, padded ones are trimmed', async () => {
    const era = biggestEra();
    const reference = await eraSongs(era.id, '?limit=20');
    for (const blank of ['q', 'category', 'sort', 'limit', 'offset'].map((name) => `${name}=%20`)) {
      const { total, songs } = await eraSongs(era.id, `?${blank}&limit=20`);
      assert.equal(total, era.songsCount, blank);
      assert.deepEqual(
        songs.map((song) => song.id),
        reference.songs.map((song) => song.id),
        blank,
      );
    }
    const padded = await eraSongs(era.id, '?limit=20&sort=+name+&category=%20best-of%20');
    const exact = await eraSongs(era.id, '?limit=20&sort=name&category=best-of');
    assert.equal(padded.total, exact.total);
    assert.deepEqual(
      padded.songs.map((song) => song.id),
      exact.songs.map((song) => song.id),
    );
  });

  it('ignores the /songs-only filters, whatever their value or encoding', async () => {
    const reference = await eraSongs(F.eraId, '?limit=20');
    const extras = ['era=%FF', 'eraFrom=%C3', 'eraTo=abc', 'quality=%FE', 'availability=Bogus', 'playable=maybe'];
    for (const extra of extras) {
      const { total, songs } = await eraSongs(F.eraId, `?${extra}&limit=20`);
      assert.equal(total, reference.total, extra);
      assert.deepEqual(
        songs.map((song) => song.id),
        reference.songs.map((song) => song.id),
        extra,
      );
    }
  });

  it('q is limited to 100 characters', async () => {
    const hundred = await req(`/eras/${F.eraId}/songs?q=${encodeURIComponent('é'.repeat(100))}`);
    assert.equal(hundred.status, 200);
    await assertError(
      await req(`/eras/${F.eraId}/songs?q=${encodeURIComponent('é'.repeat(101))}`),
      400,
      /^Search query is too long$/,
    );
  });

  it('date sorts put songs without a date last', async () => {
    const era = biggestEra();
    for (const [sort, field, direction] of [
      ['leak-newest', 'leakDate', -1],
      ['leak-oldest', 'leakDate', 1],
      ['file-newest', 'fileDate', -1],
    ]) {
      const { songs } = await eraSongs(era.id, `?limit=500&sort=${sort}`);
      const firstEmpty = songs.findIndex((song) => song[field] === null);
      if (firstEmpty !== -1) {
        assert.ok(
          songs.slice(firstEmpty).every((song) => song[field] === null),
          `${sort}: songs without ${field} come last`,
        );
      }
      const dated = songs.filter((song) => song[field] !== null);
      for (let index = 1; index < dated.length; index += 1) {
        const step = Math.sign(dated[index][field] - dated[index - 1][field]);
        assert.ok(step === 0 || step === direction, `${sort} broke at ${index}`);
        if (step === 0) {
          assert.ok(dated[index].eraPosition > dated[index - 1].eraPosition, `${sort} ties keep catalog order`);
        }
      }
    }
  });

  it('sort=name is natural (numbers compare numerically, untitled last)', async () => {
    const era = biggestEra();
    const { songs } = await eraSongs(era.id, '?limit=500&sort=name');
    const keys = songs.map((song) => naturalKey(song.title));
    const firstEmpty = keys.indexOf('');
    if (firstEmpty !== -1)
      assert.ok(
        keys.slice(firstEmpty).every((key) => key === ''),
        'untitled songs last',
      );
    for (let index = 1; index < keys.length; index += 1) {
      if (keys[index] === '' || keys[index - 1] === '') continue;
      assert.ok(keys[index - 1] <= keys[index], `name order broke at ${index}: ${keys[index - 1]} > ${keys[index]}`);
      if (keys[index - 1] === keys[index]) {
        assert.ok(songs[index].eraPosition > songs[index - 1].eraPosition, 'equal names keep catalog order');
      }
    }
    await assertError(await req(`/eras/${era.id}/songs?sort=bogus`), 400, /^Invalid sort$/);
    await assertError(await req(`/eras/${era.id}/songs?sort=ID`), 400, /^Invalid sort$/);
  });

  it('sort=category puts unmarked songs between wanted and worst-of', async () => {
    // The plain list's category order starts with a best-of song; use its era.
    const sample = await json(await req('/songs?sort=category&limit=1'));
    assert.ok(Array.isArray(sample) && sample.length === 1);
    const eraId = sample[0].eraId;
    const { songs } = await allEraSongs(eraId, '&sort=category');
    const ranks = songs.map((song) => categoryRank(song.title));
    for (let index = 1; index < ranks.length; index += 1) {
      assert.ok(
        ranks[index] >= ranks[index - 1],
        `category order broke at ${index}: ${ranks[index - 1]} → ${ranks[index]}`,
      );
      if (ranks[index] === ranks[index - 1]) {
        assert.ok(songs[index].eraPosition > songs[index - 1].eraPosition, 'same category keeps catalog order');
      }
    }
  });

  it('category filters by marker, counts exactly and validates the value', async () => {
    const sample = await json(await req('/songs?sort=category&limit=1'));
    const eraId = sample[0].eraId;
    const all = await allEraSongs(eraId);
    assert.equal(all.songs.length, all.total);
    for (const [category, marker] of [
      ['best-of', '⭐'],
      ['special', '✨'],
      ['grails', '🏆'],
      ['wanted', '🏅'],
      ['worst-of', '🗑'],
      ['ai', '🤖'],
    ]) {
      const expected = all.songs.filter((song) => song.title.includes(marker)).map((song) => song.id);
      const filtered = await allEraSongs(eraId, `&category=${category}`);
      assert.equal(filtered.total, expected.length, `X-Total-Count of category=${category}`);
      assert.deepEqual(
        filtered.songs.map((song) => song.id),
        expected,
        `category=${category} keeps catalog order`,
      );
    }
    await assertError(await req(`/eras/${eraId}/songs?category=bogus`), 400, /^Invalid category filter$/);
    const empty = await eraSongs(eraId, '?limit=1&category=');
    assert.equal(empty.total, all.total, 'an empty category is no filter');
  });

  it('400 on invalid limit/offset, 404 on a missing era', async () => {
    const cases = [
      ['limit=abc', /^Invalid limit$/],
      ['limit=0', /^Invalid limit$/],
      ['limit=-1', /^Invalid limit$/],
      ['offset=abc', /^Invalid offset$/],
      ['offset=-1', /^Invalid offset$/],
      ['offset=10001', /^Invalid offset$/],
      ['q=%FF', /^Invalid search query$/],
    ];
    for (const [query, pattern] of cases) {
      await assertError(await req(`/eras/${F.eraId}/songs?${query}`), 400, pattern);
    }
    await assertError(await req('/eras/abc/songs'), 400, /^Invalid era id$/);
    await assertError(await req(`/eras/${MISSING_ID}/songs`), 404, /^Era does not exist$/);
  });
});
