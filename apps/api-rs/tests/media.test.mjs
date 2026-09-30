// Contract tests for the media routes (era cover, stream, download, duration).
// See helpers.mjs for how to run the suite and what it expects from the server.
//
// Tool-dependent expectations adapt to the server: without ffmpeg transcodes answer 503 `Transcoding unavailable`,
// without ffprobe `/duration` answers 503 `Duration probing unavailable`. Set MEDIA_TOOLS=present or
// MEDIA_TOOLS=absent to require one of the two behaviours.

import assert from 'node:assert/strict';
import { before, describe, it } from 'node:test';

import { assertError, assertNoStore, discover, F, json, MISSING_ID, req } from './helpers.mjs';

before(discover);

const TOOLS = process.env.MEDIA_TOOLS ?? 'any';
const FILE_ETAG = /^"[0-9a-f]+-[0-9a-f]+"$/;
const TRANSCODE_TYPE = 'audio/ogg; codecs=opus';

/** Drops a response body without reading it (media bodies can be large). */
async function discard(res) {
  await res.body?.cancel().catch(() => {});
}

/** Headers of the stored file of the playable song (HEAD, no body). */
async function fileInfo(path) {
  const res = await req(path, { method: 'HEAD' });
  assert.equal(res.status, 200, `HEAD ${path}`);
  return {
    etag: res.headers.get('etag'),
    lastModified: res.headers.get('last-modified'),
    size: Number(res.headers.get('content-length')),
  };
}

function skipWithoutPlayable(t) {
  if (F.playableSongId == null) {
    t.skip('no playable songs on this server');
    return true;
  }
  return false;
}

// ---------------------------------------------------------------------------
// GET /eras/:id/cover
// ---------------------------------------------------------------------------

describe('GET /eras/:id/cover', () => {
  it('400 on invalid ids and formats, 404 without a cover', async () => {
    await assertError(await req('/eras/abc/cover'), 400, /^Invalid era id$/);
    await assertError(await req('/eras/007/cover'), 400, /^Invalid era id$/);
    await assertError(await req(`/eras/${MISSING_ID}/cover`), 404, /^Cover not found$/);
    await assertError(await req(`/eras/${F.eraId}/cover?format=bmp`), 400, /^Invalid cover format$/);
    const bare = F.eras.find((era) => !era.hasCover);
    if (bare) await assertError(await req(`/eras/${bare.id}/cover`), 404, /^Cover not found$/);
  });

  it('parameters resolve like on the JSON routes: last occurrence wins, trimmed, blank is absent', async () => {
    // `format` is validated before the cover lookup.
    await assertError(await req(`/eras/${MISSING_ID}/cover?format=jpeg&format=bmp`), 400, /^Invalid cover format$/);
    for (const format of ['bmp&format=jpeg', '%20jpeg%20', '%20', 'jpeg&format=']) {
      await assertError(await req(`/eras/${MISSING_ID}/cover?format=${format}`), 404, /^Cover not found$/);
    }
  });

  it('serves the cover with validators; immutable only for the current version', async (t) => {
    if (F.coverEraId == null) {
      t.skip('no era has a cover');
      return;
    }
    const era = F.eras.find((candidate) => candidate.id === F.coverEraId);
    const res = await req(`/eras/${era.id}/cover`);
    assert.equal(res.status, 200);
    assert.match(res.headers.get('content-type'), /^image\/(avif|jpeg|png|webp|gif)$/);
    assert.equal(res.headers.get('cache-control'), 'public, no-cache');
    assert.match(res.headers.get('etag') ?? '', /^"[0-9a-f]+"$/);
    assert.ok(res.headers.get('last-modified'), 'Last-Modified');
    assert.equal(res.headers.get('accept-ranges'), null, 'covers ignore Range');
    const bytes = await res.arrayBuffer();
    assert.equal(Number(res.headers.get('content-length')), bytes.byteLength);
    if (res.headers.get('content-type') === 'image/avif') {
      // The ETag of the primary cover is its version (hash of the bytes).
      assert.equal(res.headers.get('etag'), `"${era.coverVersion}"`);
    }

    const current = await req(`/eras/${era.id}/cover?v=${era.coverVersion}`);
    assert.equal(current.status, 200);
    assert.equal(current.headers.get('cache-control'), 'public, max-age=31536000, immutable');
    await discard(current);

    const stale = await req(`/eras/${era.id}/cover?v=000000000000`);
    assert.equal(stale.headers.get('cache-control'), 'public, no-cache');
    await discard(stale);

    const ranged = await req(`/eras/${era.id}/cover`, { headers: { Range: 'bytes=0-9' } });
    assert.equal(ranged.status, 200, 'Range is ignored');
    assert.equal(ranged.headers.get('content-range'), null);
    assert.equal((await ranged.arrayBuffer()).byteLength, bytes.byteLength);
  });

  it('304 on If-None-Match with the validators', async (t) => {
    if (F.coverEraId == null || !F.coverEtag) {
      t.skip('no cover ETag discovered');
      return;
    }
    const era = F.eras.find((candidate) => candidate.id === F.coverEraId);
    for (const value of [F.coverEtag, `W/${F.coverEtag}`, `"x", ${F.coverEtag}`, '*']) {
      const res = await req(`/eras/${era.id}/cover?v=${era.coverVersion}`, { headers: { 'If-None-Match': value } });
      assert.equal(res.status, 304, `If-None-Match: ${value}`);
      assert.equal(res.headers.get('etag'), F.coverEtag);
      assert.equal(res.headers.get('cache-control'), 'public, max-age=31536000, immutable');
      await discard(res);
    }
  });

  it('format=jpeg serves the JPEG variant or 404', async (t) => {
    if (F.coverEraId == null) {
      t.skip('no era has a cover');
      return;
    }
    const res = await req(`/eras/${F.coverEraId}/cover?format=jpeg`);
    if (res.status === 404) {
      await assertError(res, 404, /^Cover not found$/);
      return;
    }
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('content-type'), 'image/jpeg');
    const bytes = new Uint8Array(await res.arrayBuffer());
    assert.deepEqual([...bytes.slice(0, 3)], [0xff, 0xd8, 0xff], 'JPEG signature');
  });
});

