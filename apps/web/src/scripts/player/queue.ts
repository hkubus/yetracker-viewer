/**
 * The play queue: a snapshot of the list a song was started from, optionally continued past the page edge by
 * fetching the following pages of the same era list (SPEC §6.2 continuation attributes).
 */
import {
  DEFAULT_PAGE_SIZE,
  type EraInfo,
  FALLBACK_COLOR,
  hexColor,
  positiveInt,
  sanitizeTrack,
  type Track,
  trackFromSong,
} from './track.ts';

/** Where the list continues after the tracks already in the queue. */
export interface Continuation {
  era: EraInfo;
  /** Offset (in the era's song list with `params`) of the next song to fetch. */
  offset: number;
  /** Songs in the list (`X-Total-Count`), when known. */
  total: number | null;
  /** The list's `q` / `category` / `sort` parameters as a query string ('' = catalog order). */
  params: string;
}

export interface Page {
  songs: unknown[];
  total: number | null;
}

/** Fetches `limit` songs of the continuation's list starting at its offset. */
export type PageLoader = (continuation: Continuation, limit: number, signal: AbortSignal) => Promise<Page>;

/**
 * Outcome of `Queue.extend()`: playable tracks were `added`; there were `none` (the list ended, or the pages
 * searched held nothing playable); or a page request `failed` (the continuation is kept, so a later call retries).
 */
export type ExtendResult = 'added' | 'none' | 'failed';

/** Page size used when continuing a list (the API's maximum for era lists, as on the site). */
export const CONTINUATION_PAGE_SIZE = 100;
/** Upper bound of pages fetched to find the next playable song (whole eras are about ten pages). */
const MAX_PAGES_PER_EXTEND = 12;
/** Queues saved for the next visit keep at most this many tracks around the current one. */
const MAX_SAVED_TRACKS = 300;
const LIST_PARAMS = ['q', 'category', 'sort'] as const;

/** Keeps only the list parameters that change which songs a list contains or their order. */
export function listParams(value: string | null | undefined): string {
  const source = new URLSearchParams(value ?? '');
  const params = new URLSearchParams();
  for (const key of LIST_PARAMS) {
    const text = source.get(key)?.trim();
    if (text) params.set(key, text);
  }
  return params.toString();
}

function nonNegativeInt(value: unknown): number | null {
  if (value === 0 || value === '0') return 0;
  return positiveInt(value);
}

/**
 * The continuation described by a list scope's `data-queue-*` attributes. `rowCount` is the number of song rows
 * the scope renders (hidden ones included), so the next page starts at `offset + rowCount`. Returns null when the
 * attributes are missing or invalid, or when the list has no further songs.
 */
export function continuationFrom(
  attributes: { eraId?: string; offset?: string; total?: string; params?: string },
  rowCount: number,
  era: Omit<EraInfo, 'eraId'>,
): Continuation | null {
  const eraId = positiveInt(attributes.eraId);
  const offset = nonNegativeInt(attributes.offset);
  if (eraId === null || offset === null || rowCount <= 0) return null;
  const total = nonNegativeInt(attributes.total);
  const next = offset + rowCount;
  if (total !== null && next >= total) return null;
  return { era: { ...era, eraId }, offset: next, total, params: listParams(attributes.params) };
}

/** Link to the list page holding the song at 1-based `listPosition` (for payloads without `eraPosition`). */
function listPageHref(continuation: Continuation, listPosition: number, pageSize: number): string {
  const params = new URLSearchParams(continuation.params);
  const page = Math.ceil(listPosition / pageSize);
  if (page > 1) params.set('page', String(page));
  const query = params.toString();
  return `/eras/${continuation.era.eraId}${query ? `?${query}` : ''}`;
}

/** Playable tracks of one fetched page (older payloads get their position/page derived from the offset). */
export function tracksFromPage(songs: readonly unknown[], continuation: Continuation, pageSize = DEFAULT_PAGE_SIZE) {
  const catalogOrder = continuation.params === '';
  const tracks: Track[] = [];
  songs.forEach((song, index) => {
    const listPosition = continuation.offset + index + 1;
    const track = trackFromSong(
      song,
      continuation.era,
      catalogOrder ? { position: listPosition } : { pageHref: listPageHref(continuation, listPosition, pageSize) },
    );
    if (track) tracks.push(track);
  });
  return tracks;
}

export interface SavedQueue {
  tracks: Track[];
  index: number;
  continuation: Continuation | null;
}

export class Queue {
  readonly tracks: Track[];
  /** Position of the current track in `tracks`. */
  index: number;
  /** Where the list continues, or null when every song of the list is in `tracks`. */
  continuation: Continuation | null;
  private readonly pageSize: number;
  private pending: Promise<ExtendResult> | null = null;

