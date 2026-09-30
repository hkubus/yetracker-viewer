/** Media Session integration: lock-screen / notification controls, hardware media keys and position state. */
import { SITE_NAME } from './format.ts';
import type { Track } from './track.ts';

export interface MediaSessionActions {
  play(): void;
  pause(): void;
  stop(): void;
  seekBy(seconds: number): void;
  seekTo(seconds: number): void;
  next(): void;
  previous(): void;
}

/** Default jump of the "seek backward/forward" actions when the platform doesn't specify one. */
const DEFAULT_SEEK_OFFSET = 10;

export class MediaSessionBridge {
  private readonly session = 'mediaSession' in navigator ? navigator.mediaSession : null;
  private readonly actions: MediaSessionActions;
  private hasNext: boolean | null = null;
  private hasPrevious: boolean | null = null;

  constructor(actions: MediaSessionActions) {
    this.actions = actions;
    this.bind('play', () => actions.play());
    this.bind('pause', () => actions.pause());
    this.bind('stop', () => actions.stop());
    this.bind('seekbackward', (details) => actions.seekBy(-(details.seekOffset ?? DEFAULT_SEEK_OFFSET)));
    this.bind('seekforward', (details) => actions.seekBy(details.seekOffset ?? DEFAULT_SEEK_OFFSET));
    this.bind('seekto', (details) => {
      if (typeof details.seekTime === 'number') actions.seekTo(details.seekTime);
    });
  }

  private bind(action: MediaSessionAction, handler: MediaSessionActionHandler | null): void {
    try {
      this.session?.setActionHandler(action, handler);
    } catch {
      // The browser doesn't support this action.
    }
  }

  setTrack(track: Track | null, artworkUrl: string | null): void {
    if (!this.session) return;
    if (!track || typeof MediaMetadata === 'undefined') {
      this.session.metadata = null;
      return;
    }
    this.session.metadata = new MediaMetadata({
      title: track.title,
      artist: track.eraName || SITE_NAME,
      album: track.eraName || SITE_NAME,
      artwork: artworkUrl ? [{ src: artworkUrl, sizes: '512x512' }] : [],
    });
  }

  setPlaybackState(state: MediaSessionPlaybackState): void {
    if (this.session && this.session.playbackState !== state) this.session.playbackState = state;
  }

  /** Only called with a known, finite duration; clears the state otherwise. */
  setPosition(position: number, duration: number | null, playbackRate: number): void {
    if (!this.session || typeof this.session.setPositionState !== 'function') return;
    try {
      if (duration === null || !(duration > 0) || !Number.isFinite(duration)) {
        this.session.setPositionState();
        return;
      }
      this.session.setPositionState({
        duration,
        position: Math.min(Math.max(0, position), duration),
        playbackRate: playbackRate > 0 ? playbackRate : 1,
      });
    } catch {
      // Inconsistent values are rejected; the next update corrects them.
    }
  }

  /** Next/previous handlers exist only while there is somewhere to go, so the OS hides the buttons at the ends. */
  setQueueEnds(hasPrevious: boolean, hasNext: boolean): void {
    if (hasNext !== this.hasNext) {
      this.hasNext = hasNext;
      this.bind('nexttrack', hasNext ? () => this.actions.next() : null);
    }
    if (hasPrevious !== this.hasPrevious) {
      this.hasPrevious = hasPrevious;
      this.bind('previoustrack', hasPrevious ? () => this.actions.previous() : null);
    }
  }
}
