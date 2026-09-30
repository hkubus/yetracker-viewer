import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
  type EraInfo,
  eraHref,
  firstLine,
  hexColor,
  positiveInt,
  positiveNumber,
  sanitizeTrack,
  type Track,
  trackFromDataset,
  trackFromSong,
} from './track.ts';

const ERA: EraInfo = {
  eraId: 31,
  eraName: 'DONDA 2 [V1]',
  color: 'a1b2c3',
  hasCover: true,
  coverVersion: '915367c1e635',
};

const TRACK: Track = {
  id: 6765,
  title: 'NEBRASKA [V4]',
  eraId: 31,
  eraName: 'DONDA 2 [V1]',
  eraPosition: 441,
  length: 225,
  color: '666666',
  hasCover: false,
  coverVersion: null,
  pageHref: null,
};

describe('number and text helpers', () => {
  test('positiveInt accepts positive safe integers and canonical strings only', () => {
    assert.equal(positiveInt(5), 5);
    assert.equal(positiveInt('6765'), 6765);
    assert.equal(positiveInt(' 42 '), 42);
    for (const value of [0, -1, 1.5, '0', '007', '1e3', '', 'abc', null, undefined, Number.NaN, 2 ** 53]) {
      assert.equal(positiveInt(value), null, String(value));
    }
  });

  test('positiveNumber treats 0, empty and invalid values as unknown', () => {
    assert.equal(positiveNumber('185.2'), 185.2);
    assert.equal(positiveNumber(207), 207);
    for (const value of ['0', 0, '', '  ', 'x', null, undefined, -3, Number.POSITIVE_INFINITY]) {
      assert.equal(positiveNumber(value), null, String(value));
    }
  });

  test('hexColor normalizes 6-hex colors', () => {
    assert.equal(hexColor('#A1B2C3'), 'a1b2c3');
    assert.equal(hexColor(' ffffff '), 'ffffff');
    assert.equal(hexColor('fff'), null);
    assert.equal(hexColor('rgb(1, 2, 3)'), null);
    assert.equal(hexColor(undefined), null);
  });

  test('firstLine returns the first non-empty line, whitespace collapsed', () => {
    assert.equal(firstLine('NEBRASKA [V4]\n(feat. Pusha T)'), 'NEBRASKA [V4]');
    assert.equal(firstLine('\n  Title  with   spaces \r\nsecond'), 'Title with spaces');
    assert.equal(firstLine(''), '');
    assert.equal(firstLine(undefined), '');
  });
});

describe('trackFromDataset', () => {
  test('reads the SPEC 6.2 attributes', () => {
    const track = trackFromDataset({
      id: '6765',
      title: 'NEBRASKA [V4]\n(feat. Pusha T)',
      eraId: '31',
      eraName: 'DONDA 2 [V1]',
      eraPosition: '441',
      trackLength: '225',
      dominantColor: 'A1B2C3',
      hasCover: 'true',
      coverVersion: '915367c1e635',
    });
    assert.deepEqual(track, {
      id: 6765,
      title: 'NEBRASKA [V4]',
      eraId: 31,
      eraName: 'DONDA 2 [V1]',
      eraPosition: 441,
      length: 225,
      color: 'a1b2c3',
      hasCover: true,
      coverVersion: '915367c1e635',
      pageHref: null,
    });
  });

  test('accepts older markup: data-eraId, no position/color/has-cover, length 0', () => {
    const track = trackFromDataset(
      { id: '6765', title: 'Song', eraid: '31', eraName: 'Era', trackLength: '0', coverVersion: '666666' },
      { color: ' #336699 ' },
    );
    assert.ok(track);
    assert.equal(track.eraId, 31);
    assert.equal(track.eraPosition, null);
    assert.equal(track.length, null);
    assert.equal(track.color, '336699');
    assert.equal(track.hasCover, true, 'a cover version implies a cover when data-has-cover is absent');
  });

  test('explicit data-has-cover="false" wins; missing id → null; untitled fallback', () => {
    assert.equal(trackFromDataset({ id: '1', hasCover: 'false', coverVersion: 'abc' })?.hasCover, false);
    assert.equal(trackFromDataset({ id: '1' })?.title, 'Untitled');
    assert.equal(trackFromDataset({ id: '1' })?.color, '666666');
    assert.equal(trackFromDataset({ title: 'x' }), null);
    assert.equal(trackFromDataset({ id: 'abc' }), null);
  });
});

