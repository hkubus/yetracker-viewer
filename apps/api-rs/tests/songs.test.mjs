// Contract tests for the song listing, search and detail routes.
// See helpers.mjs for how to run the suite and what it expects from the server.

import assert from 'node:assert/strict';
import { before, describe, it } from 'node:test';

import {
  assertError,
  assertSearchSong,
  assertSong,
  catalog,
  discover,
  F,
  fold,
  JSON_CACHE,
  json,
  MISSING_ID,
  req,
  totalCount,
} from './helpers.mjs';

before(discover);

async function search(query) {
  const res = await req(`/songs?${query}`);
  assert.equal(res.status, 200, `GET /songs?${query}`);
  assert.equal(res.headers.get('cache-control'), JSON_CACHE);
  const body = await json(res);
  assert.deepEqual(Object.keys(body).sort(), ['limit', 'offset', 'songs', 'total'], 'search envelope');
  assert.ok(Array.isArray(body.songs));
  assert.ok(Number.isSafeInteger(body.total) && body.total >= body.songs.length);
  assert.equal(totalCount(res), body.total, 'X-Total-Count equals total');
  for (const song of body.songs) assertSearchSong(song);
  return body;
}

/** Every result of a search, page by page (50 at a time). */
async function searchAll(query) {
  const first = await search(`${query}&limit=50`);
  const songs = [...first.songs];
  for (let offset = 50; offset < first.total; offset += 50) {
    const page = await search(`${query}&limit=50&offset=${offset}`);
    assert.equal(page.total, first.total, 'total is stable across pages');
    assert.equal(page.offset, offset);
    songs.push(...page.songs);
  }
  return { total: first.total, songs };
}

const q = (value) => `q=${encodeURIComponent(value)}`;
const ids = (songs) => songs.map((song) => song.id);

/** The folded text a search matches against: name, notes, era name + subtitle, sub-era, quality, length. */
function haystack(song) {
  const era = F.eras.find((candidate) => candidate.id === song.eraId);
  return fold(
    [song.name, song.notes, era?.name ?? song.eraName, era?.subtitle, song.subEra, song.quality, song.availableLength]
      .filter(Boolean)
      .join(' '),
  );
}

// ---------------------------------------------------------------------------
// GET /songs (plain list mode)
// ---------------------------------------------------------------------------

describe('GET /songs plain list', () => {
  it('returns a bare array of v2 songs in catalog order with X-Total-Count', async () => {
    const res = await req('/songs?limit=200');
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const total = totalCount(res);
    const songs = await json(res);
    assert.ok(Array.isArray(songs) && songs.length === Math.min(200, total));
    const positions = new Map(F.eras.map((era) => [era.id, era.position]));
    songs.forEach((song, index) => {
      assertSong(song);
      assert.ok(!('eraName' in song), 'plain mode has no search fields');
      if (index === 0) return;
      const previous = songs[index - 1];
      if (song.eraId === previous.eraId) {
        assert.equal(song.eraPosition, previous.eraPosition + 1, `catalog order inside era at ${index}`);
      } else {
        assert.equal(song.eraPosition, 1, `a new era starts at its first song (index ${index})`);
        assert.ok(positions.get(song.eraId) > positions.get(previous.eraId), 'eras follow their positions');
      }
    });
    assert.equal(totalCount(await req('/songs')), total, 'total does not depend on the page');
  });

  it('keeps the real total past the end and bounds the offset', async () => {
    const { total } = await catalog();
    for (const offset of [total, total + 1, 10_000].filter((offset) => offset <= 10_000)) {
      const res = await req(`/songs?limit=5&offset=${offset}`);
      assert.equal(res.status, 200);
      assert.equal(totalCount(res), total, `X-Total-Count at offset ${offset}`);
      if (offset >= total) assert.deepEqual(await json(res), []);
    }
    await assertError(await req('/songs?offset=10001'), 400, /^Invalid offset$/);
  });

  it('sorts and validates pagination', async () => {
    const res = await req('/songs?limit=50&sort=leak-newest');
    assert.equal(res.status, 200);
    const songs = await json(res);
    const dates = songs.map((song) => song.leakDate);
    const firstEmpty = dates.indexOf(null);
    if (firstEmpty !== -1)
      assert.ok(
        dates.slice(firstEmpty).every((date) => date === null),
        'undated last',
      );
    const dated = dates.filter((date) => date !== null);
    for (let index = 1; index < dated.length; index += 1) {
      assert.ok(dated[index] <= dated[index - 1], 'newest leaks first');
    }
    const a = await json(await req('/songs?limit=1&offset=0'));
    const b = await json(await req('/songs?limit=1&offset=1'));
    assert.notEqual(a[0].id, b[0].id);
    for (const [query, pattern] of [
      ['limit=abc', /^Invalid limit$/],
      ['limit=0', /^Invalid limit$/],
      ['offset=abc', /^Invalid offset$/],
      ['sort=bogus', /^Invalid sort$/],
    ]) {
      await assertError(await req(`/songs?${query}`), 400, pattern);
    }
    const { total } = await catalog();
    const clamped = await json(await req('/songs?limit=9999'));
    assert.equal(clamped.length, Math.min(500, total), 'plain limit is clamped to 500');
  });

  it('blank q stays in plain mode', async () => {
    const res = await req('/songs?limit=2&q=%20%20');
    assert.equal(res.status, 200);
    assert.ok(Array.isArray(await json(res)));
  });
});

