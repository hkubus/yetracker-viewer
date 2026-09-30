import assert from 'node:assert/strict';
import { describe, test } from 'node:test';

// A zone west of UTC: formatting in local time would move UTC-midnight dates to the previous day/month/year.
process.env.TZ = 'America/Los_Angeles';

const { formatCatalogDate, formatRelativeTime, isoDate, MISSING_DATE, parseCatalogDate } = await import('./dates.ts');

const NOV_5_2017 = Date.UTC(2017, 10, 5) / 1000;
const NOV_2017 = Date.UTC(2017, 10, 1) / 1000;
const YEAR_2015 = Date.UTC(2015, 0, 1) / 1000;

describe('formatCatalogDate', () => {
  test('uses UTC regardless of the local time zone', () => {
    assert.equal(new Date(NOV_2017 * 1000).getMonth(), 9, 'the test zone must be behind UTC');
    assert.equal(formatCatalogDate(NOV_2017, 'month'), 'Nov 2017');
    assert.equal(formatCatalogDate(YEAR_2015, 'year'), '2015');
  });

  test('formats each precision in each style with 4-digit years', () => {
    assert.equal(formatCatalogDate(NOV_5_2017, 'day'), 'Nov 5, 2017');
    assert.equal(formatCatalogDate(NOV_5_2017, 'day', 'short'), '11/5/2017');
    assert.equal(formatCatalogDate(NOV_5_2017, 'day', 'long'), 'November 5, 2017');
    assert.equal(formatCatalogDate(NOV_2017, 'month', 'short'), 'Nov 2017');
    assert.equal(formatCatalogDate(NOV_2017, 'month', 'long'), 'November 2017');
    assert.equal(formatCatalogDate(YEAR_2015, 'year', 'short'), '2015');
    assert.equal(formatCatalogDate(YEAR_2015, 'year', 'long'), '2015');
    assert.equal(formatCatalogDate(Date.UTC(1999, 0, 2) / 1000, 'day', 'short'), '1/2/1999');
  });

  test('renders missing dates as a dash', () => {
    assert.equal(MISSING_DATE, '—');
    assert.equal(formatCatalogDate(null, null), '—');
    assert.equal(formatCatalogDate(undefined), '—');
    assert.equal(formatCatalogDate(Number.NaN, 'day'), '—');
    assert.equal(formatCatalogDate(Number.POSITIVE_INFINITY, 'day'), '—');
  });

  test('reads payloads without a precision as day precision where 0 means no date', () => {
    assert.equal(formatCatalogDate(NOV_5_2017), 'Nov 5, 2017');
    assert.equal(formatCatalogDate(0), '—');
    assert.equal(formatCatalogDate(0, null), '—');
    assert.equal(formatCatalogDate(-86_400), '—');
  });

  test('trusts the value when a precision is given, including dates before 1970', () => {
    assert.equal(formatCatalogDate(Date.UTC(1965, 0, 1) / 1000, 'year'), '1965');
    assert.equal(formatCatalogDate(0, 'day'), 'Jan 1, 1970');
  });
});

describe('isoDate', () => {
  test('matches the displayed precision', () => {
    assert.equal(isoDate(NOV_5_2017, 'day'), '2017-11-05');
    assert.equal(isoDate(NOV_2017, 'month'), '2017-11');
    assert.equal(isoDate(YEAR_2015, 'year'), '2015');
    assert.equal(isoDate(NOV_5_2017), '2017-11-05');
  });

  test('is undefined for missing dates so the attribute is omitted', () => {
    assert.equal(isoDate(null, null), undefined);
    assert.equal(isoDate(0), undefined);
    assert.equal(isoDate(Number.NaN, 'day'), undefined);
  });
});

describe('parseCatalogDate', () => {
  test('returns the date and a normalized precision', () => {
    assert.deepEqual(parseCatalogDate(NOV_2017, 'month'), { date: new Date(NOV_2017 * 1000), precision: 'month' });
    assert.equal(parseCatalogDate(NOV_2017, null)?.precision, 'day');
    assert.equal(parseCatalogDate(null, 'day'), null);
  });
});

describe('formatRelativeTime', () => {
  const now = Date.UTC(2026, 8, 30, 12, 0, 0);
  const ago = (seconds: number) => now / 1000 - seconds;

  test('describes the distance in the largest fitting unit', () => {
    assert.equal(formatRelativeTime(ago(10), now), 'just now');
    assert.equal(formatRelativeTime(ago(-20), now), 'just now');
    assert.equal(formatRelativeTime(ago(5 * 60), now), '5 minutes ago');
    assert.equal(formatRelativeTime(ago(90 * 60), now), '1 hour ago');
    assert.equal(formatRelativeTime(ago(26 * 3600), now), 'yesterday');
    assert.equal(formatRelativeTime(ago(3 * 86_400), now), '3 days ago');
    assert.equal(formatRelativeTime(ago(15 * 86_400), now), '2 weeks ago');
    assert.equal(formatRelativeTime(ago(95 * 86_400), now), '3 months ago');
    assert.equal(formatRelativeTime(ago(800 * 86_400), now), '2 years ago');
    assert.equal(formatRelativeTime(ago(-2 * 3600), now), 'in 2 hours');
  });

  test('handles non-finite input', () => {
    assert.equal(formatRelativeTime(Number.NaN, now), '—');
  });
});
