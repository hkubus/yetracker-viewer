// Black-box HTTP contract tests for the YeTracker Viewer API.
//
// Run against a running API (original or reimplementation):
//   SYNC_ON_START=false API_PORT=3100 node src/index.ts &
//   API_BASE_URL=http://127.0.0.1:3100 npm test
//   # or: API_BASE_URL=http://127.0.0.1:3100 node --test tests/
//
// Env:
//   API_BASE_URL (fallback: BASE_URL, fallback: http://127.0.0.1:3000)
//
// Notes for reimplementation verification:
// - Tests are adaptive: IDs are discovered from list endpoints instead of
//   hardcoded, so they work with any dataset.
// - Hono error responses from route handlers are `text/plain` with the
//   thrown message (e.g. "Invalid era id"). Only the global unknown-route
//   handler returns JSON `{"error":"Not found"}`.
// - Media 404s (`/stream`, `/download`, `/duration` when the file is absent
//   from disk) intentionally reset the file's DB row so the downloader
//   retries it. Run against a COPY of the database, not production.
//   Example: copy storage/db.sqlite3 + covers to /tmp/yt-test and start the
//   server with STORAGE_DIR=/tmp/yt-test (and SONGS_DIR if set).
// - When no playable files exist on disk, media *success* tests skip
//   gracefully; validation/404 tests still run.

import assert from 'node:assert/strict';
import { before, describe, it } from 'node:test';

const BASE = process.env.API_BASE_URL ?? process.env.BASE_URL ?? 'http://127.0.0.1:3000';

const JSON_CACHE = 'public, max-age=60, s-maxage=300, stale-while-revalidate=600';
const MISSING_ID = 999_999_999;

async function req(path, init) {
  const res = await fetch(`${BASE}${path}`, init);
  return res;
}

async function json(res) {
  assert.match(res.headers.get('content-type') ?? '', /application\/json/, `expected JSON content-type for ${res.url}`);
  return res.json();
}

async function text(res) {
  return res.text();
}

// Discovered fixtures (filled in before()).
const F = {
  eras: [],
  eraId: null,
  categories: [],
  categoryId: null,
  songId: null,
  nonPlayableSongId: null,
  playableSongId: null,
  coverEraId: null,
  coverEtag: null,
};

before(async () => {
  let health;
  try {
    health = await req('/health');
  } catch (error) {
    throw new Error(
      `API not reachable at ${BASE}: ${error.message}. Start it first, e.g. SYNC_ON_START=false API_PORT=3000 node src/index.ts`,
    );
  }
  assert.equal(health.status, 200, `GET /health status (body: ${await health.text()})`);

  // Discover eras.
  const erasRes = await req('/eras');
  assert.equal(erasRes.status, 200);
  F.eras = await json(erasRes);
  assert.ok(Array.isArray(F.eras) && F.eras.length > 0, 'expected at least one era');
  F.eraId = F.eras[0].id;

  // Discover categories.
  const catsRes = await req('/categories');
  assert.equal(catsRes.status, 200);
  F.categories = await json(catsRes);
  assert.ok(Array.isArray(F.categories) && F.categories.length > 0, 'expected at least one category');
  F.categoryId = F.categories[0].id;

  // Discover a song id (plain list mode).
  const songsRes = await req('/songs?limit=5');
  assert.equal(songsRes.status, 200);
  const songs = await json(songsRes);
  assert.ok(Array.isArray(songs) && songs.length > 0, 'expected at least one song');
  F.songId = songs[0].id;

  // Find a playable song if any exist (search mode with playable filter).
  try {
    const playableRes = await req('/songs?playable=true&limit=50');
    if (playableRes.status === 200) {
      const body = await json(playableRes);
      if (body.songs?.length > 0) F.playableSongId = body.songs[0].id;
    }
  } catch {
    // No playable songs — media success tests will skip.
  }
  // Find a non-playable song id for deterministic 404-file tests.
  try {
    const nonPlayableRes = await req('/songs?playable=false&limit=5');
    if (nonPlayableRes.status === 200) {
      const body = await json(nonPlayableRes);
      if (body.songs?.length > 0) F.nonPlayableSongId = body.songs[0].id;
    }
  } catch {
    // ignore
  }
  F.nonPlayableSongId ??= F.songId;

  // Find an era with a cover file (covers may not exist for every era).
  for (const era of F.eras.slice(0, 10)) {
    const coverRes = await req(`/eras/${era.id}/cover`);
    // Drain body to free the connection.
    await coverRes.arrayBuffer().catch(() => {});
    if (coverRes.status === 200) {
      F.coverEraId = era.id;
      F.coverEtag = coverRes.headers.get('etag');
      break;
    }
  }
});

