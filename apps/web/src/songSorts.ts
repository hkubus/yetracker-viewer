/**
 * Sort keys for the song listings. Mirrors `SORT_KEYS` in
 * `apps/api-rs/src/request.rs` — keep both lists in sync.
 */
export type SongSort = {
  id: string;
  label: string;
};

export const SONG_SORTS: SongSort[] = [
  { id: 'id', label: 'Catalog order' },
  { id: 'category', label: 'Category (best first)' },
  { id: 'leak-newest', label: 'Newest leak' },
  { id: 'leak-oldest', label: 'Oldest leak' },
  { id: 'file-newest', label: 'Newest file' },
  { id: 'name', label: 'Title A–Z' },
];

export const DEFAULT_SONG_SORT = 'id';

export function songSortIds(): string[] {
  return SONG_SORTS.map((sort) => sort.id);
}

/** Validates a `?sort=` value, falling back to the catalog order. */
export function normalizeSongSort(value: string | null | undefined): string {
  const trimmed = (value ?? '').trim();
  return songSortIds().includes(trimmed) ? trimmed : DEFAULT_SONG_SORT;
}

export function songSortLabel(id: string): string {
  return SONG_SORTS.find((sort) => sort.id === id)?.label ?? 'Catalog order';
}
