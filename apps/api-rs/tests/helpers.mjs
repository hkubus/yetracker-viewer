// Shared helpers for the black-box HTTP contract tests of the YeTracker Viewer API (Rust/axum).
// The tests themselves live in the sibling `*.test.mjs` files (eras, songs, media).
//
// Run against a running API server, from the repo root:
//   SYNC_ON_START=false SYNC_INTERVAL_MINUTES=0 STORAGE_DIR=/tmp/yt-test API_PORT=3100 \
//     ./apps/api-rs/target/release/yetracker-api &
//   API_BASE_URL=http://127.0.0.1:3100 node --test 'apps/api-rs/tests/*.test.mjs'
//
// Env:
//   API_BASE_URL (fallback: BASE_URL, fallback: http://127.0.0.1:3000)
//
// Notes:
// - Tests are adaptive: fixtures are discovered from plain list endpoints (`/eras`, `/songs` pages) instead of
//   being hardcoded, so they work with any catalog.
// - Route errors are `text/plain` with the message (e.g. "Invalid era id") and `Cache-Control: no-store`.
//   Unknown routes answer 404 JSON `{"error":"Not found"}`, wrong methods 405 JSON `{"error":"Method not allowed"}`.
// - Run against a COPY of the storage directory, never production data: the media suite requests files that may
//   be missing on disk.
// - When no playable files exist on disk, media *success* tests skip gracefully; validation/404 tests still run.

import assert from 'node:assert/strict';

export const BASE = process.env.API_BASE_URL ?? process.env.BASE_URL ?? 'http://127.0.0.1:3000';

export const JSON_CACHE = 'public, max-age=60, s-maxage=300, stale-while-revalidate=600';
export const MISSING_ID = 999_999_999;
/** Page size used to walk the plain song list. */
export const SCAN_PAGE = 500;
/** Largest `offset` the API accepts. */
export const MAX_OFFSET = 10_000;

export async function req(path, init) {
  const res = await fetch(`${BASE}${path}`, init);
  return res;
}

export async function json(res) {
  assert.match(res.headers.get('content-type') ?? '', /application\/json/, `expected JSON content-type for ${res.url}`);
  return res.json();
}

export async function text(res) {
  return res.text();
}

/** `X-Total-Count` as a number (asserts it is present and a non-negative integer). */
export function totalCount(res) {
  const raw = res.headers.get('x-total-count');
  assert.match(raw ?? '', /^\d+$/, `X-Total-Count missing or malformed on ${res.url}: ${raw}`);
  return Number(raw);
}

/** A route error: status, `text/plain` body matching `pattern`, never cached. */
export async function assertError(res, status, pattern) {
  assert.equal(res.status, status, `expected ${status} for ${res.url}`);
  assert.match(res.headers.get('content-type') ?? '', /^text\/plain/, `error content-type for ${res.url}`);
  assertNoStore(res);
  assert.match(await res.text(), pattern, res.url);
}

export function assertNoStore(res) {
  assert.equal(res.headers.get('cache-control'), 'no-store', `expected Cache-Control: no-store on ${res.url}`);
}

