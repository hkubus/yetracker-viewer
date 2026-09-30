import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { eraPageOf, playButtonAttributes, positiveInteger, songHref, songTextLines } from './songDisplay.ts';

describe('positiveInteger', () => {
  test('accepts positive safe integers and canonical strings only', () => {
    assert.equal(positiveInteger(31), 31);
    assert.equal(positiveInteger('31'), 31);
    for (const bad of [0, -1, 1.5, Number.NaN, '031', '', ' 3', null, undefined, 2 ** 53, {}]) {
      assert.equal(positiveInteger(bad), null, String(bad));
    }
  });
});

describe('songTextLines', () => {
  test('splits multi-line names into title and details', () => {
    assert.deepEqual(
      songTextLines({
        name: 'NEBRASKA [V4]\n(feat. Pusha T)\n  (Alternate titles: Nebraska 2)  ',
        title: 'NEBRASKA [V4]',
      }),
      { title: 'NEBRASKA [V4]', details: ['(feat. Pusha T)', '(Alternate titles: Nebraska 2)'] },
    );
  });

  test('falls back to the first line of the name, then to "Untitled"', () => {
    assert.deepEqual(songTextLines({ name: 'PABLO [V3] (feat. Future)' }), {
      title: 'PABLO [V3] (feat. Future)',
      details: [],
    });
    assert.deepEqual(songTextLines({ name: '\n\nSecond\r\nThird' }), { title: 'Second', details: ['Third'] });
    assert.deepEqual(songTextLines({ name: null, title: null }), { title: 'Untitled', details: [] });
    assert.deepEqual(songTextLines({ title: '  ' }), { title: 'Untitled', details: [] });
  });
});

describe('songHref', () => {
  test('links to the page that contains the song', () => {
    assert.equal(eraPageOf(1), 1);
    assert.equal(eraPageOf(100), 1);
    assert.equal(eraPageOf(101), 2);
    assert.equal(eraPageOf(412, 50), 9);
    assert.equal(eraPageOf(null), 1);
    assert.equal(songHref({ id: 6751, eraId: 31, eraPosition: 412 }), '/eras/31?page=5#song-6751');
    assert.equal(songHref({ id: 1, eraId: 2, eraPosition: 100 }), '/eras/2#song-1');
    assert.equal(songHref({ id: 1, eraId: 2 }), '/eras/2#song-1');
  });

  test('returns null without usable ids', () => {
    assert.equal(songHref({ id: 1, eraId: null }), null);
    assert.equal(songHref({ id: 0, eraId: 2 }), null);
  });
});

describe('playButtonAttributes', () => {
  const song = {
    id: 6751,
    eraId: 31,
    eraPosition: 412,
    name: 'NEBRASKA [V4]\n(feat. Pusha T)',
    title: 'NEBRASKA [V4]',
    eraName: 'DONDA 2 [V1]',
    dominantColor: 'A1B2C3',
    eraHasCover: true,
    eraCoverVersion: '0123456789ab',
    trackLength: 185,
    duration: 185.4,
  };

  test('emits every attribute of the player contract', () => {
    assert.deepEqual(playButtonAttributes(song), {
      'data-play-target': '',
      'data-id': '6751',
      'data-title': 'NEBRASKA [V4]',
      'data-era-id': '31',
      'data-era-name': 'DONDA 2 [V1]',
      'data-era-position': '412',
      'data-track-length': '185',
      'data-dominant-color': 'A1B2C3',
      'data-has-cover': 'true',
      'data-cover-version': '0123456789ab',
      'aria-pressed': 'false',
      title: 'Play',
    });
  });

  test('fills gaps of older payloads from the era and falls back safely', () => {
    const attributes = playButtonAttributes(
      { id: 5, eraId: 2, name: 'Song', trackLength: 61.6, dominantColor: 'nope' },
      { name: 'Era', dominantColor: '112233', coverVersion: 'abc' },
    );
    assert.equal(attributes?.['data-era-name'], 'Era');
    assert.equal(attributes?.['data-dominant-color'], '666666');
    assert.equal(attributes?.['data-track-length'], '61', 'fractions are cut off, like formatDuration');
    assert.equal(
      playButtonAttributes({ id: 5, trackLength: 200, duration: 185.4 })?.['data-track-length'],
      '185',
      'the probed duration describes the file',
    );
    assert.equal(attributes?.['data-era-position'], '');
    assert.equal(attributes?.['data-has-cover'], 'true');
    assert.equal(attributes?.['data-cover-version'], 'abc');
    const noCover = playButtonAttributes({ id: 5, eraHasCover: false, eraCoverVersion: null }, { coverVersion: 'x' });
    assert.equal(noCover?.['data-has-cover'], 'false');
    assert.equal(noCover?.['data-cover-version'], '');
    assert.equal(noCover?.['data-track-length'], '');
    assert.equal(playButtonAttributes({ id: null }), null);
  });
});
