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
  parseTimeline,
  parseTotalCount,
  summarize,
  timelineYears,
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

describe('timeline', () => {
  test('pairs each date with its event, at the precision the catalog gives', () => {
    const entries = parseTimeline(
      '(05/15/2018) (Kanye restarts the project) (12/??/2016) (Ye and Cudi have their first sessions) ' +
        '(??/??/2019) (Kanye and Chance scrap the project) (Late 2013) (The project is scrapped)',
    );
    assert.deepEqual(entries, [
      { date: '5/15/2018', iso: '2018-05-15', text: 'Kanye restarts the project' },
      { date: 'Dec 2016', iso: '2016-12', text: 'Ye and Cudi have their first sessions' },
      { date: '2019', iso: '2019', text: 'Kanye and Chance scrap the project' },
      { date: 'Late 2013', iso: '2013', text: 'The project is scrapped' },
    ]);
  });

  test('keeps nested parentheses and undated entries', () => {
    assert.deepEqual(parseTimeline('(08/29/2021) (Donda (Deluxe) officially releases)\n(Ongoing)'), [
      { date: '8/29/2021', iso: '2021-08-29', text: 'Donda (Deluxe) officially releases' },
      { date: null, text: 'Ongoing' },
    ]);
  });

  test('anything else is not a timeline', () => {
    for (const notes of ['', 'Plain text', '(05/15/2018) trailing', '(05/15/2018)', '(1/1/2020) (a', 'a) (b', '()']) {
      assert.equal(parseTimeline(notes), null, notes);
    }
    assert.equal(parseTimeline('(05/15/2018) (06/01/2018)'), null);
  });

  test('timelineYears spans the first and last dated entries', () => {
    assert.equal(timelineYears(parseTimeline('(11/30/2016) (a) (05/15/2018) (b)')), '2016–2018');
    assert.equal(timelineYears(parseTimeline('(05/15/2018) (a) (11/07/2018) (b)')), '2018');
    assert.equal(timelineYears(parseTimeline('(Ongoing)')), null);
    assert.equal(timelineYears(null), null);
  });
});