// ---------------------------------------------------------------------------
// Global / health / hello
// ---------------------------------------------------------------------------

describe('global', () => {
  it('GET /health returns { status: "ok" }', async () => {
    const res = await req('/health');
    assert.equal(res.status, 200);
    assert.deepEqual(await json(res), { status: 'ok' });
  });

  it('GET /hello returns { hello: "world" }', async () => {
    const res = await req('/hello');
    assert.equal(res.status, 200);
    assert.deepEqual(await json(res), { hello: 'world' });
  });

  it('unknown route returns 404 JSON { error: "Not found" }', async () => {
    const res = await req('/does-not-exist-xyz');
    assert.equal(res.status, 404);
    assert.deepEqual(await json(res), { error: 'Not found' });
  });

  it('unsupported method returns 404 (only GET is registered)', async () => {
    const res = await req('/health', { method: 'POST' });
    assert.equal(res.status, 404);
  });

  it('exposes X-Total-Count via CORS on paginated endpoints', async () => {
    const res = await req(`/eras/${F.eraId}/songs?limit=1`);
    assert.equal(res.status, 200);
    const exposed = res.headers.get('access-control-expose-headers') ?? '';
    assert.match(exposed, /X-Total-Count/);
  });
});

// ---------------------------------------------------------------------------
// GET /eras
// ---------------------------------------------------------------------------

describe('GET /eras', () => {
  it('returns an array with expected shape and cache header', async () => {
    const res = await req('/eras');
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const eras = await json(res);
    assert.ok(Array.isArray(eras));
    for (const era of eras) {
      assert.equal(typeof era.id, 'number');
      assert.ok('name' in era && 'notes' in era && 'description' in era);
      assert.ok('dominantColor' in era);
      assert.equal(typeof era.coverVersion, 'string');
      assert.equal(typeof era.songsCount, 'number');
      // Raw DB columns must not leak.
      assert.ok(!('imageUrl' in era), 'imageUrl must be replaced by coverVersion');
      assert.ok(!('image_url' in era));
      assert.ok(!('isMain' in era) && !('is_main' in era));
      assert.ok(!('coverSource' in era));
    }
  });
});

// ---------------------------------------------------------------------------
// GET /eras/:id
// ---------------------------------------------------------------------------

describe('GET /eras/:id', () => {
  it('returns a single era for a valid id', async () => {
    const res = await req(`/eras/${F.eraId}`);
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const era = await json(res);
    assert.equal(era.id, F.eraId);
    assert.equal(typeof era.coverVersion, 'string');
    assert.ok(!('songsCount' in era), 'detail view has no songsCount');
    assert.ok(!('imageUrl' in era) && !('isMain' in era));
  });

  it('400 on invalid ids', async () => {
    for (const bad of ['abc', '0', '-1', '1.5']) {
      const res = await req(`/eras/${bad}`);
      assert.equal(res.status, 400, `expected 400 for /eras/${bad}`);
      assert.match(await text(res), /Invalid era id/);
    }
    // Empty id (GET /eras/) does not match /eras/:id, so the global
    // unknown-route handler returns 404 JSON instead of 400 text.
    const empty = await req('/eras/');
    assert.equal(empty.status, 404);
  });

  it('404 on missing era', async () => {
    const res = await req(`/eras/${MISSING_ID}`);
    assert.equal(res.status, 404);
    assert.match(await text(res), /Era does not exist/);
  });
});

// ---------------------------------------------------------------------------
// GET /eras/:id/songs
// ---------------------------------------------------------------------------

