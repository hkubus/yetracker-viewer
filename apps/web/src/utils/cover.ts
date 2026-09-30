/** Joins an API base URL (absolute, or a same-origin prefix such as `/api`) and an absolute path. */
function joinApiPath(apiBaseUrl: string, path: string): string {
  return `${apiBaseUrl.replace(/\/+$/, '')}${path}`;
}

/**
 * URL of an era's cover: `/eras/:id/cover?v=<coverVersion>`. The version makes the URL immutable-cacheable, so
 * pass it whenever it is known. `format: 'jpeg'` asks for the JPEG variant (for og:image; 404 when there is none).
 */
export function coverUrl(
  apiBaseUrl: string,
  eraId: number | string,
  coverVersion?: string | null,
  format?: 'jpeg',
): string {
  const params = new URLSearchParams();
  if (coverVersion) params.set('v', coverVersion);
  if (format) params.set('format', format);
  const query = params.toString();
  return joinApiPath(apiBaseUrl, `/eras/${encodeURIComponent(String(eraId))}/cover${query ? `?${query}` : ''}`);
}

const MINOR_WORDS = new Set(['a', 'an', 'and', 'for', 'in', 'of', 'on', 'the', 'to', 'with']);
const BRACKETED = /\[[^\]]*\]|\([^)]*\)/g;
const WORD_SEPARATORS = /[^\p{L}\p{N}]+/u;
const DIGITS = /^\p{N}+$/u;

/**
 * Up to two initials for a cover placeholder: "The Life of Pablo" → "LP", "DONDA 2 [V1]" → "D2",
 * "808s & Heartbreak" → "8H". Bracketed/parenthesized parts and minor words are skipped; numbers are kept whole
 * (so the result can be longer for names like "Vultures 10"). Empty when the name has no letters or digits.
 */
export function eraInitials(name: string): string {
  const words = name.replace(BRACKETED, ' ').split(WORD_SEPARATORS).filter(Boolean);
  const significant = words.filter((word) => !MINOR_WORDS.has(word.toLowerCase()));
  return (significant.length > 0 ? significant : words)
    .slice(0, 2)
    .map((word) => (DIGITS.test(word) ? word : (Array.from(word)[0] ?? '')))
    .join('')
    .toUpperCase();
}
