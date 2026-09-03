export type Quality = 'Low Quality' | 'High Quality' | 'CD Quality' | 'Lossless' | 'Not Available' | 'Recording';
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
/**
 * A track. Most fields stay optional for backward compatibility with
 * callers that build partial objects (player queue, search results, …).
 * `id`/`name` are effectively always present from the API.
 */
export type Song = {
  id?: number;
  eraId?: number;
  catalogId?: string;
  name?: string;
  notes?: string;
  trackLength?: number;
  fileDate?: number;
  leakDate?: number;
  url?: string;
  availableLength?: AvailableLength;
  quality?: Quality;
  /** 1 when a local file is stored, else 0. */
  downloaded?: number;
  /** Whether the track can be streamed from the API. */
  playable?: boolean;
  /** Resolved audio duration in seconds (null when unknown). */
  duration?: number | null;
  /** Hex color (with or without leading '#') used as UI accent. */
  dominantColor?: string;
  eraName?: string;
  eraPosition?: number;
  sourceUrl?: string;
  filename?: string;
};
export type Era = {
  id: number;
  name: string;
  dominantColor: string;
  coverVersion?: string;
  notes: string;
  description: string;
  songsCount?: number;
};
/** A "yetracker sheet" category shown on the index and /categories pages. */
export type Category = {
  id: string;
  name: string;
  description: string;
  songsCount: number;
  sourceUrl: string;
};
/** A single album/demo copy inside an {@link AlbumCopyGroup}. */
export type AlbumCopy = {
  id: number;
  eraId: number | null;
  eraName: string | null;
  name: string | null;
  notes: string | null;
  trackLength: number | null;
  fileDate: number | null;
  availableLength: string | null;
  quality: string | null;
  url: string | null;
  playable: boolean;
  duration: number | null;
  coverVersion: string | null;
};
/** Copies grouped under one album/demo title. */
export type AlbumCopyGroup = {
  name: string;
  copies: AlbumCopy[];
};
/** Paginated song list payload shape (items + X-Total-Count header). */
export type Paginated<T> = {
  items: T[];
  total: number;
};
