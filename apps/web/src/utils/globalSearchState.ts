/**
 * State of the home page's song search: the query, an era range and the "playable only" switch. It lives in the
 * page URL (`/?q=kanye&eraFrom=12&eraTo=20&playable=true`) so a search survives reloads, back/forward and sharing.
 * Era bounds are era ids in the URL and the API, and indexes into the era list (catalog order) in the controls.
 */
import { categoryMarkersIn } from '../songCategories.ts';
import { clampQuery, fold, tokens } from './search.ts';

export interface GlobalSearchState {
  /** The query as typed (`clampQuery`: control characters dropped, whitespace collapsed, cut to the API's limit). */
  q: string;
  /** Index of the first era in range; 0 = from the first era. */
  from: number;
  /** Index of the last era in range; `eraCount - 1` = up to the last era. */
  to: number;
  playable: boolean;
}

/**
 * - `idle`: nothing to search for.
 * - `no-terms`: text was typed but it has no letters, digits or category markers (e.g. only punctuation), and no
 *   filter is set, so there is nothing meaningful to send.
 * - `search`: send a request (text, filters, or both).
 */
export type GlobalSearchMode = 'idle' | 'no-terms' | 'search';

/**
 * Whether a query has something the API searches for: words and numbers (its folded tokens), or category markers
 * (⭐ ✨ 🏆 🏅 🗑️ 🤖, anywhere in the text), which the API applies like its `category` filter. Anything else (`???`)
 * is a blank query to the API.
 */
export function hasSearchTerms(query: string): boolean {
  return tokens(query).length > 0 || categoryMarkersIn(query).length > 0;
}

const TRUE_VALUES = new Set(['true', '1', 'yes', 'on']);

function lastIndex(eraCount: number): number {
  return Math.max(eraCount - 1, 0);
}

function indexOfEra(value: string | null, eraIds: readonly number[]): number | null {
  if (value === null || !/^[1-9]\d*$/.test(value)) return null;
  const index = eraIds.indexOf(Number(value));
  return index === -1 ? null : index;
}

/** Everything off: no text, all eras, playable or not. */
export function defaultSearchState(eraCount: number): GlobalSearchState {
  return { q: '', from: 0, to: lastIndex(eraCount), playable: false };
}

/** Reads the state from URL parameters. Unknown era ids are ignored; a reversed range is put in order. */
export function parseSearchState(params: URLSearchParams, eraIds: readonly number[]): GlobalSearchState {
  let from = indexOfEra(params.get('eraFrom'), eraIds) ?? 0;
  let to = indexOfEra(params.get('eraTo'), eraIds) ?? lastIndex(eraIds.length);
  if (from > to) [from, to] = [to, from];
  return {
    q: clampQuery(params.get('q') ?? ''),
    from,
    to,
    playable: TRUE_VALUES.has((params.get('playable') ?? '').toLowerCase()),
  };
}

/** Whether the era range is narrower than "all eras" or "playable only" is on. */
export function hasActiveFilters(state: GlobalSearchState, eraCount: number): boolean {
  return state.playable || state.from > 0 || state.to < lastIndex(eraCount);
}

export function searchMode(state: GlobalSearchState, eraCount: number): GlobalSearchMode {
  if (hasSearchTerms(state.q) || hasActiveFilters(state, eraCount)) return 'search';
  return state.q === '' ? 'idle' : 'no-terms';
}

function setEraBounds(params: URLSearchParams, state: GlobalSearchState, eraIds: readonly number[]): void {
  const fromId = state.from > 0 ? eraIds[state.from] : undefined;
  const toId = state.to < lastIndex(eraIds.length) ? eraIds[state.to] : undefined;
  if (fromId !== undefined) params.set('eraFrom', String(fromId));
  if (toId !== undefined) params.set('eraTo', String(toId));
}

/** The page URL's query parameters for a state (defaults are left out, so the idle page is plain `/`). */
export function pageSearchParams(state: GlobalSearchState, eraIds: readonly number[]): URLSearchParams {
  const params = new URLSearchParams();
  if (state.q) params.set('q', state.q);
  setEraBounds(params, state, eraIds);
  if (state.playable) params.set('playable', 'true');
  return params;
}

/** Parameters for `GET /songs` (search/filter mode). A query without search terms is not sent. */
export function apiSearchParams(
  state: GlobalSearchState,
  eraIds: readonly number[],
  page: { offset: number; limit: number },
): URLSearchParams {
  const params = new URLSearchParams();
  if (hasSearchTerms(state.q)) params.set('q', state.q);
  setEraBounds(params, state, eraIds);
  if (state.playable) params.set('playable', 'true');
  if (page.offset > 0) params.set('offset', String(page.offset));
  params.set('limit', String(page.limit));
  return params;
}

/**
 * Identity of the result set a state asks for: queries that fold to the same text ("Can't" / "can’t", "Beyoncé" /
 * "beyonce") and carry the same category markers match the same songs, so retyping one as the other doesn't search
 * again. Folding drops the markers, so they are part of the key on their own ("⭐ glory" is not "glory").
 */
export function searchKey(state: GlobalSearchState, eraIds: readonly number[]): string {
  const params = new URLSearchParams();
  params.set('q', fold(state.q));
  const markers = categoryMarkersIn(state.q);
  if (markers.length > 0) params.set('markers', markers.join(''));
  setEraBounds(params, state, eraIds);
  if (state.playable) params.set('playable', 'true');
  return params.toString();
}
