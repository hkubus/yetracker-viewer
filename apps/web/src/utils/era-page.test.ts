import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
  eraPageHref,
  eraPageTitle,
  eraSongsApiPath,
  maxReachablePage,
  pageCountFor,
  parseId,
  parsePageParam,
  parseTotalCount,
  summarize,
} from './era-page.ts';

describe('parseId', () => {
  test('accepts canonical positive integers only', () => {
    assert.equal(parseId('31'), 31);
    assert.equal(parseId('9007199254740991'), Number.MAX_SAFE_INTEGER);
    for (const value of ['abc', '0', '01', '-1', '1.5', '1e3', ' 31', '', '9007199254740992', undefined, null]) {
      assert.equal(parseId(value), null, String(value));
    }
  });
});

describe('parsePageParam', () => {
  test('defaults to page 1 and rejects malformed values', () => {
    assert.equal(parsePageParam(null), 1);
    assert.equal(parsePageParam(''), 1);
    assert.equal(parsePageParam('3'), 3);
    assert.equal(parsePageParam('99999999999999999999'), Number.MAX_SAFE_INTEGER);
    for (const value of ['0', '-2', '02', 'abc', '1.5', '2x']) assert.equal(parsePageParam(value), null, value);
  });

  test('page math', () => {
    assert.equal(pageCountFor(0, 100), 1);
    assert.equal(pageCountFor(100, 100), 1);
    assert.equal(pageCountFor(956, 100), 10);
    assert.equal(maxReachablePage(100), 101);
    assert.equal(parseTotalCount('956'), 956);
    assert.equal(parseTotalCount(null), null);
    assert.equal(parseTotalCount('-1'), null);
  });
});

describe('era URLs', () => {
  test('links keep a stable parameter order and drop page 1', () => {
    assert.equal(eraPageHref(31), '/eras/31');
    assert.equal(eraPageHref(31, {}, 1), '/eras/31');
    assert.equal(
      eraPageHref(31, { q: 'can’t stop', category: 'best-of', sort: 'name' }, 3),
      '/eras/31?q=can%E2%80%99t+stop&category=best-of&sort=name&page=3',
    );
    assert.equal(eraPageHref(31, { q: '', category: '', sort: '' }, 2), '/eras/31?page=2');
  });

  test('API paths carry limit and offset', () => {
    assert.equal(eraSongsApiPath(31, { q: 'a&b' }, 200, 100), '/eras/31/songs?q=a%26b&limit=100&offset=200');
  });
});

describe('text', () => {
  test('eraPageTitle mentions the query, category and page', () => {
    assert.equal(eraPageTitle('DONDA 2 [V1]'), 'DONDA 2 [V1] | Ye Tracker');
    assert.equal(
      eraPageTitle('DONDA 2 [V1]', { query: 'nebraska', categoryLabel: 'Best of', page: 2 }),
      'DONDA 2 [V1] – Search “nebraska” – Best of – Page 2 | Ye Tracker',
    );
  });

  test('summarize flattens and cuts at a word boundary', () => {
    assert.equal(summarize('line 1\nline 2'), 'line 1 line 2');
    const long = 'word '.repeat(60);
    const summary = summarize(long, 40);
    assert.ok(Array.from(summary).length <= 40);
    assert.ok(summary.endsWith('word…'));
    assert.equal(summarize('a'.repeat(50), 10), `${'a'.repeat(9)}…`);
  });
});