// SPEC §3.6 fold(): the API matches folded query tokens against folded song text. Good enough for the ASCII-ish
// probes the tests derive from the catalog.
const APOSTROPHES = /['’‘ʼ`´]/g;
const SPELLED = {
  Ø: 'o',
  ø: 'o',
  Æ: 'ae',
  æ: 'ae',
  Œ: 'oe',
  œ: 'oe',
  ß: 'ss',
  Ł: 'l',
  ł: 'l',
  Đ: 'd',
  đ: 'd',
  Þ: 'th',
  þ: 'th',
  ı: 'i',
};
export function fold(value) {
  return (value ?? '')
    .replace(APOSTROPHES, '')
    .normalize('NFKD')
    .replace(/\p{M}/gu, '')
    .replace(/[ØøÆæŒœßŁłĐđÞþı]/g, (character) => SPELLED[character])
    .toLowerCase()
    .replace(APOSTROPHES, '')
    .replace(/[\u200B-\u200D\u2060\uFEFF]|\uFE0E|\uFE0F/g, '')
    .replace(/[^\p{L}\p{N}]+/gu, ' ')
    .trim();
}

/** Category order of a title: best-of, special, grails, wanted, unmarked, worst-of, AI (best leading marker). */
const MARKERS = [
  ['⭐', 0],
  ['✨', 1],
  ['🏆', 2],
  ['🏅', 3],
  ['🗑', 5],
  ['🤖', 6],
];
export function categoryRank(title) {
  let rest = title ?? '';
  let rank = null;
  for (;;) {
    rest = rest.replace(/^(?:[\s\u200B-\u200D\u2060\uFEFF]|\uFE0E|\uFE0F)+/u, '');
    const marker = MARKERS.find(([emoji]) => rest.startsWith(emoji));
    if (!marker) return rank ?? 4;
    rank = rank === null ? marker[1] : Math.min(rank, marker[1]);
    rest = rest.slice(marker[0].length);
  }
}

/** `songs.sort_title`: markers dropped, folded, digit runs zero-padded to 10 digits. */
export function naturalKey(title) {
  const markers = new RegExp(`^(?:[\\s\\uFE0E\\uFE0F]|${MARKERS.map(([emoji]) => emoji).join('|')})+`, 'u');
  return fold((title ?? '').replace(markers, '')).replace(/\d+/g, (digits) =>
    digits.replace(/^0+/, '').padStart(10, '0'),
  );
}

const SONG_KEYS = [
  'id',
  'eraId',
  'eraPosition',
  'catalogId',
  'name',
  'title',
  'subEra',
  'notes',
  'notesLinks',
  'fileDate',
  'fileDatePrecision',
  'leakDate',
  'leakDatePrecision',
  'availableLength',
  'trackLength',
  'trackLengthApprox',
  'quality',
  'url',
  'links',
  'downloadState',
  'playable',
  'duration',
];
const DOWNLOAD_STATES = ['none', 'unsupported', 'pending', 'failed', 'downloaded'];
const PRECISIONS = ['day', 'month', 'year'];

const isStringOrNull = (value) => value === null || typeof value === 'string';

/** Contract v2 `Song` (SPEC §3.2): every key present, nullable fields null, internals stripped. */
export function assertSong(song) {
  for (const key of SONG_KEYS) assert.ok(key in song, `song ${song.id} lacks ${key}`);
  for (const internal of ['downloaded', 'filename', 'fileDuration', 'searchText', 'sortTitle', 'songKey', 'position']) {
    assert.ok(!(internal in song), `song ${song.id} leaks ${internal}`);
  }
  assert.ok(Number.isSafeInteger(song.id) && song.id > 0);
  assert.ok(Number.isSafeInteger(song.eraId));
  assert.ok(Number.isSafeInteger(song.eraPosition) && song.eraPosition >= 1);
  assert.equal(song.catalogId, 'unreleased');
  assert.equal(typeof song.name, 'string');
  assert.equal(song.title, song.name.split('\n')[0], `title is the first line of ${JSON.stringify(song.name)}`);
  assert.ok(isStringOrNull(song.subEra));
  assert.equal(typeof song.notes, 'string');
  assert.ok(Array.isArray(song.notesLinks));
  for (const link of song.notesLinks) {
    assert.equal(typeof link.text, 'string');
    assert.match(link.url, /^https?:\/\//);
  }
  for (const [date, precision] of [
    ['fileDate', 'fileDatePrecision'],
    ['leakDate', 'leakDatePrecision'],
  ]) {
    if (song[date] === null) {
      assert.equal(song[precision], null, `${precision} must be null without ${date} (song ${song.id})`);
    } else {
      assert.ok(Number.isSafeInteger(song[date]), `${date} of song ${song.id}`);
      assert.ok(PRECISIONS.includes(song[precision]), `${precision} of song ${song.id}: ${song[precision]}`);
    }
  }
  assert.ok(isStringOrNull(song.availableLength));
  assert.ok(song.trackLength === null || Number.isSafeInteger(song.trackLength));
  assert.equal(typeof song.trackLengthApprox, 'boolean');
  assert.ok(isStringOrNull(song.quality));
  assert.ok(Array.isArray(song.links));
  for (const link of song.links) assert.match(link, /^https?:\/\//);
  assert.equal(new Set(song.links).size, song.links.length, `links of song ${song.id} are deduplicated`);
  assert.equal(song.url, song.links[0] ?? null, `url is links[0] (song ${song.id})`);
  assert.ok(DOWNLOAD_STATES.includes(song.downloadState), `downloadState ${song.downloadState}`);
  if (song.url === null) assert.ok(['none', 'downloaded'].includes(song.downloadState));
  if (song.url !== null && song.quality === 'Not Available' && !song.playable) {
    assert.equal(song.downloadState, 'unsupported', `song ${song.id} has no audio anywhere`);
  }
  assert.equal(typeof song.playable, 'boolean');
  assert.equal(song.playable, song.downloadState === 'downloaded', `playable ⇔ downloaded (song ${song.id})`);
  assert.ok(song.duration === null || (typeof song.duration === 'number' && song.duration > 0));
  if (!song.playable) assert.equal(song.duration, null, 'non-playable duration must be null');
}

/** Contract v2 `SearchSong`: a `Song` plus the era's display data. */
export function assertSearchSong(song) {
  assertSong(song);
  assert.equal(typeof song.eraName, 'string');
  assert.match(song.dominantColor, /^[0-9a-f]{6}$/i);
  assert.equal(typeof song.eraHasCover, 'boolean');
  assert.ok(isStringOrNull(song.eraCoverVersion));
  assert.equal(song.eraHasCover, song.eraCoverVersion !== null);
}

/** Contract v2 `Era` (SPEC §3.1). */
export function assertEra(era) {
  const keys = [
    'id',
    'position',
    'name',
    'subtitle',
    'notes',
    'description',
    'dominantColor',
    'hasCover',
    'coverVersion',
    'songsCount',
  ];
  for (const key of keys) assert.ok(key in era, `era ${era.id} lacks ${key}`);
  for (const internal of ['imageUrl', 'image_url', 'isMain', 'is_main', 'coverSource', 'key', 'coverAttempts']) {
    assert.ok(!(internal in era), `era ${era.id} leaks ${internal}`);
  }
  assert.ok(Number.isSafeInteger(era.id) && era.id > 0);
  assert.ok(Number.isSafeInteger(era.position));
  assert.equal(typeof era.name, 'string');
  assert.ok(!era.name.includes('\n'), 'era name is one line');
  assert.ok(isStringOrNull(era.subtitle));
  assert.equal(typeof era.notes, 'string');
  assert.equal(typeof era.description, 'string');
  assert.match(era.dominantColor, /^[0-9a-f]{6}$/i);
  assert.equal(typeof era.hasCover, 'boolean');
  if (era.hasCover) assert.match(era.coverVersion, /^[0-9a-f]{12}$/);
  else assert.equal(era.coverVersion, null);
  assert.ok(Number.isSafeInteger(era.songsCount) && era.songsCount >= 0);
}

/**
 * Every song of the plain list (`/songs`, catalog order) up to the API's offset limit, fetched once per process and
 * shared by the tests that need the whole catalog.
 */
let catalogPromise = null;
export function catalog() {
  catalogPromise ??= (async () => {
    const songs = [];
    let total = Number.POSITIVE_INFINITY;
    for (let offset = 0; offset < total && offset <= MAX_OFFSET; offset += SCAN_PAGE) {
      const res = await req(`/songs?limit=${SCAN_PAGE}&offset=${offset}`);
      assert.equal(res.status, 200, `GET /songs?offset=${offset}`);
      total = totalCount(res);
      const page = await json(res);
      assert.ok(Array.isArray(page));
      songs.push(...page);
      if (page.length < SCAN_PAGE) break;
    }
    return { songs, total };
  })();
  return catalogPromise;
}

/** The first playable song of the catalog, or null. */
export async function firstPlayableSong() {
  const { songs } = await catalog();
  return songs.find((song) => song.playable) ?? null;
}

// Discovered fixtures (filled in by discover()).
export const F = {
  eras: [],
  eraId: null,
  songId: null,
  nonPlayableSongId: null,
  playableSongId: null,
  coverEraId: null,
  coverEtag: null,
};

/**
 * Fills `F` from the live server. Each `*.test.mjs` file runs in its own process, so every file registers this in
 * its own top-level `before()` hook.
 */
export async function discover() {
  let health;
  try {
    health = await req('/health');
  } catch (error) {
    throw new Error(
      `API not reachable at ${BASE}: ${error.message}. Start it first, e.g. SYNC_ON_START=false API_PORT=3000 ./apps/api-rs/target/release/yetracker-api`,
    );
  }
  assert.equal(health.status, 200, `GET /health status (body: ${await health.text()})`);

  const erasRes = await req('/eras');
  assert.equal(erasRes.status, 200);
  F.eras = await json(erasRes);
  assert.ok(Array.isArray(F.eras) && F.eras.length > 0, 'expected at least one era');
  F.eraId = F.eras[0].id;

  // Songs from the plain list: stops at the first page with a playable song.
  for (let offset = 0; offset <= MAX_OFFSET; offset += SCAN_PAGE) {
    const res = await req(`/songs?limit=${SCAN_PAGE}&offset=${offset}`);
    assert.equal(res.status, 200);
    const songs = await json(res);
    assert.ok(Array.isArray(songs));
    if (offset === 0) {
      assert.ok(songs.length > 0, 'expected at least one song');
      F.songId = songs[0].id;
    }
    F.nonPlayableSongId ??= songs.find((song) => !song.playable)?.id ?? null;
    F.playableSongId ??= songs.find((song) => song.playable)?.id ?? null;
    if (F.playableSongId !== null || songs.length < SCAN_PAGE) break;
  }
  F.nonPlayableSongId ??= F.songId;

  // An era with a cover, as the era list says.
  const coverEra = F.eras.find((era) => era.hasCover);
  if (coverEra) {
    const coverRes = await req(`/eras/${coverEra.id}/cover`);
    await coverRes.arrayBuffer().catch(() => {});
    if (coverRes.status === 200) {
      F.coverEraId = coverEra.id;
      F.coverEtag = coverRes.headers.get('etag');
    }
  }
}