// ---------------------------------------------------------------------------
// GET /songs/:id/stream
// ---------------------------------------------------------------------------

describe('GET /songs/:id/stream', () => {
  it('400 on invalid ids and qualities (validated before the lookup), 404 on missing songs', async () => {
    await assertError(await req('/songs/abc/stream'), 400, /^Invalid song id$/);
    await assertError(await req('/songs/0123/stream'), 400, /^Invalid song id$/);
    await assertError(await req('/songs/%FF/stream'), 400, /^Invalid song id$/);
    await assertError(await req(`/songs/${MISSING_ID}/stream`), 404, /^Song not found$/);
    // Only the bitrates the player offers (64, 128, 192, 256, 320) are accepted.
    // Written like an id: `064` is refused like `0128`.
    for (const quality of ['abc', '7', '8', '137', '321', '0', '064', '0128', '12.5', '%FF', '64&quality=bogus']) {
      await assertError(await req(`/songs/${MISSING_ID}/stream?quality=${quality}`), 400, /^Invalid quality for file$/);
    }
    // Same parameter rules as the JSON routes: the last occurrence wins, values are trimmed, blank is absent.
    for (const quality of ['bogus&quality=64', '%2064%20', '%20', '64&quality=']) {
      await assertError(await req(`/songs/${MISSING_ID}/stream?quality=${quality}`), 404, /^Song not found$/);
    }
    // With a quality: `format=opus|aac` and `start=<seconds>` (0–86400, at most 3 decimals).
    for (const format of ['AAC', 'mp3', 'ogg']) {
      await assertError(await req(`/songs/${MISSING_ID}/stream?quality=64&format=${format}`), 400, /^Invalid format$/);
    }
    for (const start of ['-1', '1e3', '.5', '5.', 'abc', '86400.5', '1.2345']) {
      await assertError(await req(`/songs/${MISSING_ID}/stream?quality=64&start=${start}`), 400, /^Invalid start$/);
    }
    for (const query of ['quality=64&format=aac&start=93.5', 'quality=64&format=%20&start=0', 'format=bogus&start=x']) {
      await assertError(await req(`/songs/${MISSING_ID}/stream?${query}`), 404, /^Song not found$/);
    }
  });

  it('404 Song file not found (never 500) when the song has no stored file', async () => {
    const res = await req(`/songs/${F.nonPlayableSongId}/stream`);
    await assertError(res, 404, /^Song file not found$/);
    const head = await req(`/songs/${F.nonPlayableSongId}/stream`, { method: 'HEAD' });
    assert.equal(head.status, 404);
    const transcode = await req(`/songs/${F.nonPlayableSongId}/stream?quality=64`, { method: 'HEAD' });
    assert.equal(transcode.status, 404);
  });

  it('serves the stored file with validators', async (t) => {
    if (skipWithoutPlayable(t)) return;
    const path = `/songs/${F.playableSongId}/stream`;
    const res = await req(path);
    assert.equal(res.status, 200);
    assert.match(res.headers.get('etag') ?? '', FILE_ETAG);
    assert.equal(res.headers.get('cache-control'), 'public, no-cache');
    assert.equal(res.headers.get('accept-ranges'), 'bytes');
    assert.ok(res.headers.get('last-modified'), 'Last-Modified');
    assert.match(res.headers.get('content-type') ?? '', /^(audio\/|application\/octet-stream)/);
    assert.ok(Number(res.headers.get('content-length')) > 0);
    assert.match(res.headers.get('vary') ?? '', /Origin/);
    await discard(res);

    const head = await req(path, { method: 'HEAD' });
    assert.equal(head.status, 200);
    assert.equal(head.headers.get('etag'), res.headers.get('etag'));
    assert.equal(head.headers.get('content-length'), res.headers.get('content-length'));
  });

  it('serves single byte ranges and ignores the rest', async (t) => {
    if (skipWithoutPlayable(t)) return;
    const path = `/songs/${F.playableSongId}/stream`;
    const { size } = await fileInfo(path);
    const cases = [
      ['bytes=0-99', 0, 99],
      ['Bytes=0-0', 0, 0],
      [`bytes=-10`, size - 10, size - 1],
      [`bytes=${size - 5}-`, size - 5, size - 1],
      [`bytes=0-${size + 1000}`, 0, size - 1],
    ];
    for (const [range, start, end] of cases) {
      if (end - start > 1_000_000) continue;
      const res = await req(path, { headers: { Range: range } });
      assert.equal(res.status, 206, range);
      assert.equal(res.headers.get('content-range'), `bytes ${start}-${end}/${size}`, range);
      assert.equal((await res.arrayBuffer()).byteLength, end - start + 1, range);
    }
    for (const range of ['bytes=nonsense', 'items=0-9', 'bytes=0-1,5-6', 'bytes=5-2', 'bytes=-']) {
      const res = await req(path, { headers: { Range: range } });
      assert.equal(res.status, 200, `${range} is ignored`);
      assert.equal(res.headers.get('content-range'), null);
      assert.equal(Number(res.headers.get('content-length')), size);
      await discard(res);
    }
  });

  it('416 for a range past the end: Content-Range, text/plain, no-store, no validators', async (t) => {
    if (skipWithoutPlayable(t)) return;
    const path = `/songs/${F.playableSongId}/stream`;
    const { size } = await fileInfo(path);
    for (const range of [`bytes=${size}-`, 'bytes=-0', 'bytes=99999999999999999999-']) {
      const res = await req(path, { headers: { Range: range } });
      assert.equal(res.status, 416, range);
      assert.equal(res.headers.get('content-range'), `bytes */${size}`);
      assert.match(res.headers.get('content-type') ?? '', /^text\/plain/);
      assertNoStore(res);
      assert.equal(res.headers.get('etag'), null);
      await discard(res);
    }
  });

  it('304 on If-None-Match (weak comparison, lists, *) and If-Modified-Since', async (t) => {
    if (skipWithoutPlayable(t)) return;
    const path = `/songs/${F.playableSongId}/stream`;
    const { etag, lastModified } = await fileInfo(path);
    const conditions = [
      { 'If-None-Match': etag },
      { 'If-None-Match': `W/${etag}` },
      { 'If-None-Match': `"other", ${etag}` },
      { 'If-None-Match': '*' },
      { 'If-Modified-Since': lastModified },
    ];
    for (const headers of conditions) {
      const res = await req(path, { headers });
      assert.equal(res.status, 304, JSON.stringify(headers));
      assert.equal(res.headers.get('etag'), etag);
      assert.equal(res.headers.get('cache-control'), 'public, no-cache');
      assert.equal(res.headers.get('last-modified'), lastModified);
      await discard(res);
    }
    const changed = await req(path, { headers: { 'If-None-Match': '"other"' }, method: 'HEAD' });
    assert.equal(changed.status, 200);
    // A date later than the server's clock is invalid: no 304.
    const future = new Date(Date.now() + 365 * 86_400_000).toUTCString();
    const ahead = await req(path, { headers: { 'If-Modified-Since': future }, method: 'HEAD' });
    assert.equal(ahead.status, 200, `If-Modified-Since: ${future}`);
  });

  it('HEAD ignores Range and describes the whole file', async (t) => {
    if (skipWithoutPlayable(t)) return;
    const path = `/songs/${F.playableSongId}/stream`;
    const { etag, size } = await fileInfo(path);
    for (const range of ['bytes=0-9', `bytes=${size}-`, 'bytes=-0']) {
      const res = await req(path, { headers: { Range: range }, method: 'HEAD' });
      assert.equal(res.status, 200, `HEAD with Range: ${range}`);
      assert.equal(Number(res.headers.get('content-length')), size);
      assert.equal(res.headers.get('content-range'), null);
      assert.equal(res.headers.get('etag'), etag);
      assert.equal(res.headers.get('accept-ranges'), 'bytes');
    }
    const download = await req(`/songs/${F.playableSongId}/download`, {
      headers: { Range: 'bytes=0-1' },
      method: 'HEAD',
    });
    assert.equal(download.status, 200);
    assert.equal(Number(download.headers.get('content-length')), size);
  });

  it('honours If-Range', async (t) => {
    if (skipWithoutPlayable(t)) return;
    const path = `/songs/${F.playableSongId}/stream`;
    const { etag, lastModified, size } = await fileInfo(path);
    const expectations = [
      [etag, 206],
      [lastModified, 206],
      ['"stale"', 200],
      [`W/${etag}`, 200],
    ];
    for (const [ifRange, status] of expectations) {
      const res = await req(path, { headers: { Range: 'bytes=0-9', 'If-Range': ifRange } });
      assert.equal(res.status, status, `If-Range: ${ifRange}`);
      if (status === 206) assert.equal(res.headers.get('content-range'), `bytes 0-9/${size}`);
      await discard(res);
    }
  });

  it('transcodes to Ogg Opus, then serves the cached transcode like a file', async (t) => {
    if (skipWithoutPlayable(t)) return;
    const path = `/songs/${F.playableSongId}/stream?quality=64`;
    const head = await req(path, { method: 'HEAD' });
    if (head.status === 503 && head.headers.get('retry-after') === null) {
      assert.notEqual(TOOLS, 'present', 'ffmpeg should be available');
      await assertError(await req(path), 503, /^Transcoding unavailable$/);
      return;
    }
    // A transcode cached earlier is served like a file, with or without ffmpeg.
    const cachedAlready = head.status === 200 && head.headers.get('content-length') !== null;
    if (!cachedAlready) assert.notEqual(TOOLS, 'absent', 'ffmpeg should be missing');
    if (head.status === 503) {
      t.skip('every transcode slot is busy');
      return;
    }
    assert.equal(head.status, 200);
    assert.equal(head.headers.get('content-type'), TRANSCODE_TYPE);

    const res = await req(path);
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('content-type'), TRANSCODE_TYPE);
    if (res.headers.get('content-length') === null) {
      // Live: streamed while ffmpeg writes it.
      assert.equal(res.headers.get('accept-ranges'), 'none');
      assert.equal(res.headers.get('cache-control'), 'no-store');
    }
    const body = new Uint8Array(await res.arrayBuffer());
    assert.equal(new TextDecoder().decode(body.slice(0, 4)), 'OggS', 'Ogg container');

    // Once complete it is cached: a file response with validators and ranges.
    let cached;
    for (let attempt = 0; attempt < 20; attempt += 1) {
      cached = await req(path, { method: 'HEAD' });
      if (cached.headers.get('content-length') !== null) break;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    assert.equal(cached.status, 200);
    assert.equal(Number(cached.headers.get('content-length')), body.byteLength);
    assert.equal(cached.headers.get('accept-ranges'), 'bytes');
    assert.equal(cached.headers.get('cache-control'), 'public, no-cache');
    assert.match(cached.headers.get('etag') ?? '', FILE_ETAG);
    const ranged = await req(path, { headers: { Range: 'bytes=0-3' } });
    assert.equal(ranged.status, 206);
    assert.equal(ranged.headers.get('content-range'), `bytes 0-3/${body.byteLength}`);
    assert.equal(new TextDecoder().decode(await ranged.arrayBuffer()), 'OggS');
    const notModified = await req(path, { headers: { 'If-None-Match': cached.headers.get('etag') } });
    assert.equal(notModified.status, 304);
  });

  it('HEAD never starts a transcode', async (t) => {
    if (skipWithoutPlayable(t)) return;
    // Find a bitrate nobody transcoded yet (a HEAD without Content-Length).
    let quality = null;
    for (const candidate of [128, 192, 256, 320].sort(() => Math.random() - 0.5)) {
      if (quality !== null) break;
      const res = await req(`/songs/${F.playableSongId}/stream?quality=${candidate}`, { method: 'HEAD' });
      if (res.status === 503) {
        t.skip('transcoding unavailable or busy');
        return;
      }
      if (res.headers.get('content-length') === null) quality = candidate;
    }
    if (quality === null) {
      t.skip('every probed bitrate is cached already');
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 3000));
    const again = await req(`/songs/${F.playableSongId}/stream?quality=${quality}`, { method: 'HEAD' });
    assert.equal(again.status, 200);
    assert.equal(again.headers.get('content-length'), null, 'still not transcoded, so HEAD did not start ffmpeg');
    assert.equal(again.headers.get('cache-control'), 'no-store');
  });
});

