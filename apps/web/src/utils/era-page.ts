/**
 * URL and text helpers for the era pages (`/eras/:id?page=&q=&category=&sort=`). Pure functions, safe anywhere.
 */

/** Largest `offset` the API accepts on list endpoints. */
export const MAX_LIST_OFFSET = 10_000;

const ID_PATTERN = /^[1-9]\d*$/;

/**
 * Parses an id the way the API validates path ids: digits without leading zeros, within the safe integer range.
 * Anything else (`abc`, `0`, `01`, `-1`, `1.5`) is `null`.
 */
export function parseId(value: string | null | undefined): number | null {
  if (!value || !ID_PATTERN.test(value)) return null;
  const id = Number(value);
  return Number.isSafeInteger(id) ? id : null;
}

/**
 * Parses `?page=`: missing or blank means page 1; a positive integer is that page (huge values become
 * `Number.MAX_SAFE_INTEGER`, i.e. "past the end"); anything else is `null` (a malformed link).
 */
export function parsePageParam(value: string | null | undefined): number | null {
  const trimmed = value?.trim() ?? '';
  if (trimmed === '') return 1;
  if (!ID_PATTERN.test(trimmed)) return null;
  const page = Number(trimmed);
  return Number.isSafeInteger(page) ? page : Number.MAX_SAFE_INTEGER;
}

/** Number of pages needed for `total` items (at least 1, so an empty list still has its first page). */
export function pageCountFor(total: number, pageSize: number): number {
  return Math.max(1, Math.ceil(Math.max(0, total) / pageSize));
}

/** The last page the API can serve (its offset limit). */
export function maxReachablePage(pageSize: number): number {
  return Math.floor(MAX_LIST_OFFSET / pageSize) + 1;
}

/** `X-Total-Count` as a non-negative integer, or `null` when missing or malformed. */
export function parseTotalCount(value: string | null | undefined): number | null {
  if (value == null || !/^\d+$/.test(value.trim())) return null;
  const total = Number(value.trim());
  return Number.isSafeInteger(total) ? total : null;
}

export interface EraListParams {
  /** Search query (already normalized); empty means none. */
  q?: string;
  /** Category filter id; empty means none. */
  category?: string;
  /** Non-default sort id; empty means the default order (never sent explicitly). */
  sort?: string;
}

/** Query parameters of an era listing, in a stable order; empty values are left out. */
export function eraListSearchParams(params: EraListParams): URLSearchParams {
  const search = new URLSearchParams();
  if (params.q) search.set('q', params.q);
  if (params.category) search.set('category', params.category);
  if (params.sort) search.set('sort', params.sort);
  return search;
}

/** Link to a page of an era: `/eras/31?q=nebraska&page=2`. Page 1 has no `page` parameter. */
export function eraPageHref(eraId: number, params: EraListParams = {}, page = 1): string {
  const search = eraListSearchParams(params);
  if (page > 1) search.set('page', String(page));
  const query = search.toString();
  return `/eras/${eraId}${query ? `?${query}` : ''}`;
}

/** API path of one page of an era's songs. */
export function eraSongsApiPath(eraId: number, params: EraListParams, offset: number, limit: number): string {
  const search = eraListSearchParams(params);
  search.set('limit', String(limit));
  search.set('offset', String(offset));
  return `/eras/${eraId}/songs?${search.toString()}`;
}

/** Document title of an era page: `DONDA 2 [V1] – Search “nebraska” – Page 2 | Ye Tracker`. */
export function eraPageTitle(
  eraName: string,
  { query, categoryLabel, page = 1 }: { query?: string; categoryLabel?: string; page?: number } = {},
): string {
  const parts = [eraName];
  if (query) parts.push(`Search “${query}”`);
  if (categoryLabel) parts.push(categoryLabel);
  if (page > 1) parts.push(`Page ${page}`);
  return `${parts.join(' – ')} | Ye Tracker`;
}

/**
 * Single-line summary for meta descriptions: whitespace (line breaks included) collapsed, and cut at a word
 * boundary with an ellipsis when longer than `maxLength` characters.
 */
