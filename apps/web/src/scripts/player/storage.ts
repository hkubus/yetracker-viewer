/**
 * Player preferences and the "resume where you left off" state in localStorage. Every read is validated: values may
 * come from older versions of the site or be edited by hand, and storage may be unavailable (private mode).
 */
import { Queue, type SavedQueue } from './queue.ts';
import { sanitizeTrack, type Track } from './track.ts';

const QUALITY_KEY = 'yetracker:quality';
const VOLUME_KEY = 'yetracker:volume';
const MUTED_KEY = 'yetracker:muted';
const SESSION_KEY = 'yetracker:player';
const SESSION_VERSION = 1;
/** A paused session older than this isn't offered again. */
const SESSION_MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000;

function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string | null): void {
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {
    // Storage full or disabled: preferences simply don't persist.
  }
}

/** A stored volume in (0, 1]; null when missing or invalid (0 is expressed as "muted", never stored). */
export function parseVolume(raw: string | null): number | null {
  if (raw === null || raw.trim() === '') return null;
  const value = Number(raw);
  return Number.isFinite(value) && value > 0 && value <= 1 ? value : null;
}

export function parseQuality(raw: string | null, allowed: readonly string[]): string | null {
  return raw !== null && allowed.includes(raw) ? raw : null;
}

export const preferences = {
  quality: (allowed: readonly string[]) => parseQuality(read(QUALITY_KEY), allowed),
  setQuality: (value: string) => write(QUALITY_KEY, value),
  volume: () => parseVolume(read(VOLUME_KEY)),
  setVolume: (value: number) => {
    if (value > 0 && value <= 1) write(VOLUME_KEY, String(Math.round(value * 100) / 100));
  },
  muted: () => read(MUTED_KEY) === 'true',
  setMuted: (value: boolean) => write(MUTED_KEY, value ? 'true' : null),
};

export interface SavedSession {
  track: Track;
  /** Seconds into the track. */
  position: number;
  queue: Queue;
}

interface StoredSession {
  v: number;
  savedAt: number;
  track: Track;
  position: number;
  queue: SavedQueue;
}

export function parseSession(raw: string | null, now = Date.now()): SavedSession | null {
  if (!raw) return null;
  let data: Partial<StoredSession>;
  try {
    data = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof data !== 'object' || data === null || data.v !== SESSION_VERSION) return null;
  if (typeof data.savedAt !== 'number' || now - data.savedAt > SESSION_MAX_AGE_MS) return null;
  const track = sanitizeTrack(data.track);
  if (!track) return null;
  const position = typeof data.position === 'number' && Number.isFinite(data.position) ? Math.max(0, data.position) : 0;
  return { track, position, queue: Queue.fromJSON(data.queue, track) ?? Queue.single(track) };
}

export const session = {
  load: (): SavedSession | null => parseSession(read(SESSION_KEY)),
  save: (track: Track, position: number, queue: Queue | null) => {
    const stored: StoredSession = {
      v: SESSION_VERSION,
      savedAt: Date.now(),
      track,
      position: Math.round(Math.max(0, position) * 10) / 10,
      queue: (queue ?? Queue.single(track)).toJSON(),
    };
    write(SESSION_KEY, JSON.stringify(stored));
  },
  clear: () => write(SESSION_KEY, null),
};
