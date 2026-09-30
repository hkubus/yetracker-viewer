import type { SongCategoryId } from '@yetracker/types';

export interface SongCategory {
  id: SongCategoryId;
  /** Readable name. Menus show it before the emoji so type-ahead and screen readers start with a word. */
  label: string;
  /** The marker the catalog puts in front of the song titles of this category. */
  emoji: string;
}

/**
 * The `category` filters of the song listings, in the API's category sort order (unmarked songs sort between
 * "Wanted" and "Worst of").
 */
export const SONG_CATEGORIES: readonly SongCategory[] = [
  { id: 'best-of', label: 'Best of', emoji: '⭐' },
  { id: 'special', label: 'Special', emoji: '✨' },
  { id: 'grails', label: 'Grails', emoji: '🏆' },
  { id: 'wanted', label: 'Wanted', emoji: '🏅' },
  { id: 'worst-of', label: 'Worst of', emoji: '🗑️' },
  { id: 'ai', label: 'AI', emoji: '🤖' },
];

/** Typed as `string[]` so callers can test arbitrary input with `.includes()`. */
export function songCategoryIds(): string[] {
  return SONG_CATEGORIES.map((category) => category.id);
}

/** Emoji variation selectors: `🗑️` and `🗑` are the same marker. */
const VARIATION_SELECTORS = /[\uFE0E\uFE0F]/g;

/** The category markers that occur anywhere in `text` (each once, in menu order), variation selectors ignored. */
export function categoryMarkersIn(text: string): string[] {
  return SONG_CATEGORIES.map((category) => category.emoji.replace(VARIATION_SELECTORS, '')).filter((marker) =>
    text.includes(marker),
  );
}

export function isSongCategoryId(value: string): value is SongCategoryId {
  return SONG_CATEGORIES.some((category) => category.id === value);
}

/** Validates a `?category=` value; `''` means no category filter. */
export function normalizeSongCategory(value: string | null | undefined): SongCategoryId | '' {
  const trimmed = (value ?? '').trim();
  return isSongCategoryId(trimmed) ? trimmed : '';
}