export function summarize(text: string, maxLength = 160): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  const chars = Array.from(flat);
  if (chars.length <= maxLength) return flat;
  const cut = chars.slice(0, maxLength - 1).join('');
  const lastSpace = cut.lastIndexOf(' ');
  const base = lastSpace > maxLength * 0.6 ? cut.slice(0, lastSpace) : cut;
  return `${base.replace(/[\s,.;:–—-]+$/u, '')}…`;
}

/** One dated event of an era's timeline (`era.notes`). */
export interface TimelineEntry {
  /** As shown: `5/15/2018`, `Dec 2016`, `2019`, `Late 2013`; null for an undated entry such as `(Ongoing)`. */
  date: string | null;
  /** `<time datetime>` value at the precision the catalog gives (`2018-05-15`, `2016-12`, `2019`). */
  iso?: string;
  text: string;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const NUMERIC_DATE = /^(\d{1,2}|\?\?)\/(\d{1,2}|\?\?)\/(\d{4})$/;
const SEASON_DATE = /^(?:(?:Early|Mid|Late)\s+)?(\d{4})$/i;

/** A timeline date (`05/15/2018`, `12/??/2016`, `??/??/2019`, `Late 2013`) in display form, or null. */
function timelineDate(value: string): Pick<TimelineEntry, 'date' | 'iso'> | null {
  const numeric = NUMERIC_DATE.exec(value);
  if (numeric) {
    const [, monthText = '', dayText = '', year = ''] = numeric;
    const month = Number(monthText);
    const day = Number(dayText);
    if (monthText === '??' || month < 1 || month > 12) return { date: year, iso: year };
    const iso = `${year}-${String(month).padStart(2, '0')}`;
    if (dayText === '??' || day < 1 || day > 31) return { date: `${MONTHS[month - 1]} ${year}`, iso };
    return { date: `${month}/${day}/${year}`, iso: `${iso}-${String(day).padStart(2, '0')}` };
  }
  const season = SEASON_DATE.exec(value);
  return season ? { date: value.replace(/\s+/g, ' '), iso: season[1] } : null;
}

/**
 * Reads an era's timeline notes — `(05/15/2018) (Kanye restarts the project) (06/01/2018) (ye officially releases)`
 * — as dated entries. Events may contain parentheses of their own (`(Donda (Deluxe) officially releases)`). Null
 * when the notes are anything else (they are then shown as written).
 */
export function parseTimeline(notes: string): TimelineEntry[] | null {
  const groups: string[] = [];
  let depth = 0;
  let start = 0;
  for (let index = 0; index < notes.length; index += 1) {
    const char = notes[index] ?? '';
    if (char === '(') {
      if (depth === 0) start = index + 1;
      depth += 1;
    } else if (char === ')') {
      if (depth === 0) return null;
      depth -= 1;
      if (depth === 0) groups.push(notes.slice(start, index).replace(/\s+/g, ' ').trim());
    } else if (depth === 0 && !/\s/.test(char)) {
      return null;
    }
  }
  if (depth !== 0 || groups.length === 0 || groups.includes('')) return null;

  const entries: TimelineEntry[] = [];
  for (let index = 0; index < groups.length; index += 1) {
    const group = groups[index] ?? '';
    const date = timelineDate(group);
    if (!date) {
      entries.push({ date: null, text: group });
      continue;
    }
    // A date is always followed by its event.
    const next = groups[index + 1];
    if (next === undefined || timelineDate(next)) return null;
    entries.push({ ...date, text: next });
    index += 1;
  }
  return entries;
}

/** The years a timeline spans (`2016–2018`, or `2018`), from its first and last dated entries. */
export function timelineYears(entries: readonly TimelineEntry[] | null): string | null {
  const years = (entries ?? []).flatMap((entry) => (entry.iso ? [entry.iso.slice(0, 4)] : []));
  const first = years[0];
  const last = years.at(-1);
  if (!first || !last) return null;
  return first === last ? first : `${first}–${last}`;
}
