import type { SongSortKey } from '@yetracker/types';

/** A sort offered by the era song list. `id` is the API's `sort` value. */
export interface SongSort {
  id: SongSortKey;
  label: string;
}

/**
 * Sorts offered by the era song list, in menu order. The labels describe the API's orders: catalog order is the
 * sheet order; the category order is best of, special, grails, wanted, unmarked, worst of, AI; the date sorts put
 * songs without a date last; the title order is natural (`[V3]` before `[V10]`). Every sort breaks ties by catalog
 * order.
 */
export const SONG_SORTS: readonly SongSort[] = [
  { id: 'catalog', label: 'Catalog order' },
  { id: 'category', label: 'Category (best first)' },
  { id: 'leak-newest', label: 'Leak date (newest first)' },
  { id: 'leak-oldest', label: 'Leak date (oldest first)' },
  { id: 'file-newest', label: 'File date (newest first)' },
  { id: 'name', label: 'Title (A–Z)' },
];

/**
 * The API's default order. It is never sent explicitly: requests and URLs simply omit `sort`, which also keeps
 * working against API versions that only know the `id` alias.
 */
export const DEFAULT_SONG_SORT: SongSortKey = 'catalog';

/** Older URLs spell catalog order `sort=id`. */
const SORT_ALIASES: Readonly<Partial<Record<string, SongSortKey>>> = { id: 'catalog' };

/** Typed as `string[]` so callers can test arbitrary input with `.includes()`. */
export function songSortIds(): string[] {
  return SONG_SORTS.map((sort) => sort.id);
}

/** Validates a `?sort=` value (aliases included), falling back to catalog order. */
export function normalizeSongSort(value: string | null | undefined): SongSortKey {
  const trimmed = (value ?? '').trim();
  const id = SORT_ALIASES[trimmed] ?? trimmed;
  return SONG_SORTS.find((sort) => sort.id === id)?.id ?? DEFAULT_SONG_SORT;
}
