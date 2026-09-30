/**
 * Presentation helpers for search-result songs outside the era song list (home page search results, recently
 * leaked). Every field is read defensively: older API versions omit `title`, the cover fields and the date
 * precisions. Title splitting and the playback length come from `songRow.ts`, so every song list agrees.
 */
import type { SearchSong } from '@yetracker/types';
import { colorValue } from './color.ts';
import { playbackLength, type SongName, songName } from './songRow.ts';

/** A song as the search endpoints send it; any field may be missing or null in older payloads. */
export type SearchSongPayload = { [Key in keyof SearchSong]?: SearchSong[Key] | null };

/** Era display data used when a song payload lacks its era's fields. */
export interface EraDisplayInfo {
  name?: string | null;
  dominantColor?: string | null;
  hasCover?: boolean | null;
  coverVersion?: string | null;
}

/** Songs per page in the era song list; a song's page is `ceil(eraPosition / pageSize)`. */
export const DEFAULT_ERA_PAGE_SIZE = 100;

/** A positive safe integer from a number or a canonical decimal string; null otherwise. */
export function positiveInteger(value: unknown): number | null {
  const number =
    typeof value === 'number'
      ? value
      : typeof value === 'string' && /^[1-9]\d*$/.test(value)
        ? Number(value)
        : Number.NaN;
  return Number.isSafeInteger(number) && number > 0 ? number : null;
}

function text(value: unknown): string {
  return typeof value === 'string' ? value.trim() : '';
}

/** The title line and the muted detail lines (credits, alternate titles) of a song name. */
export function songTextLines(song: Pick<SearchSongPayload, 'name' | 'title'>): SongName {
  return songName({ name: song.name ?? undefined, title: song.title ?? undefined });
}

/** The era song-list page (1-based) that contains the song at `eraPosition`. */
export function eraPageOf(eraPosition: unknown, pageSize: number = DEFAULT_ERA_PAGE_SIZE): number {
  const position = positiveInteger(eraPosition);
  const size = Number.isSafeInteger(pageSize) && pageSize > 0 ? pageSize : DEFAULT_ERA_PAGE_SIZE;
  return position === null ? 1 : Math.ceil(position / size);
}

/** Deep link to the song's row in its era (`/eras/31?page=5#song-6751`); null without usable ids. */
export function songHref(
  song: Pick<SearchSongPayload, 'id' | 'eraId' | 'eraPosition'>,
  pageSize: number = DEFAULT_ERA_PAGE_SIZE,
): string | null {
  const id = positiveInteger(song.id);
  const eraId = positiveInteger(song.eraId);
  if (id === null || eraId === null) return null;
  const page = eraPageOf(song.eraPosition, pageSize);
  return `/eras/${eraId}${page > 1 ? `?page=${page}` : ''}#song-${id}`;
}

/**
 * The attributes of a play button as the player expects them: `data-play-target`, `data-id`, `data-title`,
 * `data-era-id`, `data-era-name`, `data-era-position`, `data-track-length` (empty when unknown),
 * `data-dominant-color` (6 hex digits), `data-has-cover`, `data-cover-version` (empty without a cover), plus
 * `aria-pressed="false"` and `title="Play"` (the player flips both for the current song). The accessible name
 * (`Play <title>`) is left to the caller. Returns null when the song has no usable id.
 *
 * `era` fills in what older payloads lack. Without any cover information the cover is assumed to exist when a
 * version is known; the player falls back to a placeholder when it doesn't load.
 */
export function playButtonAttributes(
  song: SearchSongPayload,
  era?: EraDisplayInfo | null,
): Record<string, string> | null {
  const id = positiveInteger(song.id);
  if (id === null) return null;
  const eraId = positiveInteger(song.eraId);
  const eraPosition = positiveInteger(song.eraPosition);
  const length = playbackLength({ duration: song.duration ?? null, trackLength: song.trackLength ?? null });
  const coverVersion = text(song.eraCoverVersion) || text(era?.coverVersion);
  const hasCover =
    typeof song.eraHasCover === 'boolean'
      ? song.eraHasCover
      : typeof era?.hasCover === 'boolean'
        ? era.hasCover
        : coverVersion !== '';
  return {
    'data-play-target': '',
    'data-id': String(id),
    'data-title': songTextLines(song).title,
    'data-era-id': eraId === null ? '' : String(eraId),
    'data-era-name': text(song.eraName) || text(era?.name),
    'data-era-position': eraPosition === null ? '' : String(eraPosition),
    'data-track-length': length === null ? '' : String(length),
    'data-dominant-color': colorValue(song.dominantColor ?? era?.dominantColor).slice(1),
    'data-has-cover': hasCover ? 'true' : 'false',
    'data-cover-version': hasCover ? coverVersion : '',
    'aria-pressed': 'false',
    title: 'Play',
  };
}
