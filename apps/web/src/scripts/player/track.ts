/**
 * The player's notion of a song: plain data built from a play button's attributes or from an API song payload,
 * safe to keep across page navigations and to store in localStorage.
 */

/** Songs per era page on the site (`/eras/:id?page=n`). */
export const DEFAULT_PAGE_SIZE = 100;

export interface Track {
  id: number;
  /** Display title: the first line of the song name. */
  title: string;
  eraId: number | null;
  eraName: string;
  /** 1-based position in the era's catalog order; decides which era page lists the song. */
  eraPosition: number | null;
  /** Length in seconds from the catalog, used until the audio element knows better. */
  length: number | null;
  /** The era's color: six lowercase hex digits without `#`. */
  color: string;
  hasCover: boolean;
  coverVersion: string | null;
  /** Page that lists the song, for payloads without `eraPosition` (older API versions). */
  pageHref: string | null;
}

/** Display data shared by the songs of one era. */
export interface EraInfo {
  eraId: number;
  eraName: string;
  color: string;
  hasCover: boolean;
  coverVersion: string | null;
}

export const FALLBACK_COLOR = '666666';
const HEX_COLOR = /^#?([\da-f]{6})$/i;
const COVER_VERSION = /^[\w.-]{1,64}$/;
const LINE_BREAK = /\r\n?|\n/;
const WHITESPACE = /\s+/g;
const MAX_TITLE_LENGTH = 500;

/** A positive safe integer from a number or a canonical decimal string (no leading zeros); otherwise null. */
export function positiveInt(value: unknown): number | null {
  const number =
    typeof value === 'number'
      ? value
      : typeof value === 'string' && /^[1-9]\d*$/.test(value.trim())
        ? Number(value)
        : NaN;
  return Number.isSafeInteger(number) && number > 0 ? number : null;
}

/** A finite number > 0 from a number or a numeric string; otherwise null (so `0` and `''` mean "unknown"). */
export function positiveNumber(value: unknown): number | null {
  const number =
    typeof value === 'number' ? value : typeof value === 'string' && value.trim() !== '' ? Number(value) : NaN;
  return Number.isFinite(number) && number > 0 ? number : null;
}

/** `rrggbb` (lowercase, no `#`) from `#rrggbb`/`rrggbb`; otherwise null. */
export function hexColor(value: unknown): string | null {
  const match = typeof value === 'string' ? HEX_COLOR.exec(value.trim()) : null;
  return match?.[1] ? match[1].toLowerCase() : null;
}

/** The first non-empty line of a (possibly multi-line) name, whitespace collapsed. */
export function firstLine(value: unknown): string {
  if (typeof value !== 'string') return '';
  for (const line of value.split(LINE_BREAK)) {
    const text = line.replace(WHITESPACE, ' ').trim();
    if (text) return text.slice(0, MAX_TITLE_LENGTH);
  }
  return '';
}

function cleanText(value: unknown, max = MAX_TITLE_LENGTH): string {
  return typeof value === 'string' ? value.replace(WHITESPACE, ' ').trim().slice(0, max) : '';
}

function coverVersion(value: unknown): string | null {
  const text = typeof value === 'string' ? value.trim() : '';
  return COVER_VERSION.test(text) ? text : null;
}

/** Same-site path (`/eras/…`), or null: stored/derived links must never point elsewhere. */
function sitePath(value: unknown): string | null {
  return typeof value === 'string' && value.startsWith('/') && !value.startsWith('//') ? value : null;
}

/**
 * Reads a play button's `data-*` attributes (SPEC §6.2), also accepting the older `data-eraId` spelling (the
 * browser lowercases it to `data-eraid`). `fallbackColor` is used when the button has no `data-dominant-color`.
 */
export function trackFromDataset(
  data: Readonly<Record<string, string | undefined>>,
  fallback: { color?: string | null; pageHref?: string | null } = {},
): Track | null {
  const id = positiveInt(data.id);
  if (id === null) return null;
  const version = coverVersion(data.coverVersion);
  return {
    id,
    title: firstLine(data.title) || 'Untitled',
    eraId: positiveInt(data.eraId ?? data.eraid),
    eraName: cleanText(data.eraName),
    eraPosition: positiveInt(data.eraPosition),
    length: positiveNumber(data.trackLength),
    color: hexColor(data.dominantColor) ?? hexColor(fallback.color) ?? FALLBACK_COLOR,
    // Older markup has no data-has-cover; a cover version then implies a cover (as in CoverImage).
    hasCover: data.hasCover === undefined ? version !== null : data.hasCover === 'true',
    coverVersion: version,
    pageHref: sitePath(fallback.pageHref),
  };
}

/**
 * Builds a track from an API song payload (`/eras/:id/songs`, v1 or v2). Returns null for songs that can't be
 * played. `position` is the song's catalog position when the payload lacks `eraPosition` and the list is in
 * catalog order; `pageHref` links older payloads to the page they came from.
 */
export function trackFromSong(
  song: unknown,
  era: EraInfo,
  fallback: { position?: number | null; pageHref?: string | null } = {},
): Track | null {
  if (typeof song !== 'object' || song === null) return null;
  const data = song as Record<string, unknown>;
  if (data.playable !== true) return null;
  const id = positiveInt(data.id);
  if (id === null) return null;
  const eraPosition = positiveInt(data.eraPosition) ?? positiveInt(fallback.position ?? null);
  return {
    id,
    title: cleanText(data.title) || firstLine(data.name) || 'Untitled',
    eraId: positiveInt(data.eraId) ?? era.eraId,
    eraName: era.eraName,
    eraPosition,
    length: positiveNumber(data.duration) ?? positiveNumber(data.trackLength),
    color: era.color,
    hasCover: era.hasCover,
    coverVersion: era.coverVersion,
    pageHref: eraPosition === null ? sitePath(fallback.pageHref) : null,
  };
}

/** Validates a track read back from storage (it may come from an older version of the site, or be tampered with). */
export function sanitizeTrack(value: unknown): Track | null {
  if (typeof value !== 'object' || value === null) return null;
  const data = value as Record<string, unknown>;
  const id = positiveInt(data.id);
  if (id === null) return null;
  const version = coverVersion(data.coverVersion);
  return {
    id,
    title: cleanText(data.title) || 'Untitled',
    eraId: positiveInt(data.eraId),
    eraName: cleanText(data.eraName),
    eraPosition: positiveInt(data.eraPosition),
    length: positiveNumber(data.length),
    color: hexColor(data.color) ?? FALLBACK_COLOR,
    hasCover: data.hasCover === true && version !== null,
    coverVersion: version,
    pageHref: sitePath(data.pageHref),
  };
}

/** The era page that lists the track (`/eras/31?page=5#song-6751`), or null when the era is unknown. */
export function eraHref(track: Track, pageSize = DEFAULT_PAGE_SIZE): string | null {
  if (track.eraId === null) return null;
  const anchor = `#song-${track.id}`;
  if (track.eraPosition !== null) {
    const page = Math.ceil(track.eraPosition / (pageSize > 0 ? pageSize : DEFAULT_PAGE_SIZE));
    return `/eras/${track.eraId}${page > 1 ? `?page=${page}` : ''}${anchor}`;
  }
  if (track.pageHref) return `${track.pageHref.replace(/#.*$/, '')}${anchor}`;
  return `/eras/${track.eraId}${anchor}`;
}