// ---------------------------------------------------------------------------
// GET /songs/:id/download
// ---------------------------------------------------------------------------

describe('GET /songs/:id/download', () => {
  it('400 on invalid id, 404 on missing song or file', async () => {
    await assertError(await req('/songs/abc/download'), 400, /^Invalid song id$/);
    await assertError(await req(`/songs/${MISSING_ID}/download`), 404, /^Song not found$/);
    await assertError(await req(`/songs/${F.nonPlayableSongId}/download`), 404, /^Song file not found$/);
  });

  it('serves an attachment named after the song, with ranges', async (t) => {
    if (skipWithoutPlayable(t)) return;
    const path = `/songs/${F.playableSongId}/download`;
    const res = await req(path, { method: 'HEAD' });
    assert.equal(res.status, 200);
    assert.equal(res.headers.get('content-type'), 'application/octet-stream');
    const disposition = res.headers.get('content-disposition') ?? '';
    assert.match(disposition, /^attachment; filename="[^"\\/]+"; filename\*=UTF-8''[A-Za-z0-9!#$&+\-.^_`|~%]+$/);
    const name = decodeURIComponent(disposition.split("UTF-8''")[1]);
    assert.ok(name.length <= 130 && !/[\n\r<>:"/\\|?*]/.test(name), `safe name: ${name}`);
    assert.match(res.headers.get('etag') ?? '', FILE_ETAG);
    assert.equal(res.headers.get('cache-control'), 'public, no-cache');
    const size = Number(res.headers.get('content-length'));

    const ranged = await req(path, { headers: { Range: 'bytes=0-1' } });
    assert.equal(ranged.status, 206);
    assert.equal(ranged.headers.get('content-range'), `bytes 0-1/${size}`);
    assert.equal((await ranged.arrayBuffer()).byteLength, 2);

    const unsatisfiable = await req(path, { headers: { Range: `bytes=${size}-` } });
    assert.equal(unsatisfiable.status, 416);
    assert.equal(unsatisfiable.headers.get('content-range'), `bytes */${size}`);
    await discard(unsatisfiable);
  });
});

// ---------------------------------------------------------------------------
// GET /songs/:id/duration
// ---------------------------------------------------------------------------

describe('GET /songs/:id/duration', () => {
  it('400 on invalid id, 404 on missing song or file', async () => {
    await assertError(await req('/songs/abc/duration'), 400, /^Invalid song id$/);
    await assertError(await req(`/songs/${MISSING_ID}/duration`), 404, /^Song not found$/);
    await assertError(await req(`/songs/${F.nonPlayableSongId}/duration`), 404, /^Song file not found$/);
  });

  it('returns { duration } (revalidated, with an ETag) or 503 without ffprobe', async (t) => {
    if (skipWithoutPlayable(t)) return;
    const path = `/songs/${F.playableSongId}/duration`;
    const res = await req(path);
    if (res.status === 503) {
      assert.notEqual(TOOLS, 'present', 'ffprobe should be available');
      await assertError(res, 503, /^Duration probing unavailable$/);
      return;
    }
    assert.equal(res.status, 200);
    // The song's file can be replaced under the same id, so the duration is never immutable.
    assert.equal(res.headers.get('cache-control'), 'public, no-cache');
    const etag = res.headers.get('etag');
    assert.match(etag ?? '', /^W\/"[0-9a-f]{32}"$/);
    const body = await json(res);
    assert.equal(typeof body.duration, 'number');
    assert.ok(body.duration > 0);
    const again = await req(path, { headers: { 'If-None-Match': etag } });
    assert.equal(again.status, 304);
    assert.equal(again.headers.get('etag'), etag);
    assert.equal(again.headers.get('cache-control'), 'public, no-cache');
    await discard(again);
    // Stored once probed: the song payload reports it too.
    const song = await json(await req(`/songs/${F.playableSongId}`));
    assert.equal(song.duration, body.duration);
  });
});
