import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { categoryMarkersIn, normalizeSongCategory } from './songCategories.ts';

describe('categoryMarkersIn', () => {
  test('finds the markers typed anywhere, with or without variation selectors', () => {
    assert.deepEqual(categoryMarkersIn('⭐ glory'), ['⭐']);
    assert.deepEqual(categoryMarkersIn('glory ⭐️'), ['⭐']);
    assert.deepEqual(categoryMarkersIn('🤖🗑️'), ['🗑', '🤖']);
    assert.deepEqual(categoryMarkersIn('🗑'), ['🗑']);
    assert.deepEqual(categoryMarkersIn('???'), []);
    assert.deepEqual(categoryMarkersIn(''), []);
  });

  test('a title carries the markers it starts with', () => {
    const title = '🗑️ 🤖 Song [V2]';
    assert.ok(categoryMarkersIn('🗑').every((marker) => title.includes(marker)));
    assert.ok(!categoryMarkersIn('⭐').every((marker) => title.includes(marker)));
  });
});

describe('normalizeSongCategory', () => {
  test('accepts known ids only', () => {
    assert.equal(normalizeSongCategory(' best-of '), 'best-of');
    assert.equal(normalizeSongCategory('nope'), '');
    assert.equal(normalizeSongCategory(null), '');
  });
});
