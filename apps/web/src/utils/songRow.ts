/**
 * Pure helpers that turn an API song into what a song row shows. Shared by the era song list (server-rendered) and
 * its client script. Every field that newer API versions added is optional here, so older payloads still render.
 */
import type { DownloadState, NotesLink, Song } from '@yetracker/types';
import { formatDuration } from './duration.ts';

/** A song as a list receives it: `id` plus whatever else the API sent. `downloaded` only exists in older payloads. */
export type SongPayload = Pick<Song, 'id'> & Partial<Omit<Song, 'id'>> & { downloaded?: number | string | null };

export interface SongName {
  /** The first line of the name (category markers included). */
  title: string;
  /** The other lines: credits, alternate titles. */
  details: string[];
}

/** Splits a song name into the title line and the muted detail lines. */
export function songName(song: Pick<SongPayload, 'name' | 'title'>): SongName {
  const lines = (typeof song.name === 'string' ? song.name : '')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  const title = (typeof song.title === 'string' ? song.title.trim() : '') || lines[0] || 'Untitled';
  const details = lines[0] === title ? lines.slice(1) : lines.filter((line) => line !== title);
  return { title, details };
}

/** Parses an absolute http(s) URL; anything else (relative, `javascript:`, malformed) is rejected. */
export function httpUrl(value: unknown): string | null {
  if (typeof value !== 'string' || value.trim() === '') return null;
  try {
    const url = new URL(value.trim());
    return url.protocol === 'https:' || url.protocol === 'http:' ? url.href : null;
  } catch {
    return null;
  }
}

/** The song's source links, primary first, deduplicated; only http(s) URLs survive. */
export function songLinks(song: Pick<SongPayload, 'url' | 'links'>): string[] {
  const candidates = [song.url, ...(Array.isArray(song.links) ? song.links : [])];
  const links: string[] = [];
  for (const candidate of candidates) {
    const url = httpUrl(candidate);
    if (url && !links.includes(url)) links.push(url);
  }
  return links;
}

/** Short readable form of a link: host without `www.` plus path and query, e.g. `imgur.gg/f/abc123`. */
export function linkLabel(href: string): string {
  try {
    const url = new URL(href);
    const host = url.hostname.replace(/^www\./, '');
    const rest = `${url.pathname === '/' ? '' : url.pathname}${url.search}`;
    let readable = rest;
    try {
      readable = decodeURIComponent(rest);
    } catch {
      // Keep the encoded form when it is not valid percent-encoding.
    }
    return `${host}${readable}`;
  } catch {
    return href;
  }
}

const DOWNLOAD_STATES: readonly string[] = [
  'none',
  'unsupported',
  'pending',
  'failed',
  'downloaded',
] satisfies DownloadState[];

/** Whether the API can stream the song. */
export function isPlayable(song: Pick<SongPayload, 'playable' | 'downloadState'>): boolean {
  if (typeof song.playable === 'boolean') return song.playable;
  return song.downloadState === 'downloaded';
}

/**
 * Where the song's audio stands (see `DownloadState`). Older payloads have no `downloadState`: there, a song with a
 * link but no download job (`downloaded: null`) is on a host the API does not download from, and a song with a job
 * that is not playable is still waiting for its download.
 */
export function downloadStateOf(song: SongPayload): DownloadState {
  if (isPlayable(song)) return 'downloaded';
  const state = song.downloadState;
  // "downloaded" without a playable file: the file went missing and will be fetched again.
  if (state === 'downloaded') return 'pending';
  if (typeof state === 'string' && DOWNLOAD_STATES.includes(state)) return state;
  if (songLinks(song).length === 0) return 'none';
  return song.downloaded === null || song.downloaded === undefined ? 'unsupported' : 'pending';
}

/** Why a song has no play button, for everything but `downloaded`. */
export const DOWNLOAD_STATE_LABELS: Readonly<Record<Exclude<DownloadState, 'downloaded'>, string>> = {
  none: 'No audio file',
  unsupported: 'Source can’t be downloaded here — open the source',
  pending: 'Not downloaded yet',
  failed: 'Download failed — open the source',
};

export interface SongLength {
  /** `m:ss` / `h:mm:ss`. */
  text: string;
  /** The catalog only gives an approximate length (`~2:00`). */
  approx: boolean;
}

function positiveSeconds(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null;
}

/** The catalog's track length (with its "approximate" flag), else the probed duration of the audio file. */
export function songLength(
  song: Pick<SongPayload, 'trackLength' | 'trackLengthApprox' | 'duration'>,
): SongLength | null {
  const catalog = positiveSeconds(song.trackLength);
  if (catalog !== null) return { text: formatDuration(catalog), approx: song.trackLengthApprox === true };
  const probed = positiveSeconds(song.duration);
  return probed === null ? null : { text: formatDuration(probed), approx: false };
}

/**
 * Whole seconds for the player's `data-track-length`: the probed duration first (it describes the file), else the
 * catalog's. Fractions are cut off, never rounded up, like `formatDuration()` does everywhere (a 321.8 s file is
 * 5:21 in the list and in the player, before and after the player knows the file's own duration).
 */
export function playbackLength(song: Pick<SongPayload, 'trackLength' | 'duration'>): number | null {
  const seconds = Math.floor(positiveSeconds(song.duration) ?? positiveSeconds(song.trackLength) ?? 0);
  return seconds > 0 ? seconds : null;
}

export interface NotesSegment {
  text: string;
  /** Present when the segment is a link (always http or https). */
  href?: string;
}

export interface LinkedNotes {
  segments: NotesSegment[];
  /** Links whose text does not occur in the notes (shown after them). */
  extraLinks: Array<Required<NotesSegment>>;
}

/**
 * Splits notes into plain-text and link segments: each notes link claims the first occurrence of its text that no
 * earlier link claimed. Links with a non-http(s) URL are dropped; links whose text is not found are returned
 * separately.
 */
export function linkNotes(notes: string, links: readonly NotesLink[] | null | undefined): LinkedNotes {
  const claimed: Array<{ start: number; end: number; href: string }> = [];
  const extraLinks: LinkedNotes['extraLinks'] = [];
  for (const link of links ?? []) {
    const href = httpUrl(link?.url);
    if (!href) continue;
    const text = typeof link.text === 'string' ? link.text.trim() : '';
    let start = -1;
    if (text) {
      for (let from = 0; ; from = start + 1) {
        start = notes.indexOf(text, from);
        const end = start + text.length;
        if (start === -1 || !claimed.some((range) => start < range.end && end > range.start)) break;
      }
    }
    if (start === -1) extraLinks.push({ text: text || linkLabel(href), href });
    else claimed.push({ start, end: start + text.length, href });
  }
  claimed.sort((a, b) => a.start - b.start);
  const segments: NotesSegment[] = [];
  let cursor = 0;
  for (const range of claimed) {
    if (range.start > cursor) segments.push({ text: notes.slice(cursor, range.start) });
    segments.push({ text: notes.slice(range.start, range.end), href: range.href });
    cursor = range.end;
  }
  if (cursor < notes.length) segments.push({ text: notes.slice(cursor) });
  return { segments, extraLinks };
}

/** `1 song`, `2 songs`, `1,234 songs`. */
export function countLabel(count: number, singular: string, plural = `${singular}s`): string {
  return `${count.toLocaleString('en-US')} ${count === 1 ? singular : plural}`;
}