// ---------------------------------------------------------------------------
// GET /songs (search / filter mode)
// ---------------------------------------------------------------------------

describe('GET /songs search', () => {
  it('q returns the envelope with every match counted', async () => {
    const body = await search(`${q('love')}&limit=5`);
    assert.equal(body.offset, 0);
    assert.equal(body.limit, 5);
    assert.ok(body.songs.length <= 5);
    for (const song of body.songs) assert.ok(haystack(song).includes('love'), `${song.title} matches love`);

    const empty = await search(q('zzzz-no-such-song-xyz-123'));
    assert.deepEqual(empty.songs, []);
    assert.equal(empty.total, 0);
  });

  it('pages through every match: offset works and total is exact', async () => {
    const { total } = await catalog();
    const broad = await search(`${q('a')}&limit=1`);
    assert.ok(broad.total > 1000 || broad.total === total, 'no 1000-row cap');
    const { songs, total: matches } = await searchAll(q('king'));
    assert.equal(songs.length, matches);
    assert.equal(new Set(songs.map((song) => song.id)).size, matches, 'no song appears twice');
    for (const song of songs) assert.ok(haystack(song).includes('king'), `${song.title} matches king`);
    const past = await search(`${q('king')}&offset=${matches + 10}`);
    assert.deepEqual(past.songs, []);
    assert.equal(past.total, matches);
  });

  it('matching folds case, accents, apostrophes and punctuation', async () => {
    const variants = [
      ['can’t tell me nothing', "can't tell me nothing", 'Cant Tell Me Nothing', 'CAN`T-TELL-ME NOTHING'],
      ['beyonce', 'Beyoncé', 'BEYONCÉ'],
      ['jay z', 'JAŸ-Z', 'jay-z'],
    ];
    for (const spellings of variants) {
      const [reference, ...others] = await Promise.all(spellings.map((spelling) => search(`${q(spelling)}&limit=50`)));
      for (const [index, body] of others.entries()) {
        assert.equal(body.total, reference.total, `${spellings[index + 1]} vs ${spellings[0]}`);
        assert.deepEqual(
          body.songs.map((song) => song.id),
          reference.songs.map((song) => song.id),
        );
      }
    }
  });

  it('tokens match anywhere, in any order, all required', async () => {
    const { songs } = await catalog();
    const sample = songs.find((song) => /^[A-Za-z][\w ]+ \[V\d+\]$/.test(song.title));
    assert.ok(sample, 'need a plain versioned title');
    const [base, version] = sample.title.split(' [');
    const phrase = `${fold(base)} ${fold(version)}`; // e.g. "nebraska v4"
    const exact = await search(`${q(phrase)}&limit=50`);
    assert.ok(exact.songs.length > 0);
    assert.equal(fold(exact.songs[0].title), fold(sample.title), 'the exact title ranks first');
    assert.ok(exact.songs.some((song) => song.id === sample.id));
    const reversed = await search(`${q(phrase.split(' ').reverse().join(' '))}&limit=50`);
    assert.equal(reversed.total, exact.total);
    for (const song of reversed.songs) {
      for (const token of phrase.split(' ')) assert.ok(haystack(song).includes(token), `${song.title} has ${token}`);
    }
    const none = await search(`${q(`${phrase} zzqxjw`)}`);
    assert.equal(none.total, 0);
  });

  it('a query without letters, digits or category markers counts as blank', async () => {
    const { total } = await catalog();
    const plain = ids(await json(await req('/songs?limit=3')));
    for (const value of ['???', ' - ', '★ …', '\u200B']) {
      const res = await req(`/songs?${q(value)}&limit=3`);
      assert.equal(res.status, 200, `q=${JSON.stringify(value)}`);
      assert.equal(totalCount(res), total);
      const body = await json(res);
      assert.ok(Array.isArray(body), `q=${JSON.stringify(value)} is the plain list, not a search matching everything`);
      assert.deepEqual(ids(body), plain);
    }
    // With a filter it is filter-only search mode, exactly as without q.
    const era = F.eras[0];
    const filtered = await search(`${q('???')}&era=${era.id}&limit=5`);
    const reference = await search(`era=${era.id}&limit=5`);
    assert.equal(filtered.total, reference.total);
    assert.deepEqual(ids(filtered.songs), ids(reference.songs));
  });

  it('category markers in q filter like category (AND), with or without variation selectors', async () => {
    const { songs } = await catalog();
    for (const [category, marker] of [
      ['best-of', '⭐'],
      ['worst-of', '🗑'],
      ['ai', '🤖'],
    ]) {
      const expected = ids(songs.filter((song) => song.title.includes(marker)));
      const found = await searchAll(q(marker));
      assert.equal(found.total, expected.length, `q=${marker} counts category=${category}`);
      assert.deepEqual(ids(found.songs), expected, `q=${marker} lists category=${category} in catalog order`);
      for (const spelled of [`${marker}\uFE0F`, ` ${marker}\uFE0E ??? `]) {
        const body = await search(`${q(spelled)}&limit=50`);
        assert.equal(body.total, expected.length, `q=${JSON.stringify(spelled)}`);
        assert.deepEqual(ids(body.songs), expected.slice(0, 50));
      }
    }
    // Text and markers combine with AND, like q + category.
    const combined = await search(`${q('⭐️ a')}&limit=50`);
    const reference = await search(`${q('a')}&category=best-of&limit=50`);
    assert.equal(combined.total, reference.total);
    assert.deepEqual(ids(combined.songs), ids(reference.songs));
    // Several markers (or a marker plus category): songs carrying all of them.
    const both = await searchAll(q('🗑️🤖'));
    const expected = ids(songs.filter((song) => song.title.includes('🗑') && song.title.includes('🤖')));
    assert.deepEqual(ids(both.songs), expected);
    const withCategory = await search(`${q('🗑️')}&category=ai&limit=50`);
    assert.equal(withCategory.total, expected.length);
  });

  it('q is limited to 100 characters and must decode', async () => {
    await search(q('x'.repeat(100)));
    await assertError(await req(`/songs?${q('x'.repeat(101))}`), 400, /^Search query is too long$/);
    await assertError(await req(`/songs?${q(`${'é'.repeat(99)}ab`)}`), 400, /^Search query is too long$/);
    await assertError(await req('/songs?q=%FF'), 400, /^Invalid search query$/);
  });

  it('limit defaults to 50 and is capped at 50', async () => {
    const body = await search(`${q('a')}&limit=100`);
    assert.equal(body.limit, 50);
    assert.equal(body.songs.length, Math.min(50, body.total));
    const defaults = await search(q('a'));
    assert.equal(defaults.limit, 50);
    await assertError(await req(`/songs?${q('a')}&offset=10001`), 400, /^Invalid offset$/);
  });

  it('playable filter finds every playable song, before pagination', async () => {
    const { songs } = await catalog();
    const playable = songs.filter((song) => song.playable).map((song) => song.id);
    const { songs: found, total } = await searchAll('playable=true');
    assert.equal(total, playable.length, 'playable=true total');
    assert.deepEqual(
      found.map((song) => song.id),
      playable,
      'playable songs in catalog order',
    );
    for (const song of found) {
      assert.equal(song.playable, true);
      assert.equal(song.downloadState, 'downloaded');
    }
    const status = await json(await req('/status'));
    assert.equal(status.playableSongs, playable.length);
    const unplayable = await search('playable=false&limit=5');
    assert.equal(unplayable.total + total, songs.length);
    for (const song of unplayable.songs) assert.equal(song.playable, false);
    const recent = await search('playable=true&sort=leak-newest&limit=8');
    assert.equal(recent.total, playable.length);
    const dates = recent.songs.map((song) => song.leakDate).filter((date) => date !== null);
    for (let index = 1; index < dates.length; index += 1) assert.ok(dates[index] <= dates[index - 1]);
  });

  it('era, eraFrom and eraTo filter by era and by era position', async () => {
    const byPosition = [...F.eras].sort((a, b) => a.position - b.position);
    const era = byPosition[Math.floor(byPosition.length / 2)];
    const single = await search(`era=${era.id}&limit=50`);
    assert.equal(single.total, era.songsCount);
    for (const song of single.songs) assert.equal(song.eraId, era.id);

    const from = byPosition[1] ?? byPosition[0];
    const to = byPosition[Math.min(3, byPosition.length - 1)];
    const inRange = byPosition.filter(
      (candidate) => candidate.position >= from.position && candidate.position <= to.position,
    );
    const range = await search(`eraFrom=${from.id}&eraTo=${to.id}&limit=50`);
    assert.equal(
      range.total,
      inRange.reduce((sum, candidate) => sum + candidate.songsCount, 0),
      'the range counts every song of the eras between the two positions',
    );
    for (const song of range.songs) assert.ok(inRange.some((candidate) => candidate.id === song.eraId));

    const tail = await search(`eraFrom=${byPosition.at(-1).id}&limit=1`);
    assert.equal(tail.total, byPosition.at(-1).songsCount);
    const head = await search(`eraTo=${byPosition[0].id}&limit=1`);
    assert.equal(head.total, byPosition[0].songsCount);

    // Filters combine with the query.
    const combined = await search(`${q('a')}&eraFrom=${from.id}&eraTo=${to.id}&limit=50`);
    assert.ok(combined.total <= range.total);
    for (const song of combined.songs) assert.ok(inRange.some((candidate) => candidate.id === song.eraId));
  });

  it('category filters search results by marker', async () => {
    const { songs } = await catalog();
    const expected = songs.filter((song) => song.title.includes('⭐')).map((song) => song.id);
    const found = await searchAll('category=best-of');
    assert.equal(found.total, expected.length);
    assert.deepEqual(
      found.songs.map((song) => song.id),
      expected,
    );
    const combined = await search(`${q('a')}&category=best-of&limit=50`);
    for (const song of combined.songs) assert.ok(song.title.includes('⭐'));
  });

  it('quality and availability filters work', async () => {
    let body = await search('quality=CD%20Quality&limit=20');
    assert.ok(body.total > 0);
    for (const song of body.songs) assert.equal(song.quality, 'CD Quality');
    body = await search('availability=Snippet&limit=20');
    for (const song of body.songs) assert.equal(song.availableLength, 'Snippet');
  });

  it('eraPosition points at the song in its era listing', async () => {
    const body = await search(`era=${F.eraId}&quality=CD%20Quality&limit=3`);
    for (const song of body.songs) {
      const page = await json(await req(`/eras/${song.eraId}/songs?limit=1&offset=${song.eraPosition - 1}`));
      assert.equal(page[0]?.id, song.id, `eraPosition ${song.eraPosition} of song ${song.id}`);
    }
    const ranked = await search(`${q('love')}&limit=10`);
    for (const song of ranked.songs.slice(0, 3)) {
      const page = await json(await req(`/eras/${song.eraId}/songs?limit=1&offset=${song.eraPosition - 1}`));
      assert.equal(page[0]?.id, song.id);
    }
  });

  it('search results carry the era display data', async () => {
    const body = await search(`era=${F.eraId}&limit=1`);
    const era = F.eras.find((candidate) => candidate.id === F.eraId);
    const [song] = body.songs;
    assert.equal(song.eraName, era.name);
    assert.equal(song.dominantColor, era.dominantColor);
    assert.equal(song.eraHasCover, era.hasCover);
    assert.equal(song.eraCoverVersion, era.coverVersion);
  });

  it('400 on invalid filters', async () => {
    const cases = [
      ['/songs?quality=Bogus', /^Invalid quality filter$/],
      ['/songs?availability=Bogus', /^Invalid availability filter$/],
      ['/songs?playable=maybe', /^Invalid playable filter$/],
      ['/songs?category=bogus', /^Invalid category filter$/],
      ['/songs?era=abc', /^Invalid era filter$/],
      ['/songs?era=01', /^Invalid era filter$/],
      ['/songs?eraFrom=abc', /^Invalid starting era filter$/],
      ['/songs?eraTo=abc', /^Invalid ending era filter$/],
      ['/songs?era=%FF', /^Invalid era filter$/],
      ['/songs?eraFrom=%C3', /^Invalid starting era filter$/],
      [`/songs?eraFrom=${MISSING_ID}`, /^Invalid starting era filter$/],
      [`/songs?eraTo=${MISSING_ID}`, /^Invalid ending era filter$/],
      ['/songs?playable=true&sort=bogus', /^Invalid sort$/],
    ];
    for (const [path, pattern] of cases) {
      await assertError(await req(path), 400, pattern);
    }
    const byPosition = [...F.eras].sort((a, b) => a.position - b.position);
    if (byPosition.length > 1) {
      await assertError(
        await req(`/songs?eraFrom=${byPosition.at(-1).id}&eraTo=${byPosition[0].id}`),
        400,
        /^Starting era must not be after ending era$/,
      );
    }
  });
});

