/**
 * Text normalization shared by every search in the site. The API folds its search index with the same
 * algorithm, so a query folded here matches exactly what the server matches.
 */

/** Longest query the API accepts, in Unicode code points (after trimming and collapsing whitespace). */
export const MAX_QUERY_LENGTH = 100;

const APOSTROPHES = /['’‘ʼ`´]/g;
const COMBINING_MARKS = /\p{M}/gu;
// Letters that have no Unicode decomposition to an ASCII base letter.
const UNDECOMPOSABLE_LETTERS = /[ØøÆæŒœßŁłĐđÞþı]/g;
const LETTER_REPLACEMENTS: Readonly<Record<string, string>> = {
  Ø: 'o',
  ø: 'o',
  Æ: 'ae',
  æ: 'ae',
  Œ: 'oe',
  œ: 'oe',
  ß: 'ss',
  Ł: 'l',
  ł: 'l',
  Đ: 'd',
  đ: 'd',
  Þ: 'th',
  þ: 'th',
  ı: 'i',
};
// Zero-width characters, the word joiner, the BOM and emoji/text variation selectors. An alternation rather than
// a character class: the joiner and the selectors combine with their neighbours.
const INVISIBLES = /​|‌|‍|⁠|﻿|︎|️/g;
const SEPARATORS = /[^\p{L}\p{N}]+/gu;
const WHITESPACE = /\s+/g;
// C0 and C1 control characters (NUL, ESC, DEL, NEL, …) except the ones that are whitespace (tab, line breaks).
const CONTROL_CHARACTERS = /(?!\s)\p{Cc}/gu;

/**
 * Folds text for accent-, case- and punctuation-insensitive matching:
 * `fold('⭐️ NEBRASKA [V4] (feat. JAŸ-Z)') === 'nebraska v4 feat jay z'`, `fold('can’t') === 'cant'`.
 *
 * Steps: strip apostrophes; NFKD and drop combining marks; spell out letters without a decomposition (Ø→o, Æ→ae,
 * Œ→oe, ß→ss, Ł→l, Đ→d, Þ→th, ı→i); lowercase (locale-independent); strip apostrophes again (NFKD turns
 * fullwidth ones into ASCII); drop zero-width characters and variation selectors; turn every run of characters
 * that are not letters or digits into one space; trim.
 * Apostrophes are stripped before NFKD too because NFKD turns `´` into a space plus a combining accent.
 */
export function fold(text: string): string {
  return text
    .replace(APOSTROPHES, '')
    .normalize('NFKD')
    .replace(COMBINING_MARKS, '')
    .replace(UNDECOMPOSABLE_LETTERS, (letter) => LETTER_REPLACEMENTS[letter] ?? letter)
    .toLowerCase()
    .replace(APOSTROPHES, '')
    .replace(INVISIBLES, '')
    .replace(SEPARATORS, ' ')
    .trim();
}

/** Search tokens of a query: its folded words. A blank query has no tokens. */
export function tokens(query: string): string[] {
  const folded = fold(query);
  return folded ? folded.split(' ') : [];
}

/**
 * Whether every token occurs in the (already folded) haystack as a substring, so prefixes match while typing.
 * No tokens means no constraint: everything matches.
 */
export function matchesAllTokens(foldedHaystack: string, queryTokens: readonly string[]): boolean {
  return queryTokens.every((token) => foldedHaystack.includes(token));
}

/**
 * Drops control characters (a URL can carry `%00`; they mean nothing in a search and must not reach the page title,
 * the search box or the API), then trims and collapses whitespace the way the API does before validating `q`.
 * Tabs and line breaks count as whitespace: `'a\tb'` → `'a b'`, `'\0abc'` → `'abc'`.
 */
export function normalizeQuery(query: string): string {
  return query.replace(CONTROL_CHARACTERS, '').trim().replace(WHITESPACE, ' ');
}

/** `normalizeQuery`, then cut to the API's limit (counted in code points, never splitting a surrogate pair). */
export function clampQuery(query: string, maxLength: number = MAX_QUERY_LENGTH): string {
  const normalized = normalizeQuery(query);
  const codePoints = Array.from(normalized);
  return codePoints.length > maxLength ? codePoints.slice(0, maxLength).join('').trimEnd() : normalized;
}