  constructor(tracks: Track[], index: number, continuation: Continuation | null, pageSize = DEFAULT_PAGE_SIZE) {
    this.tracks = tracks;
    this.index = index;
    this.continuation = continuation;
    this.pageSize = pageSize;
  }

  static single(track: Track): Queue {
    return new Queue([track], 0, null);
  }

  get current(): Track | undefined {
    return this.tracks[this.index];
  }

  hasPrevious(): boolean {
    return this.index > 0;
  }

  /** True when a following track exists or may exist on a page that hasn't been fetched yet. */
  hasNext(): boolean {
    return this.index < this.tracks.length - 1 || this.continuation !== null;
  }

  /** Whether moving by `direction` needs `extend()` first. */
  needsExtension(direction: 1 | -1): boolean {
    return direction === 1 && this.index >= this.tracks.length - 1 && this.continuation !== null;
  }

  /**
   * Fetches following pages until at least one new playable track was added, the list ended, or a page failed to
   * load. Concurrent calls share one request chain.
   */
  extend(load: PageLoader, signal: AbortSignal): Promise<ExtendResult> {
    this.pending ??= this.fetchMore(load, signal).finally(() => {
      this.pending = null;
    });
    return this.pending;
  }

  private async fetchMore(load: PageLoader, signal: AbortSignal): Promise<ExtendResult> {
    for (let page = 0; page < MAX_PAGES_PER_EXTEND; page += 1) {
      const continuation = this.continuation;
      if (!continuation) return 'none';
      if (signal.aborted) return 'failed';
      let result: Page;
      try {
        result = await load(continuation, CONTINUATION_PAGE_SIZE, signal);
      } catch {
        // Network trouble: keep the continuation so a later Next can try again.
        return 'failed';
      }
      const known = new Set(this.tracks.map((track) => track.id));
      const added = tracksFromPage(result.songs, continuation, this.pageSize).filter((track) => !known.has(track.id));
      this.tracks.push(...added);
      const offset = continuation.offset + result.songs.length;
      const total = result.total ?? continuation.total;
      const ended = result.songs.length === 0 || (total !== null && offset >= total);
      this.continuation = ended ? null : { ...continuation, offset, total };
      if (added.length > 0) return 'added';
    }
    return 'none';
  }

  /** A bounded, storable copy (tracks around the current one). */
  toJSON(): SavedQueue {
    const start = Math.max(0, Math.min(this.index - MAX_SAVED_TRACKS / 2, this.tracks.length - MAX_SAVED_TRACKS));
    const tracks = this.tracks.slice(start, start + MAX_SAVED_TRACKS);
    const endsEarly = start + tracks.length < this.tracks.length;
    return {
      tracks,
      index: this.index - start,
      // A trimmed tail can't be continued from the stored offset.
      continuation: endsEarly ? null : this.continuation,
    };
  }

  /** Restores a stored queue; null when it is unusable or doesn't contain `current`. */
  static fromJSON(value: unknown, current: Track, pageSize = DEFAULT_PAGE_SIZE): Queue | null {
    if (typeof value !== 'object' || value === null) return null;
    const data = value as Record<string, unknown>;
    if (!Array.isArray(data.tracks)) return null;
    const tracks = data.tracks
      .slice(0, MAX_SAVED_TRACKS)
      .map(sanitizeTrack)
      .filter((track): track is Track => track !== null);
    const stored = nonNegativeInt(data.index);
    const index =
      stored !== null && tracks[stored]?.id === current.id
        ? stored
        : tracks.findIndex((track) => track.id === current.id);
    if (index < 0) return null;
    tracks[index] = current;
    return new Queue(tracks, index, sanitizeContinuation(data.continuation), pageSize);
  }
}

function sanitizeContinuation(value: unknown): Continuation | null {
  if (typeof value !== 'object' || value === null) return null;
  const data = value as Record<string, unknown>;
  const era = (typeof data.era === 'object' && data.era !== null ? data.era : {}) as Record<string, unknown>;
  const eraId = positiveInt(era.eraId);
  const offset = nonNegativeInt(data.offset);
  if (eraId === null || offset === null) return null;
  const version =
    typeof era.coverVersion === 'string' && /^[\w.-]{1,64}$/.test(era.coverVersion) ? era.coverVersion : null;
  return {
    era: {
      eraId,
      eraName: typeof era.eraName === 'string' ? era.eraName.slice(0, 500) : '',
      color: hexColor(era.color) ?? FALLBACK_COLOR,
      hasCover: era.hasCover === true && version !== null,
      coverVersion: version,
    },
    offset,
    total: nonNegativeInt(data.total),
    params: listParams(typeof data.params === 'string' ? data.params : ''),
  };
}
