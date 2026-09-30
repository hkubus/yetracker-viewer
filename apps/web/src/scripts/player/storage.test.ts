import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { positionText, qualityLabel, volumeText } from './format.ts';
import { parseQuality, parseSession, parseVolume } from './storage.ts';

const TRACK = {
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

describe('preferences', () => {
  test('parseVolume restores only valid, audible values', () => {
    assert.equal(parseVolume('0.4'), 0.4);
    assert.equal(parseVolume('1'), 1);
    for (const raw of [null, '', ' ', '0', '-0.5', '1.5', 'abc', 'NaN', 'Infinity']) {
      assert.equal(parseVolume(raw), null, String(raw));
    }
  });

  test('parseQuality accepts only offered options', () => {
    const allowed = ['', '64', '128'];
    assert.equal(parseQuality('64', allowed), '64');
    assert.equal(parseQuality('', allowed), '');
    assert.equal(parseQuality('320', allowed), null);
    assert.equal(parseQuality(null, allowed), null);
  });
});

describe('parseSession', () => {
  const now = Date.parse('2026-09-30T10:00:00Z');
  const stored = (overrides: Record<string, unknown> = {}) =>
    JSON.stringify({
      v: 1,
      savedAt: now - 60_000,
      track: TRACK,
      position: 30.5,
      queue: { tracks: [TRACK, { ...TRACK, id: 6766 }], index: 0, continuation: null },
      ...overrides,
    });

  test('restores track, position and queue', () => {
    const session = parseSession(stored(), now);
    assert.equal(session?.track.id, 6765);
    assert.equal(session?.position, 30.5);
    assert.deepEqual(
      session?.queue.tracks.map((t) => t.id),
      [6765, 6766],
    );
    assert.equal(session?.queue.index, 0);
  });

  test('falls back to a one-song queue when the stored queue is unusable', () => {
    const session = parseSession(stored({ queue: { tracks: [{ ...TRACK, id: 1 }], index: 0 } }), now);
    assert.deepEqual(
      session?.queue.tracks.map((t) => t.id),
      [6765],
    );
  });

  test('rejects old, foreign or broken data', () => {
    assert.equal(parseSession(null, now), null);
    assert.equal(parseSession('{', now), null);
    assert.equal(parseSession('42', now), null);
    assert.equal(parseSession(stored({ v: 2 }), now), null);
    assert.equal(parseSession(stored({ savedAt: now - 31 * 24 * 3600 * 1000 }), now), null);
    assert.equal(parseSession(stored({ track: { title: 'no id' } }), now), null);
    assert.equal(parseSession(stored({ position: 'x' }), now)?.position, 0);
    assert.equal(parseSession(stored({ position: -5 }), now)?.position, 0);
  });
});

describe('format', () => {
  test('position text for the slider', () => {
    assert.equal(positionText(83, 225), '1:23 of 3:45');
    assert.equal(positionText(3723, 4200), '1:02:03 of 1:10:00');
    assert.equal(positionText(5, null), '0:05');
    assert.equal(positionText(-1, 0), '0:00');
  });

  test('volume and quality labels', () => {
    assert.equal(volumeText(0.8, false), '80%');
    assert.equal(volumeText(0.8, true), 'Muted');
    assert.equal(qualityLabel(''), 'Original');
    assert.equal(qualityLabel('128'), '128 kbps');
  });
});
