import assert from 'node:assert/strict';
import { test } from 'node:test';
import { formatDuration, MISSING_DURATION } from './duration.ts';

test('formats minutes and seconds below an hour', () => {
  assert.equal(formatDuration(0), '0:00');
  assert.equal(formatDuration(5), '0:05');
  assert.equal(formatDuration(59.99), '0:59');
  assert.equal(formatDuration(185.2), '3:05');
  assert.equal(formatDuration(3599), '59:59');
});

test('switches to hours from one hour on', () => {
  assert.equal(formatDuration(3600), '1:00:00');
  assert.equal(formatDuration(3725), '1:02:05');
  assert.equal(formatDuration(36_000 + 61), '10:01:01');
});

test('renders unknown values as a dash', () => {
  assert.equal(MISSING_DURATION, '—');
  for (const value of [null, undefined, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY, -1]) {
    assert.equal(formatDuration(value), '—');
  }
});