describe('GET /eras/:id/songs', () => {
  it('returns paginated songs with X-Total-Count and playback fields', async () => {
    const res = await req(`/eras/${F.eraId}/songs?limit=2`);
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const total = Number(res.headers.get('x-total-count'));
    assert.ok(Number.isSafeInteger(total) && total >= 0);
    const songs = await json(res);
    assert.ok(Array.isArray(songs) && songs.length <= 2);
    assert.ok(songs.length <= total || total === 0);
    const ids = [];
    for (const s of songs) {
      assert.equal(s.eraId, F.eraId);
      assert.equal(s.catalogId, 'unreleased');
      assert.equal(typeof s.id, 'number');
      assert.equal(typeof s.playable, 'boolean');
      assert.ok('duration' in s);
      if (!s.playable) assert.equal(s.duration, null, 'non-playable duration must be null');
      assert.ok(!('filename' in s) && !('fileDuration' in s), 'internal file fields stripped');
      ids.push(s.id);
    }
    assert.deepEqual(
      [...ids].sort((a, b) => a - b),
      ids,
      'ordered by id ASC',
    );
  });

  it('offset paginates', async () => {
    const first = await json(await req(`/eras/${F.eraId}/songs?limit=1&offset=0`));
    const second = await json(await req(`/eras/${F.eraId}/songs?limit=1&offset=1`));
    if (first.length > 0 && second.length > 0) {
      assert.notEqual(first[0].id, second[0].id);
    }
  });

  it('q filters (case-insensitive substring)', async () => {
    const all = await json(await req(`/eras/${F.eraId}/songs?limit=1`));
    assert.ok(all.length > 0, 'need at least one song to derive a query');
    const probe = (all[0].name ?? '').split(/\s+/).find((w) => w.length >= 3) ?? 'a';
    const res = await req(`/eras/${F.eraId}/songs?limit=5&q=${encodeURIComponent(probe)}`);
    assert.equal(res.status, 200);
    const songs = await json(res);
    assert.ok(songs.length > 0, `expected q=${probe} to match`);
  });

  it('400 on invalid limit/offset and overlong q', async () => {
    let res = await req(`/eras/${F.eraId}/songs?limit=abc`);
    assert.equal(res.status, 400);
    assert.match(await text(res), /Invalid limit/);

    res = await req(`/eras/${F.eraId}/songs?limit=0`);
    assert.equal(res.status, 400);

    res = await req(`/eras/${F.eraId}/songs?offset=abc`);
    assert.equal(res.status, 400);
    assert.match(await text(res), /Invalid offset/);

    res = await req(`/eras/${F.eraId}/songs?q=${'x'.repeat(101)}`);
    assert.equal(res.status, 400);
    assert.match(await text(res), /Search query is too long/);
  });

  it('400 on invalid era id, 404 on missing era', async () => {
    let res = await req('/eras/abc/songs');
    assert.equal(res.status, 400);
    res = await req(`/eras/${MISSING_ID}/songs`);
    assert.equal(res.status, 404);
    assert.match(await text(res), /Era does not exist/);
  });
});

// ---------------------------------------------------------------------------
// GET /eras/:id/cover
// ---------------------------------------------------------------------------

describe('GET /eras/:id/cover', () => {
  it('400 on invalid id, 404 on missing cover', async () => {
    let res = await req('/eras/abc/cover');
    assert.equal(res.status, 400);
    assert.match(await text(res), /Invalid era id/);

    res = await req(`/eras/${MISSING_ID}/cover`);
    // Missing era and missing file both surface as 404 Cover not found
    // (cover handler stats the file directly without checking the era row).
    assert.equal(res.status, 404);
    assert.match(await text(res), /Cover not found/);
  });

  it('serves image/avif with ETag and immutable caching (skips if no covers)', async (t) => {
    if (F.coverEraId == null) {
      t.skip('no era cover file found in first 10 eras');
      return;
    }
    const res = await req(`/eras/${F.coverEraId}/cover`);
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('content-type'), 'image/avif');
    assert.equal(res.headers.get('cache-control'), 'public, max-age=86400, immutable');
    assert.ok(res.headers.get('etag'), 'ETag required');
    assert.ok(res.headers.get('last-modified'), 'Last-Modified required');
    assert.equal(res.headers.get('accept-ranges'), 'bytes');
    assert.ok(Number(res.headers.get('content-length')) > 0);
    const bytes = await res.arrayBuffer();
    assert.ok(bytes.byteLength > 0);
  });

  it('returns 304 on matching If-None-Match (skips if no covers)', async (t) => {
    if (F.coverEraId == null || !F.coverEtag) {
      t.skip('no cover ETag discovered');
      return;
    }
    const res = await req(`/eras/${F.coverEraId}/cover`, {
      headers: { 'If-None-Match': F.coverEtag },
    });
    assert.equal(res.status, 304);
    await res.arrayBuffer().catch(() => {});
  });
});