// ---------------------------------------------------------------------------
// Query parameters: blank, padded and repeated values
// ---------------------------------------------------------------------------

describe('GET /songs query parameters', () => {
  it('blank values count as absent, for every parameter', async () => {
    const { total } = await catalog();
    const plain = ids(await json(await req('/songs?limit=5')));
    for (const name of ['q', 'era', 'eraFrom', 'eraTo', 'quality', 'availability', 'playable', 'category', 'sort']) {
      for (const blank of ['', '%20', '+%09+']) {
        const res = await req(`/songs?${name}=${blank}&limit=5`);
        assert.equal(res.status, 200, `${name}=${blank}`);
        assert.equal(totalCount(res), total, `${name}=${blank}`);
        const body = await json(res);
        assert.ok(Array.isArray(body), `${name}=${blank} stays in plain mode`);
        assert.deepEqual(ids(body), plain, `${name}=${blank}`);
      }
    }
    for (const name of ['limit', 'offset']) {
      const res = await req(`/songs?${name}=%20`);
      assert.equal(res.status, 200, `${name}=%20`);
      assert.equal((await json(res)).length, Math.min(100, total), `${name}=%20 is the default`);
    }
    // In search mode too: a blank filter doesn't narrow the search.
    const reference = await search(`${q('a')}&limit=5`);
    for (const name of ['era', 'eraFrom', 'eraTo', 'quality', 'availability', 'playable', 'category', 'sort']) {
      const body = await search(`${q('a')}&${name}=%20&limit=5`);
      assert.equal(body.total, reference.total, `q=a&${name}=%20`);
      assert.deepEqual(ids(body.songs), ids(reference.songs));
    }
  });

  it('values are trimmed', async () => {
    const era = F.eras[0];
    for (const [padded, exact] of [
      [`era=%20${era.id}%20`, `era=${era.id}`],
      [`eraFrom=+${era.id}+`, `eraFrom=${era.id}`],
      ['quality=%20CD%20Quality%09', 'quality=CD%20Quality'],
      ['playable=+true+', 'playable=true'],
      ['category=%20best-of%20', 'category=best-of'],
    ]) {
      const body = await search(`${padded}&limit=5`);
      const reference = await search(`${exact}&limit=5`);
      assert.equal(body.total, reference.total, padded);
      assert.deepEqual(ids(body.songs), ids(reference.songs), padded);
    }
    const sorted = await json(await req('/songs?sort=%20name%20&limit=5'));
    assert.deepEqual(ids(sorted), ids(await json(await req('/songs?sort=name&limit=5'))));
  });

  it('the last occurrence of a repeated parameter wins', async () => {
    const byName = ids(await json(await req('/songs?sort=name&limit=5')));
    const last = await req('/songs?sort=bogus&sort=name&limit=5');
    assert.equal(last.status, 200);
    assert.deepEqual(ids(await json(last)), byName);
    await assertError(await req('/songs?sort=name&sort=bogus'), 400, /^Invalid sort$/);
    // A blank last occurrence clears the parameter.
    const cleared = await json(await req('/songs?sort=name&sort=&limit=5'));
    assert.deepEqual(ids(cleared), ids(await json(await req('/songs?limit=5'))));
    // Decoding follows the winning occurrence.
    assert.equal((await req('/songs?q=%FF&q=love&limit=1')).status, 200);
    await assertError(await req('/songs?q=love&q=%FF'), 400, /^Invalid search query$/);
  });
});

// ---------------------------------------------------------------------------
// GET /songs/:id
// ---------------------------------------------------------------------------

describe('GET /songs/:id', () => {
  it('returns the same song object as the era listing', async () => {
    const res = await req(`/songs/${F.songId}`);
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('cache-control'), JSON_CACHE);
    const song = await json(res);
    assertSong(song);
    assert.equal(song.id, F.songId);
    const listed = await json(await req(`/eras/${song.eraId}/songs?limit=1&offset=${song.eraPosition - 1}`));
    assert.deepEqual(song, listed[0]);
    if (F.playableSongId !== null) {
      const playable = await json(await req(`/songs/${F.playableSongId}`));
      assertSong(playable);
      assert.equal(playable.playable, true);
      assert.equal(playable.downloadState, 'downloaded');
    }
  });

  it('400 on invalid ids, 404 on a missing song', async () => {
    for (const bad of ['abc', '0', '01', '-1', '1.5', '%FF', '9007199254740992']) {
      await assertError(await req(`/songs/${bad}`), 400, /^Invalid song id$/);
    }
    await assertError(await req(`/songs/${MISSING_ID}`), 404, /^Song not found$/);
  });
});