describe('trackFromSong', () => {
  test('v2 payload: title, eraPosition, duration preferred over trackLength', () => {
    const track = trackFromSong(
      {
        id: 6965,
        eraId: 31,
        eraPosition: 641,
        name: 'TRUE LOVE [V23]\n(feat. X)',
        title: 'TRUE LOVE [V23]',
        playable: true,
        duration: 196.4,
        trackLength: 196,
      },
      ERA,
    );
    assert.deepEqual(track, {
      id: 6965,
      title: 'TRUE LOVE [V23]',
      eraId: 31,
      eraName: 'DONDA 2 [V1]',
      eraPosition: 641,
      length: 196.4,
      color: 'a1b2c3',
      hasCover: true,
      coverVersion: '915367c1e635',
      pageHref: null,
    });
  });

  test('v1 payload: first line of name, fallback position or page link', () => {
    const song = { id: 6965, eraId: 31, name: 'TRUE LOVE [V23] (feat. X)', playable: true, trackLength: 196 };
    assert.equal(trackFromSong(song, ERA, { position: 641 })?.eraPosition, 641);
    const linked = trackFromSong(song, ERA, { pageHref: '/eras/31?q=love&page=2' });
    assert.equal(linked?.eraPosition, null);
    assert.equal(linked?.pageHref, '/eras/31?q=love&page=2');
    assert.equal(linked?.title, 'TRUE LOVE [V23] (feat. X)');
  });

  test('songs that are not playable (or invalid) are skipped', () => {
    assert.equal(trackFromSong({ id: 1, playable: false }, ERA), null);
    assert.equal(trackFromSong({ id: 1 }, ERA), null);
    assert.equal(trackFromSong({ id: 'x', playable: true }, ERA), null);
    assert.equal(trackFromSong(null, ERA), null);
  });
});

describe('sanitizeTrack', () => {
  test('round-trips a valid track', () => {
    assert.deepEqual(sanitizeTrack(JSON.parse(JSON.stringify(TRACK))), TRACK);
  });

  test('rejects or repairs invalid stored values', () => {
    assert.equal(sanitizeTrack({ title: 'no id' }), null);
    assert.equal(sanitizeTrack('nope'), null);
    const repaired = sanitizeTrack({
      id: 3,
      title: 42,
      color: 'red',
      hasCover: true,
      coverVersion: '<script>',
      pageHref: '//evil.example/x',
    });
    assert.deepEqual(repaired, {
      id: 3,
      title: 'Untitled',
      eraId: null,
      eraName: '',
      eraPosition: null,
      length: null,
      color: '666666',
      hasCover: false,
      coverVersion: null,
      pageHref: null,
    });
  });
});

describe('eraHref', () => {
  test('links to the page that lists the song', () => {
    assert.equal(eraHref(TRACK), '/eras/31?page=5#song-6765');
    assert.equal(eraHref({ ...TRACK, eraPosition: 100 }), '/eras/31#song-6765');
    assert.equal(eraHref({ ...TRACK, eraPosition: 101 }), '/eras/31?page=2#song-6765');
    assert.equal(eraHref({ ...TRACK, eraPosition: 101 }, 50), '/eras/31?page=3#song-6765');
  });

  test('falls back to the stored page link, then to the era', () => {
    assert.equal(
      eraHref({ ...TRACK, eraPosition: null, pageHref: '/eras/31?page=5#old' }),
      '/eras/31?page=5#song-6765',
    );
    assert.equal(eraHref({ ...TRACK, eraPosition: null }), '/eras/31#song-6765');
    assert.equal(eraHref({ ...TRACK, eraId: null }), null);
  });
});