// ---------------------------------------------------------------------------
// GET /songs (plain list mode)
// ---------------------------------------------------------------------------

describe('GET /songs plain list', () => {
  it('returns a bare array with default pagination', async () => {
    const res = await req('/songs?limit=2');
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const songs = await json(res);
    assert.ok(Array.isArray(songs) && songs.length <= 2);
    for (const s of songs) {
      assert.equal(typeof s.id, 'number');
      assert.equal(s.catalogId, 'unreleased');
      assert.ok('name' in s && 'quality' in s && 'url' in s);
      assert.ok(!('playable' in s), 'plain mode has no playable enrichment');
    }
  });

  it('offset works and invalid pagination is 400', async () => {
    const a = await json(await req('/songs?limit=1&offset=0'));
    const b = await json(await req('/songs?limit=1&offset=1'));
    if (a.length > 0 && b.length > 0) assert.notEqual(a[0].id, b[0].id);

    let res = await req('/songs?limit=abc');
    assert.equal(res.status, 400);
    res = await req('/songs?limit=0');
    assert.equal(res.status, 400);
    res = await req('/songs?offset=abc');
    assert.equal(res.status, 400);
  });
});

// ---------------------------------------------------------------------------
// GET /songs (search / filter mode)
// ---------------------------------------------------------------------------

describe('GET /songs search', () => {
  it('q returns envelope { songs, total } with ranking fields', async () => {
    const res = await req('/songs?q=love&limit=5');
    assert.equal(res.status, 200);
    const body = await json(res);
    assert.ok(Array.isArray(body.songs) && body.songs.length <= 5);
    assert.equal(typeof body.total, 'number');
    assert.ok(body.total >= body.songs.length);
    for (const s of body.songs) {
      assert.equal(typeof s.id, 'number');
      assert.equal(typeof s.playable, 'boolean');
      assert.equal(typeof s.eraPosition, 'number');
      assert.ok('eraName' in s && 'dominantColor' in s);
    }
  });

  it('empty result keeps envelope shape', async () => {
    const res = await req('/songs?q=zzzz-no-such-song-xyz-123');
    assert.equal(res.status, 200);
    const body = await json(res);
    assert.deepEqual(body.songs, []);
    assert.equal(body.total, 0);
  });

  it('era / eraFrom / eraTo filters work', async () => {
    let res = await req(`/songs?era=${F.eraId}&limit=5`);
    assert.equal(res.status, 200);
    let body = await json(res);
    assert.ok(body.total > 0);
    for (const s of body.songs) assert.equal(s.eraId, F.eraId);

    res = await req(`/songs?eraFrom=${F.eraId}&eraTo=${F.eraId}&limit=5`);
    assert.equal(res.status, 200);
    body = await json(res);
    for (const s of body.songs) assert.equal(s.eraId, F.eraId);
  });

  it('quality / availability / playable filters work', async () => {
    let res = await req('/songs?quality=CD%20Quality&limit=2');
    assert.equal(res.status, 200);
    let body = await json(res);
    for (const s of body.songs) assert.equal(s.quality, 'CD Quality');

    res = await req('/songs?availability=Snippet&limit=2');
    assert.equal(res.status, 200);
    body = await json(res);
    for (const s of body.songs) assert.equal(s.availableLength, 'Snippet');

    res = await req('/songs?playable=false&limit=2');
    assert.equal(res.status, 200);
    body = await json(res);
    for (const s of body.songs) assert.equal(s.playable, false);
  });

  it('search limit is capped at 50', async () => {
    const res = await req('/songs?q=a&limit=100');
    assert.equal(res.status, 200);
    const body = await json(res);
    assert.ok(body.songs.length <= 50, `got ${body.songs.length}, expected <= 50`);
  });

  it('400 on invalid filters', async () => {
    const cases = [
      ['/songs?quality=Bogus', /Invalid quality filter/],
      ['/songs?availability=Bogus', /Invalid availability filter/],
      ['/songs?playable=maybe', /Invalid playable filter/],
      ['/songs?era=abc', /Invalid era filter/],
      ['/songs?eraFrom=abc', /Invalid starting era filter/],
      ['/songs?eraTo=abc', /Invalid ending era filter/],
      [`/songs?q=${'x'.repeat(101)}`, /Search query is too long/],
    ];
    for (const [path, pattern] of cases) {
      const res = await req(path);
      assert.equal(res.status, 400, `expected 400 for ${path}`);
      assert.match(await text(res), pattern, path);
    }
    // eraFrom > eraTo
    const res = await req('/songs?eraFrom=5&eraTo=2');
    assert.equal(res.status, 400);
    assert.match(await text(res), /Starting era must not be after ending era/);
  });
});

