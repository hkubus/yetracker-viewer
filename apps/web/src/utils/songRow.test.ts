import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { formatDuration } from './duration.ts';
import {
  countLabel,
  DOWNLOAD_STATE_LABELS,
  downloadStateOf,
  httpUrl,
  isPlayable,
  linkLabel,
  linkNotes,
  playbackLength,
  songLength,
  songLinks,
  songName,
} from './songRow.ts';

describe('songName', () => {
  test('splits the title line from credit and alternate-title lines', () => {
    assert.deepEqual(
      songName({
        name: 'NEBRASKA [V4]\n(feat. Pusha T)\n(Alternate titles: Nebraska 2)',
        title: 'NEBRASKA [V4]',
      }),
      { title: 'NEBRASKA [V4]', details: ['(feat. Pusha T)', '(Alternate titles: Nebraska 2)'] },
    );
  });

  test('derives the title from the first line of older one-line names', () => {
    assert.deepEqual(songName({ name: '⭐ Home [V3] (feat. John Legend)' }), {
      title: '⭐ Home [V3] (feat. John Legend)',
      details: [],
    });
    assert.deepEqual(songName({ name: '  A \n\n B  ' }), { title: 'A', details: ['B'] });
  });

  test('falls back to "Untitled"', () => {
    assert.deepEqual(songName({}), { title: 'Untitled', details: [] });
    assert.deepEqual(songName({ name: ' \n ', title: '' }), { title: 'Untitled', details: [] });
  });
});

describe('links', () => {
  test('httpUrl accepts only absolute http(s) URLs', () => {
    assert.equal(httpUrl('https://imgur.gg/f/abc'), 'https://imgur.gg/f/abc');
    assert.equal(httpUrl(' http://example.com '), 'http://example.com/');
    assert.equal(httpUrl('javascript:alert(1)'), null);
    assert.equal(httpUrl('/relative'), null);
    assert.equal(httpUrl('not a url'), null);
    assert.equal(httpUrl(null), null);
  });

  test('songLinks puts the primary link first and deduplicates', () => {
    assert.deepEqual(
      songLinks({ url: 'https://imgur.gg/f/a', links: ['https://imgur.gg/f/a', 'https://youtu.be/x', 'ftp://x/y'] }),
      ['https://imgur.gg/f/a', 'https://youtu.be/x'],
    );
    assert.deepEqual(songLinks({ url: 'https://pillows.su/f/1' }), ['https://pillows.su/f/1']);
    assert.deepEqual(songLinks({ url: null, links: [] }), []);
  });

  test('linkLabel is host (without www.) plus path and query', () => {
    assert.equal(linkLabel('https://imgur.gg/f/abc123'), 'imgur.gg/f/abc123');
    assert.equal(linkLabel('https://www.youtube.com/watch?v=xyz'), 'youtube.com/watch?v=xyz');
    assert.equal(linkLabel('https://pillows.su/'), 'pillows.su');
    assert.equal(linkLabel('https://example.com/a%20b'), 'example.com/a b');
  });
});

describe('download state', () => {
  test('uses downloadState from current payloads', () => {
    for (const state of ['none', 'unsupported', 'pending', 'failed'] as const) {
      assert.equal(downloadStateOf({ id: 1, downloadState: state, playable: false }), state);
    }
    assert.equal(downloadStateOf({ id: 1, downloadState: 'downloaded', playable: true }), 'downloaded');
    assert.equal(downloadStateOf({ id: 1, downloadState: 'downloaded', playable: false }), 'pending');
  });

  test('derives a state for older payloads', () => {
    assert.equal(downloadStateOf({ id: 1, playable: true, downloaded: 1 }), 'downloaded');
    assert.equal(downloadStateOf({ id: 1, playable: false, url: null, downloaded: null }), 'none');
    assert.equal(
      downloadStateOf({ id: 1, playable: false, url: 'https://imgur.gg/f/a', downloaded: null }),
      'unsupported',
    );
    assert.equal(downloadStateOf({ id: 1, playable: false, url: 'https://pillows.su/f/a', downloaded: 0 }), 'pending');
    assert.equal(downloadStateOf({ id: 1, playable: false, url: 'https://pillows.su/f/a', downloaded: 1 }), 'pending');
  });

  test('isPlayable prefers the playable flag', () => {
    assert.equal(isPlayable({ playable: true }), true);
    assert.equal(isPlayable({ playable: false, downloadState: 'downloaded' }), false);
    assert.equal(isPlayable({ downloadState: 'downloaded' }), true);
    assert.equal(isPlayable({}), false);
  });

  test('every non-playable state has a label', () => {
    for (const state of ['none', 'unsupported', 'pending', 'failed'] as const) {
      assert.ok(DOWNLOAD_STATE_LABELS[state].length > 0);
    }
  });
});

