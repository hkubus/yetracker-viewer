import type { DatePrecision } from '@yetracker/types';

/** Shown where a catalog date is missing. */
export const MISSING_DATE = '—';

/**
 * - `short`: `11/5/2017`, `Nov 2017`, `2015` — compact table cells.
 * - `medium` (default): `Nov 5, 2017`, `Nov 2017`, `2015`.
 * - `long`: `November 5, 2017`, `November 2017`, `2015`.
 */
export type CatalogDateStyle = 'short' | 'medium' | 'long';

export interface CatalogDate {
  date: Date;
  precision: DatePrecision;
}

const PRECISIONS: readonly string[] = ['day', 'month', 'year'] satisfies DatePrecision[];

/**
 * Normalizes an API date: Unix seconds at UTC midnight of the first day of the period plus its precision.
 * Returns null when there is no usable date. Payloads without a precision field are read as day precision,
 * with `0` meaning "no date" (older API versions).
 */
export function parseCatalogDate(
  unixSeconds: number | null | undefined,
  precision?: DatePrecision | null,
): CatalogDate | null {
  if (typeof unixSeconds !== 'number' || !Number.isFinite(unixSeconds)) return null;
  const known = typeof precision === 'string' && PRECISIONS.includes(precision);
  if (!known && unixSeconds <= 0) return null;
  const date = new Date(unixSeconds * 1000);
  if (Number.isNaN(date.getTime())) return null;
  return { date, precision: known ? precision : 'day' };
}

const MONTH_NAME: Record<CatalogDateStyle, 'short' | 'long'> = { short: 'short', medium: 'short', long: 'long' };
const formatters = new Map<string, Intl.DateTimeFormat>();

function formatter(style: CatalogDateStyle, precision: DatePrecision): Intl.DateTimeFormat {
  const key = `${style}:${precision}`;
  let cached = formatters.get(key);
  if (!cached) {
    const options: Intl.DateTimeFormatOptions = { timeZone: 'UTC', year: 'numeric' };
    if (precision === 'day' && style === 'short') {
      options.month = 'numeric';
      options.day = 'numeric';
    } else if (precision !== 'year') {
      options.month = MONTH_NAME[style];
      if (precision === 'day') options.day = 'numeric';
    }
    cached = new Intl.DateTimeFormat('en-US', options);
    formatters.set(key, cached);
  }
  return cached;
}

/**
 * Formats a catalog date at its precision, always in UTC with a 4-digit year: `Nov 5, 2017`, `Nov 2017`, `2015`
 * (see `CatalogDateStyle`). Missing dates render as `—`.
 */
export function formatCatalogDate(
  unixSeconds: number | null | undefined,
  precision?: DatePrecision | null,
  style: CatalogDateStyle = 'medium',
): string {
  const parsed = parseCatalogDate(unixSeconds, precision);
  return parsed ? formatter(style, parsed.precision).format(parsed.date) : MISSING_DATE;
}

/**
 * Machine-readable value for `<time datetime>` at the same precision as the displayed text: `2017-11-05`,
 * `2017-11`, `2015`. `undefined` when the date is missing, so the attribute is omitted.
 */
export function isoDate(unixSeconds: number | null | undefined, precision?: DatePrecision | null): string | undefined {
  const parsed = parseCatalogDate(unixSeconds, precision);
  if (!parsed) return undefined;
  const iso = parsed.date.toISOString();
  const year = parsed.date.getUTCFullYear();
  // toISOString() writes years outside 0000–9999 with a sign and 6 digits; catalog dates never get there.
  if (year < 0 || year > 9999) return undefined;
  if (parsed.precision === 'year') return iso.slice(0, 4);
  if (parsed.precision === 'month') return iso.slice(0, 7);
  return iso.slice(0, 10);
}

const relativeTime = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });
const RELATIVE_STEPS: ReadonlyArray<[limitSeconds: number, unit: Intl.RelativeTimeFormatUnit, unitSeconds: number]> = [
  [3600, 'minute', 60],
  [86_400, 'hour', 3600],
  [7 * 86_400, 'day', 86_400],
  [30 * 86_400, 'week', 7 * 86_400],
  [365 * 86_400, 'month', 30 * 86_400],
  [Number.POSITIVE_INFINITY, 'year', 365 * 86_400],
];

/**
 * Relative description of a Unix timestamp such as `5 minutes ago`, `yesterday`, `3 weeks ago`; `just now` within a
 * minute (also for small clock skew). `nowMs` is injectable for tests and server/client consistency.
 */
export function formatRelativeTime(unixSeconds: number, nowMs: number = Date.now()): string {
  const deltaSeconds = unixSeconds - nowMs / 1000;
  const distance = Math.abs(deltaSeconds);
  if (!Number.isFinite(distance)) return MISSING_DATE;
  if (distance < 60) return 'just now';
  for (const [limit, unit, unitSeconds] of RELATIVE_STEPS) {
    if (distance < limit) return relativeTime.format(Math.trunc(deltaSeconds / unitSeconds), unit);
  }
  return MISSING_DATE;
}