// ---------------------------------------------------------------------------
// GET /songs/:id
// ---------------------------------------------------------------------------

describe('GET /songs/:id', () => {
  it('returns the song row for a valid id', async () => {
    const res = await req(`/songs/${F.songId}`);
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const song = await json(res);
    assert.equal(song.id, F.songId);
    // Drizzle maps DB columns to camelCase JS keys.
    assert.ok('eraId' in song && 'catalogId' in song);
    assert.ok('fileDate' in song && 'leakDate' in song);
    assert.ok('availableLength' in song && 'trackLength' in song);
  });

  it('400 on invalid id, 404 on missing song', async () => {
    let res = await req('/songs/abc');
    assert.equal(res.status, 400);
    assert.match(await text(res), /Invalid song id/);

    res = await req('/songs/0');
    assert.equal(res.status, 400);

    res = await req(`/songs/${MISSING_ID}`);
    assert.equal(res.status, 404);
    assert.match(await text(res), /Song not found/);
  });
});

// ---------------------------------------------------------------------------
// GET /songs/:id/stream
// ---------------------------------------------------------------------------

describe('GET /songs/:id/stream', () => {
  it('400 on invalid id, 404 on missing song', async () => {
    let res = await req('/songs/abc/stream');
    assert.equal(res.status, 400);
    assert.match(await text(res), /Invalid song id/);

    res = await req(`/songs/${MISSING_ID}/stream`);
    assert.equal(res.status, 404);
    assert.match(await text(res), /Song not found/);
  });

  it('404 when the file is not stored (non-playable song)', async () => {
    // NOTE: this intentionally exercises the missing-file path, which resets
    // the file DB row for re-download. Run against a DB copy.
    const res = await req(`/songs/${F.nonPlayableSongId}/stream`);
    assert.ok([404, 500].includes(res.status), `expected 404/500 for missing file, got ${res.status}`);
    await res.arrayBuffer().catch(() => {});
  });

  it('serves bytes with ETag + range support when playable (skips if none)', async (t) => {
    if (F.playableSongId == null) {
      t.skip('no playable songs on this server');
      return;
    }
    const res = await req(`/songs/${F.playableSongId}/stream`);
    assert.equal(res.status, 200);
    const etag = res.headers.get('etag');
    assert.ok(etag, 'ETag required');
    assert.equal(res.headers.get('cache-control'), 'public, max-age=31536000, immutable');
    assert.equal(res.headers.get('accept-ranges'), 'bytes');
    assert.ok(res.headers.get('content-type'), 'Content-Type required');
    const full = await res.arrayBuffer();
    assert.ok(full.byteLength > 0);

    // Range request.
    const end = Math.min(99, full.byteLength - 1);
    const ranged = await req(`/songs/${F.playableSongId}/stream`, {
      headers: { Range: `bytes=0-${end}` },
    });
    assert.equal(ranged.status, 206);
    assert.equal(ranged.headers.get('content-range'), `bytes 0-${end}/${full.byteLength}`);
    const partial = await ranged.arrayBuffer();
    assert.equal(partial.byteLength, end + 1);

    // Conditional request.
    const notModified = await req(`/songs/${F.playableSongId}/stream`, {
      headers: { 'If-None-Match': etag },
    });
    assert.equal(notModified.status, 304);
    await notModified.arrayBuffer().catch(() => {});

    // Malformed range.
    const badRange = await req(`/songs/${F.playableSongId}/stream`, {
      headers: { Range: 'bytes=nonsense' },
    });
    assert.equal(badRange.status, 416);
    await badRange.arrayBuffer().catch(() => {});
  });

  it('400 on invalid transcode quality when playable (skips if none)', async (t) => {
    if (F.playableSongId == null) {
      t.skip('no playable songs on this server');
      return;
    }
    for (const q of ['abc', '7', '321', '0']) {
      const res = await req(`/songs/${F.playableSongId}/stream?quality=${q}`);
      assert.equal(res.status, 400, `expected 400 for quality=${q}`);
      assert.match(await text(res), /Invalid quality for file/);
    }
  });

  it('transcodes to audio/opus when playable (skips if none)', async (t) => {
    if (F.playableSongId == null) {
      t.skip('no playable songs on this server');
      return;
    }
    const res = await req(`/songs/${F.playableSongId}/stream?quality=32`);
    // 200 with opus body, or 503 when transcoding slots are full.
    assert.ok([200, 503].includes(res.status), `unexpected status ${res.status}`);
    if (res.status === 200) {
      assert.equal(res.headers.get('content-type'), 'audio/opus');
      assert.equal(res.headers.get('cache-control'), 'no-store');
    }
    await res.arrayBuffer().catch(() => {});
  });
});