describe('lengths', () => {
  test('songLength prefers the catalog length and keeps its approximate flag', () => {
    assert.deepEqual(songLength({ trackLength: 120, trackLengthApprox: true, duration: 118.4 }), {
      text: '2:00',
      approx: true,
    });
    assert.deepEqual(songLength({ trackLength: null, duration: 3725.9 }), { text: '1:02:05', approx: false });
    assert.equal(songLength({ trackLength: 0, duration: null }), null);
    assert.equal(songLength({}), null);
  });

  test('playbackLength prefers the probed duration', () => {
    assert.equal(playbackLength({ trackLength: 120, duration: 118.6 }), 118);
    assert.equal(playbackLength({ trackLength: 120, duration: null }), 120);
    assert.equal(playbackLength({ trackLength: null, duration: Number.NaN }), null);
    assert.equal(playbackLength({ trackLength: null, duration: 0.4 }), null, 'under a second: left to the player');
  });

  test('playbackLength and songLength agree with formatDuration (fractions are never rounded up)', () => {
    // 321.77 s is 5:21 on the audio element's clock; rounding would show 5:22 until the file's own length is known.
    const length = playbackLength({ trackLength: null, duration: 321.769229 });
    assert.equal(length, 321);
    assert.equal(formatDuration(length), formatDuration(321.769229));
    assert.equal(songLength({ trackLength: null, duration: 321.769229 })?.text, '5:21');
    assert.equal(formatDuration(playbackLength({ trackLength: null, duration: 59.9999 })), '0:59');
  });
});

describe('linkNotes', () => {
  test('turns occurrences of the link texts into link segments', () => {
    const notes = 'Played during the Common vs. Kanye freestyle battle.\nSee the video.';
    const { segments, extraLinks } = linkNotes(notes, [
      { text: 'the video', url: 'https://youtu.be/v' },
      { text: 'the Common vs. Kanye freestyle battle', url: 'https://imgur.gg/f/nhOhAwL' },
    ]);
    assert.deepEqual(segments, [
      { text: 'Played during ' },
      { text: 'the Common vs. Kanye freestyle battle', href: 'https://imgur.gg/f/nhOhAwL' },
      { text: '.\nSee ' },
      { text: 'the video', href: 'https://youtu.be/v' },
      { text: '.' },
    ]);
    assert.deepEqual(extraLinks, []);
    assert.equal(segments.map((segment) => segment.text).join(''), notes);
  });

  test('repeated link texts claim successive occurrences', () => {
    const { segments } = linkNotes('link and link', [
      { text: 'link', url: 'https://a.example/' },
      { text: 'link', url: 'https://b.example/' },
    ]);
    assert.deepEqual(segments, [
      { text: 'link', href: 'https://a.example/' },
      { text: ' and ' },
      { text: 'link', href: 'https://b.example/' },
    ]);
  });

  test('drops unsafe URLs and returns links whose text is missing', () => {
    const { segments, extraLinks } = linkNotes('Some notes', [
      { text: 'Some', url: 'javascript:alert(1)' },
      { text: 'elsewhere', url: 'https://x.example/a' },
      { text: '', url: 'https://y.example/b' },
    ]);
    assert.deepEqual(segments, [{ text: 'Some notes' }]);
    assert.deepEqual(extraLinks, [
      { text: 'elsewhere', href: 'https://x.example/a' },
      { text: 'y.example/b', href: 'https://y.example/b' },
    ]);
  });

  test('handles missing links', () => {
    assert.deepEqual(linkNotes('plain', undefined), { segments: [{ text: 'plain' }], extraLinks: [] });
    assert.deepEqual(linkNotes('', []), { segments: [], extraLinks: [] });
  });
});

test('countLabel pluralizes and groups digits', () => {
  assert.equal(countLabel(1, 'song'), '1 song');
  assert.equal(countLabel(0, 'song'), '0 songs');
  assert.equal(countLabel(9650, 'song'), '9,650 songs');
  assert.equal(countLabel(2, 'match', 'matches'), '2 matches');
});
