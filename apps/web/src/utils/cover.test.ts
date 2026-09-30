import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { coverUrl, eraInitials } from './cover.ts';

describe('coverUrl', () => {
  test('adds the version and format when given', () => {
    assert.equal(coverUrl('http://localhost:3000', 31), 'http://localhost:3000/eras/31/cover');
    assert.equal(
      coverUrl('http://localhost:3000/', 31, 'a9cf54ab9fba'),
      'http://localhost:3000/eras/31/cover?v=a9cf54ab9fba',
    );
    assert.equal(coverUrl('https://api.example', '7', null, 'jpeg'), 'https://api.example/eras/7/cover?format=jpeg');
    assert.equal(
      coverUrl('https://api.example', 7, 'abc', 'jpeg'),
      'https://api.example/eras/7/cover?v=abc&format=jpeg',
    );
  });

  test('supports same-origin prefixes and escapes the id', () => {
    assert.equal(coverUrl('/api', 3, 'x y'), '/api/eras/3/cover?v=x+y');
    assert.equal(coverUrl('', 3), '/eras/3/cover');
    assert.equal(coverUrl('/api', '1/2'), '/api/eras/1%2F2/cover');
  });
});

describe('eraInitials', () => {
  test('uses the first letters of the first two significant words', () => {
    assert.equal(eraInitials('The Life of Pablo'), 'LP');
    assert.equal(eraInitials('My Beautiful Dark Twisted Fantasy'), 'MB');
    assert.equal(eraInitials('Before The College Dropout'), 'BC');
    assert.equal(eraInitials('Yeezus'), 'Y');
    assert.equal(eraInitials('ye'), 'Y');
  });

  test('keeps numbers whole and skips bracketed parts', () => {
    assert.equal(eraInitials('DONDA 2 [V1]'), 'D2');
    assert.equal(eraInitials('808s & Heartbreak'), '8H');
    assert.equal(eraInitials('Watch The Throne\n(Collaboration with JAŸ-Z as The Throne)'), 'WT');
    assert.equal(eraInitials('Vultures 10'), 'V10');
  });

  test('falls back to minor words, and to empty when there are no letters', () => {
    assert.equal(eraInitials('The The'), 'TT');
    assert.equal(eraInitials('Ÿ É'), 'ŸÉ');
    assert.equal(eraInitials('⭐ 🗑️'), '');
    assert.equal(eraInitials(''), '');
  });
});