// ---------------------------------------------------------------------------
// GET /songs/:id/download
// ---------------------------------------------------------------------------

describe('GET /songs/:id/download', () => {
  it('400 on invalid id, 404 on missing song', async () => {
    let res = await req('/songs/abc/download');
    assert.equal(res.status, 400);
    assert.match(await text(res), /Invalid song id/);

    res = await req(`/songs/${MISSING_ID}/download`);
    assert.equal(res.status, 404);
    assert.match(await text(res), /Song not found/);
  });

  it('404 when the file is not stored (non-playable song)', async () => {
    const res = await req(`/songs/${F.nonPlayableSongId}/download`);
    assert.equal(res.status, 404);
    assert.match(await text(res), /Song file not found/);
    await res.arrayBuffer().catch(() => {});
  });

  it('serves attachment with ETag when playable (skips if none)', async (t) => {
    if (F.playableSongId == null) {
      t.skip('no playable songs on this server');
      return;
    }
    const res = await req(`/songs/${F.playableSongId}/download`);
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('content-type'), 'application/octet-stream');
    assert.match(res.headers.get('content-disposition') ?? '', /attachment; filename="song-\d+/);
    assert.ok(res.headers.get('etag'));
    assert.equal(res.headers.get('cache-control'), 'public, max-age=31536000, immutable');
    const bytes = await res.arrayBuffer();
    assert.ok(bytes.byteLength > 0);
  });
});

// ---------------------------------------------------------------------------
// GET /songs/:id/duration
// ---------------------------------------------------------------------------

describe('GET /songs/:id/duration', () => {
  it('400 on invalid id, 404 on missing song', async () => {
    let res = await req('/songs/abc/duration');
    assert.equal(res.status, 400);
    assert.match(await text(res), /Invalid song id/);

    res = await req(`/songs/${MISSING_ID}/duration`);
    assert.equal(res.status, 404);
    assert.match(await text(res), /Song not found/);
  });

  it('404 when the file is not stored (non-playable song)', async () => {
    const res = await req(`/songs/${F.nonPlayableSongId}/duration`);
    // Non-playable: either no files row ("Could not find file") or missing
    // file on disk ("Song file not found"). Both are 404.
    assert.equal(res.status, 404);
    assert.match(await text(res), /file/i);
  });

  it('returns { duration } with immutable caching when playable (skips if none)', async (t) => {
    if (F.playableSongId == null) {
      t.skip('no playable songs on this server');
      return;
    }
    const res = await req(`/songs/${F.playableSongId}/duration`);
    assert.ok([200, 422].includes(res.status), `unexpected status ${res.status}`);
    if (res.status === 200) {
      assert.equal(res.headers.get('cache-control'), 'public, max-age=86400, immutable');
      const body = await json(res);
      assert.equal(typeof body.duration, 'number');
      assert.ok(body.duration > 0);
    } else {
      assert.match(await text(res), /Could not determine file duration/);
    }
  });
});

// ---------------------------------------------------------------------------
// GET /categories
// ---------------------------------------------------------------------------

