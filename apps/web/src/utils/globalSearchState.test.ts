import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
  apiSearchParams,
  defaultSearchState,
  hasActiveFilters,
  hasSearchTerms,
  pageSearchParams,
  parseSearchState,
  searchKey,
  searchMode,
} from './globalSearchState.ts';

const eraIds = [1, 2, 3, 5, 8];
const params = (query: string) => new URLSearchParams(query);

describe('parseSearchState', () => {
  test('reads the query, era bounds and playable switch', () => {
    assert.deepEqual(parseSearchState(params('q=%20kid%20%20cudi%20&eraFrom=2&eraTo=5&playable=true'), eraIds), {
      q: 'kid cudi',
      from: 1,
      to: 3,
      playable: true,
    });
  });

  test('defaults and ignores invalid or unknown values', () => {
    assert.deepEqual(parseSearchState(params(''), eraIds), defaultSearchState(eraIds.length));
    assert.deepEqual(parseSearchState(params('eraFrom=4&eraTo=08&playable=maybe'), eraIds), {
      q: '',
      from: 0,
      to: 4,
      playable: false,
    });
    assert.deepEqual(parseSearchState(params('eraFrom=8&eraTo=2'), eraIds), { q: '', from: 1, to: 4, playable: false });
    assert.deepEqual(parseSearchState(params('eraFrom=3'), []), { q: '', from: 0, to: 0, playable: false });
  });

  test('clamps long queries to 100 code points', () => {
    const state = parseSearchState(params(`q=${encodeURIComponent('é'.repeat(150))}`), eraIds);
    assert.equal(Array.from(state.q).length, 100);
  });

  test('drops control characters from the query', () => {
    const state = parseSearchState(params('q=%00abc%1B'), eraIds);
    assert.equal(state.q, 'abc');
    assert.equal(pageSearchParams(state, eraIds).toString(), 'q=abc');
    const blank = parseSearchState(params('q=%00%00'), eraIds);
    assert.equal(blank.q, '');
    assert.equal(searchMode(blank, eraIds.length), 'idle');
    assert.equal(pageSearchParams(blank, eraIds).toString(), '');
  });
});

describe('searchMode', () => {
  const idle = defaultSearchState(eraIds.length);
  test('distinguishes idle, text without terms, and searches', () => {
    assert.equal(searchMode(idle, eraIds.length), 'idle');
    assert.equal(searchMode({ ...idle, q: '!!! ★' }, eraIds.length), 'no-terms');
    assert.equal(searchMode({ ...idle, q: '!!! 🏆' }, eraIds.length), 'search', 'a category marker is a search term');
    assert.equal(searchMode({ ...idle, q: 'can’t' }, eraIds.length), 'search');
    assert.equal(searchMode({ ...idle, playable: true }, eraIds.length), 'search');
    assert.equal(searchMode({ ...idle, from: 1 }, eraIds.length), 'search');
    assert.equal(searchMode({ ...idle, q: '???', to: 3 }, eraIds.length), 'search');
    assert.equal(hasActiveFilters({ ...idle, to: 3 }, eraIds.length), true);
    assert.equal(hasActiveFilters(idle, eraIds.length), false);
  });
});

describe('URL and API parameters', () => {
  test('page parameters leave defaults out', () => {
    assert.equal(pageSearchParams(defaultSearchState(eraIds.length), eraIds).toString(), '');
    assert.equal(
      pageSearchParams({ q: 'Beyoncé', from: 1, to: 4, playable: true }, eraIds).toString(),
      'q=Beyonc%C3%A9&eraFrom=2&playable=true',
    );
    assert.equal(pageSearchParams({ q: '', from: 0, to: 2, playable: false }, eraIds).toString(), 'eraTo=3');
  });

  test('API parameters support filter-only searches and paging', () => {
    assert.equal(
      apiSearchParams({ q: '', from: 0, to: 4, playable: true }, eraIds, { offset: 0, limit: 50 }).toString(),
      'playable=true&limit=50',
    );
    assert.equal(
      apiSearchParams({ q: 'kid cudi', from: 2, to: 3, playable: false }, eraIds, {
        offset: 100,
        limit: 50,
      }).toString(),
      'q=kid+cudi&eraFrom=3&eraTo=5&offset=100&limit=50',
    );
    assert.equal(
      apiSearchParams({ q: '?!', from: 0, to: 3, playable: false }, eraIds, { offset: 0, limit: 50 }).toString(),
      'eraTo=5&limit=50',
    );
  });

  test('category markers count as search terms and are sent as typed', () => {
    for (const query of ['⭐', '⭐️', 'x 🗑️', '🗑', '🤖🏅']) assert.equal(hasSearchTerms(query), true, query);
    for (const query of ['', '???', '★ ☆', '\u200b']) assert.equal(hasSearchTerms(query), false, query);
    assert.equal(
      apiSearchParams({ q: '⭐️', from: 0, to: 4, playable: false }, eraIds, { offset: 0, limit: 50 }).toString(),
      `q=${encodeURIComponent('⭐️')}&limit=50`,
    );
  });

  test('search keys keep the category markers', () => {
    const key = (q: string) => searchKey({ q, from: 0, to: 4, playable: false }, eraIds);
    assert.notEqual(key('⭐ glory'), key('glory'));
    assert.equal(key('⭐ glory'), key('Glory ⭐️'), 'anywhere in the query, variation selector or not');
    assert.equal(key('🗑️🤖'), key('🤖 🗑'));
    assert.notEqual(key('🗑️'), key('🤖'));
  });

  test('search keys fold the query', () => {
    const a = searchKey({ q: 'Can’t', from: 0, to: 4, playable: false }, eraIds);
    const b = searchKey({ q: "can't", from: 0, to: 4, playable: false }, eraIds);
    const c = searchKey({ q: "can't", from: 0, to: 4, playable: true }, eraIds);
    assert.equal(a, b);
    assert.notEqual(a, c);
  });
});
