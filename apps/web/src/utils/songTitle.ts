/**
 * Display helpers for song titles in the home page lists (recently leaked, search results). Nearly every title
 * ends in credits — "Stronger [V5] (prod. Kanye West, MIKE DEAN & Timbaland)" — that are often longer than the
 * song's own name; showing them muted keeps the name scannable. The full title stays what players, links and
 * accessible names use.
 */

export interface TitleParts {
  /** The song's own name (category markers and version tags included). */
  main: string;
  /** The trailing credits, from the first `(feat. …`, `(ft. …`, `(prod. …`, `(ref. …` or `(with …` on; or ''. */
  credits: string;
}

const CREDITS_START = /\s\((?:feat|ft|prod|ref)\.\s|\s\(with\s/i;

/** Splits the credits off a title; a title without credits (or with nothing before them) comes back whole. */
export function splitCredits(title: string): TitleParts {
  const text = title.trim();
  const match = CREDITS_START.exec(text);
  if (!match) return { main: text, credits: '' };
  const main = text.slice(0, match.index).trim();
  return main ? { main, credits: text.slice(match.index).trim() } : { main: text, credits: '' };
}