describe('GET /categories', () => {
  it('returns category list with expected shape', async () => {
    const res = await req('/categories');
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const cats = await json(res);
    assert.ok(Array.isArray(cats) && cats.length > 0);
    const ids = new Set();
    for (const c of cats) {
      assert.equal(typeof c.id, 'string');
      assert.equal(typeof c.name, 'string');
      assert.equal(typeof c.description, 'string');
      assert.equal(typeof c.songsCount, 'number');
      assert.match(c.sourceUrl, /^https:\/\/yetracker\.net\/#gid=\d+$/);
      ids.add(c.id);
    }
    assert.ok(!ids.has('unreleased'), 'primary catalog excluded from list');
    assert.ok(!ids.has('album-copies'), 'album-copies excluded from list');
  });
});

// ---------------------------------------------------------------------------
// GET /categories/:id
// ---------------------------------------------------------------------------

describe('GET /categories/:id', () => {
  it('returns a single category for a valid id', async () => {
    const res = await req(`/categories/${F.categoryId}`);
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const cat = await json(res);
    assert.equal(cat.id, F.categoryId);
    assert.equal(typeof cat.songsCount, 'number');
    assert.match(cat.sourceUrl, /^https:\/\/yetracker\.net\/#gid=\d+$/);
  });

  it('404 on unknown id and on the primary catalog', async () => {
    let res = await req('/categories/no-such-category');
    assert.equal(res.status, 404);
    assert.match(await text(res), /Category does not exist/);

    res = await req('/categories/unreleased');
    assert.equal(res.status, 404);
  });
});

// ---------------------------------------------------------------------------
// GET /categories/:id/songs
// ---------------------------------------------------------------------------

describe('GET /categories/:id/songs', () => {
  it('returns paginated category songs with X-Total-Count', async () => {
    const res = await req(`/categories/${F.categoryId}/songs?limit=2`);
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const total = Number(res.headers.get('x-total-count'));
    assert.ok(Number.isSafeInteger(total) && total >= 0);
    const songs = await json(res);
    assert.ok(Array.isArray(songs) && songs.length <= 2);
    for (const s of songs) {
      assert.equal(s.catalogId, F.categoryId);
      assert.equal(typeof s.playable, 'boolean');
      if (!s.playable) assert.equal(s.duration, null);
    }
  });

  it('q filters and invalid inputs are handled', async () => {
    const res = await req(`/categories/${F.categoryId}/songs?limit=1`);
    assert.equal(res.status, 200);
    const songs = await json(res);
    if (songs.length > 0) {
      const probe = (songs[0].name ?? '').split(/\s+/).find((w) => w.length >= 3) ?? 'a';
      const filtered = await req(`/categories/${F.categoryId}/songs?q=${encodeURIComponent(probe)}&limit=5`);
      assert.equal(filtered.status, 200);
    }

    let bad = await req(`/categories/${F.categoryId}/songs?limit=abc`);
    assert.equal(bad.status, 400);
    bad = await req(`/categories/${F.categoryId}/songs?q=${'x'.repeat(101)}`);
    assert.equal(bad.status, 400);
    assert.match(await text(bad), /Search query is too long/);
  });

  it('404 on unknown category and primary catalog', async () => {
    let res = await req('/categories/no-such-category/songs');
    assert.equal(res.status, 404);
    res = await req('/categories/unreleased/songs');
    assert.equal(res.status, 404);
  });
});

// ---------------------------------------------------------------------------
// GET /album-copies
// ---------------------------------------------------------------------------

describe('GET /album-copies', () => {
  it('returns groups of copies with cover and playback fields', async () => {
    const res = await req('/album-copies');
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const groups = await json(res);
    assert.ok(Array.isArray(groups) && groups.length > 0);
    const seen = new Set();
    for (const g of groups) {
      assert.equal(typeof g.name, 'string');
      assert.ok(Array.isArray(g.copies) && g.copies.length > 0);
      const key = g.name.trim().replace(/\s+/g, ' ').toLowerCase();
      assert.ok(!seen.has(key), `duplicate group key: ${g.name}`);
      seen.add(key);
      for (const copy of g.copies) {
        assert.equal(typeof copy.id, 'number');
        assert.ok('eraName' in copy);
        assert.ok('coverVersion' in copy);
        assert.equal(typeof copy.playable, 'boolean');
        if (!copy.playable) assert.equal(copy.duration, null);
      }
    }
  });
});
