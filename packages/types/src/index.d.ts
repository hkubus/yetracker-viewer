/**
 * Types for the yetracker viewer HTTP API. Type-only: the package has no runtime code.
 *
 * Convention: every key is always present in responses; a missing value is `null` (typed `T | null`).
 * JSON keys are camelCase. Timestamps are Unix seconds (UTC).
 */

/** Values of the sheet's "Quality" column. */
export type Quality = 'Low Quality' | 'High Quality' | 'CD Quality' | 'Lossless' | 'Not Available' | 'Recording';

/** Values of the sheet's "Available Length" column. */
export type AvailableLength =
  | 'Full'
  | 'Snippet'
  | 'Confirmed'
  | 'Beat Only'
  | 'Partial'
  | 'Tagged'
  | 'OG File'
  | 'Stem Bounce'
  | 'Rumored'
  | 'Conflicting Sources';

/** How much of a catalog date is known. The timestamp is UTC midnight of the first day of that period. */
export type DatePrecision = 'day' | 'month' | 'year';

/**
 * Where the song's audio stands:
 * - `none`: the song has no source link.
 * - `unsupported`: there is a link, but it isn't downloaded here: an unsupported host, quality "Not Available", or a
 *   YouTube link while YouTube downloads are disabled (`YOUTUBE_DOWNLOAD=false`).
 * - `pending`: queued for download or waiting to retry.
 * - `failed`: the downloader gave up (e.g. 404/410, not an audio file, too many failed attempts); it tries again
 *   30 days later.
 * - `downloaded`: the file is on disk; equivalent to `playable: true`.
 */
export type DownloadState = 'none' | 'unsupported' | 'pending' | 'failed' | 'downloaded';

/** A link found inside a song's notes cell. */
export interface NotesLink {
  text: string;
  url: string;
}

/** `GET /eras` (array of main eras, ordered by `position`) and `GET /eras/:id`. */
export interface Era {
  id: number;
  /** Catalog (sheet) order. */
  position: number;
  /** First line of the sheet's era-name cell. */
  name: string;
  /** Remaining lines of the era-name cell joined with a space, e.g. "(Collaboration with JAŸ-Z as The Throne)". */
  subtitle: string | null;
  /** May contain line breaks (`\n`). */
  notes: string;
  /** May contain line breaks (`\n`). */
  description: string;
  /** Six hex digits without `#` (the API falls back to `666666`). */
  dominantColor: string;
  /** A cover file exists: request `/eras/:id/cover?v=<coverVersion>`. */
  hasCover: boolean;
  /** 12-hex hash of the cover bytes; `null` when there is no cover. */
  coverVersion: string | null;
  songsCount: number;
}

/** `GET /eras/:id/songs` and `GET /songs` (plain list mode) return `Song[]`; `GET /songs/:id` returns one. */
export interface Song {
  id: number;
  /** `null` only for a row without an era: the column is nullable, though the importer always sets it. */
  eraId: number | null;
  /** 1-based index inside the era in catalog order; the song is on era page `ceil(eraPosition / pageSize)`. */
  eraPosition: number;
  catalogId: string;
  /** Full name cell. May contain `\n`: title line, then credit and alternate-title lines. */
  name: string;
  /** First line of `name`, category emoji markers included. */
  title: string;
  /** The sheet section (sub-era header row) the song sits under. */
  subEra: string | null;
  /** May contain line breaks (`\n`). */
  notes: string;
  /** Anchors found inside the notes cell; may be empty. */
  notesLinks: NotesLink[];
  /** Start of the period the file dates from (see `fileDatePrecision`). */
  fileDate: number | null;
  /** `null` exactly when `fileDate` is `null`. */
  fileDatePrecision: DatePrecision | null;
  /** Start of the period the song leaked in (see `leakDatePrecision`). */
  leakDate: number | null;
  /** `null` exactly when `leakDate` is `null`. */
  leakDatePrecision: DatePrecision | null;
  availableLength: AvailableLength | null;
  /** Length from the sheet, in seconds. */
  trackLength: number | null;
  /** The sheet gave an approximate length (`~2:00`). */
  trackLengthApprox: boolean;
  quality: Quality | null;
  /** Primary link: `links[0]`, or `null` when there are none. */
  url: string | null;
  /** Every http(s) link of the Link(s) cell, deduplicated, primary first. */
  links: string[];
  downloadState: DownloadState;
  /** `/songs/:id/stream` can serve the song (same as `downloadState === 'downloaded'`). */
  playable: boolean;
  /** Probed audio duration in seconds; `null` until known. */
  duration: number | null;
}

/** A search/filter result: the song plus the display data of its era. */
export interface SearchSong extends Song {
  eraName: string;
  /** The era's dominant color: six hex digits without `#`. */
  dominantColor: string;
  eraHasCover: boolean;
  eraCoverVersion: string | null;
}

/**
 * `GET /songs` in search/filter mode: used when `q` has searchable text (letters or digits) or a category marker
 * (⭐ ✨ 🏆 🏅 🗑️ 🤖), or when any of `era`, `eraFrom`, `eraTo`, `quality`, `availability`, `playable`, `category` has
 * a non-blank value. A symbol-only `q` (`???`) counts as blank, like a missing one: with no filter either, the answer
 * is the plain `Song[]` list.
 */
export interface SearchResponse {
  songs: SearchSong[];
  /** Number of matches after every filter (not capped). */
  total: number;
  offset: number;
  limit: number;
}

/** `GET /status`. */
export interface StatusResponse {
  status: 'ok';
  /** When the last *successful* catalog import finished; `null` until one has succeeded. */
  lastImportAt: number | null;
  /** Whether the latest import attempt succeeded; `null` before the first attempt. */
  lastImportOk: boolean | null;
  /**
   * A fixed, generic message ("Catalog update failed") when the latest attempt failed, else `null`. The details only
   * go to the server log.
   */
  lastImportError: string | null;
  /** Number of main eras (the length of `GET /eras`). */
  eras: number;
  songs: number;
  /** Songs whose audio file is on disk (`playable: true`). */
  playableSongs: number;
}

/** `GET /health`: `ok` with status 200, `error` with status 503. */
export interface HealthResponse {
  status: 'ok' | 'error';
}

/** `GET /songs/:id/duration`. */
export interface DurationResponse {
  duration: number;
}

/** JSON body of 404 (unknown route) and 405 responses. Other errors are `text/plain` messages. */
export interface ApiErrorBody {
  error: string;
}

/** `sort` values accepted by `/songs` and `/eras/:id/songs`. `catalog` is sheet order; `id` is kept as an alias. */
export type SongSortKey = 'id' | 'catalog' | 'category' | 'leak-newest' | 'leak-oldest' | 'file-newest' | 'name';

/** `category` values accepted by `/songs` and `/eras/:id/songs`. */
export type SongCategoryId = 'best-of' | 'special' | 'grails' | 'wanted' | 'worst-of' | 'ai';

/** Query parameters of `GET /songs`. `q` is at most 100 characters; `limit` at most 50; `offset` at most 10000. */
export interface SongSearchParams {
  q?: string;
  era?: number;
  /** Era id; the range is evaluated by era position. */
  eraFrom?: number;
  /** Era id; the range is evaluated by era position. */
  eraTo?: number;
  quality?: Quality;
  availability?: AvailableLength;
  playable?: boolean;
  category?: SongCategoryId;
  sort?: SongSortKey;
  offset?: number;
  limit?: number;
}
