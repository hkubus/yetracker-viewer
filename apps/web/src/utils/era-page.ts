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
