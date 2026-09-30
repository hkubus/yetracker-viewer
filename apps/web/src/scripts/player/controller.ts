/**
 * Playback logic of the site-wide audio player. One instance lives for the whole visit: the player element is
 * persisted across client-side navigations and every listener here is registered once.
 *
 * Vocabulary:
 * - a *run* is one selected track, kept across reloads of the same song (retries, quality fallback/switches);
 * - a *load* is one assignment of `audio.src`; starting a new load aborts the previous one's timers and requests,
 *   so a stale failure handler can never act on the new source.
 */
import { coverUrl } from '../../utils/cover.ts';
import { formatDuration } from '../../utils/duration.ts';
import { playingDocumentTitle, qualityLabel } from './format.ts';
import { MediaSessionBridge } from './media-session.ts';
import { anySignal, type Probe, probe, sleep, startStream, waitForStream } from './probe.ts';
import type { Continuation, ExtendResult, Page, Queue } from './queue.ts';
import { preferences, session as savedSession } from './storage.ts';
import {
  addsPlayTargets,
  playTargetFrom,
  snapshotQueue,
  syncPlayTargets,
  trackFromTarget,
  visibleTargetFor,
} from './targets.ts';
import { positiveNumber, type Track } from './track.ts';
import type { PlayerView } from './view.ts';

/** Stream qualities offered: '' is the original file, the others are Ogg Opus transcodes (kbps). */
export const QUALITIES: readonly string[] = ['', '64', '128', '192', '256', '320'];
const DEFAULT_TRANSCODE_QUALITY = '128';

/** A stream gap must last this long before the state reads "Buffering…" (avoids flicker). */
const BUFFERING_DELAY_MS = 500;
/** Buffering this long without progress counts as a failed load. */
const STALL_TIMEOUT_MS = 20_000;
/** How long a quality change waits for the new transcode while the current stream keeps playing. */
const QUALITY_SWITCH_WAIT_MS = 60_000;
/** Pause before the one automatic retry of a failed original file. */
const RETRY_DELAY_MS = 1_000;
const MAX_RETRY_AFTER_S = 30;
/** After the API said transcoding is unavailable, play originals for this long without asking again. */
const TRANSCODE_BLOCK_MS = 5 * 60_000;
/** A transcode ending this much before the file's real length (seconds, or share of the length) was cut off. */
const TRUNCATION_TOLERANCE_S = 10;
const TRUNCATION_TOLERANCE_SHARE = 0.05;
const SAVE_INTERVAL_MS = 5_000;
const MAX_CONSECUTIVE_SKIPS = 5;
/** Tolerance when checking whether a position is inside a buffered/seekable range. */
const SEEK_TOLERANCE_S = 0.25;
const PAGE_FETCH_TIMEOUT_MS = 10_000;
const DURATION_TIMEOUT_MS = 5_000;
/**
 * The ClientRouter announces a new page by reading `document.title` 60 ms after `astro:page-load`; a background tab
 * gets its "▶ song" title back only after that. A navigation that never reaches `astro:page-load` releases the title
 * anyway.
 */
const TITLE_RELEASE_DELAY_MS = 250;
const TITLE_HOLD_LIMIT_MS = 15_000;

const MESSAGES = {
  missing: "This song's audio file isn't available right now.",
  network: "Couldn't load the audio. Check your connection and try again.",
  stalled: 'The audio stopped loading. Check your connection and try again.',
  server: 'The audio server had a problem. Try again in a moment.',
  unplayable: "This audio file couldn't be played.",
  nextPage: "Couldn't load the next songs of this list. Check your connection, then press Next to try again.",
} as const;

type Intent = 'play' | 'pause';
/** How a track was reached. Failures of tracks reached by next/previous/auto-advance are skipped over. */
type Mode = 'user' | 'continuous';
type Direction = 1 | -1;
/** Outcome of moving through the queue (see `advance()`). */
type Advance = 'moved' | 'end' | 'failed' | 'superseded';
type FailureCause = 'network' | 'decode' | 'unsupported' | 'stalled' | 'truncated';
export type PlayerState = 'playing' | 'paused' | 'stopped' | 'error';

interface Run {
  readonly track: Track;
  readonly mode: Mode;
  readonly direction: Direction;
  announced: boolean;
  /** The one automatic retry after a failure has been used. */
  retried: boolean;
  /** The one retry after "transcoder busy" has been used. */
  retriedBusy: boolean;
  /** Quality used for this song instead of the preference ('' after falling back to the original file). */
  qualityOverride: string | null;
  /** The audio file's real length from `/songs/:id/duration` (the catalog's length can be wrong). */
  fileDuration: number | null;
  fileDurationRequest: Promise<number | null> | null;
}

interface Load {
  readonly url: string;
  readonly quality: string;
  readonly abort: AbortController;
  /** Position to seek to once metadata is known. */
  startAt: number;
  /** Last known playback position (while a seek waits: the one from before it), to continue from after a failure. */
  position: number;
  /** Metadata is loaded: from now on pause events are real pauses. */
  ready: boolean;
  /** A failure of this load is being handled. */
  failed: boolean;
  /** Started to reach a position a live transcode couldn't seek to (don't wait for it again). */
  forSeek: boolean;
  /** Playback holds while a live transcode becomes seekable at this position. */
  seekTarget: number | null;
}

export interface PlayerOptions {
  apiBaseUrl: string;
  pageSize: number;
  canPlayOpus: boolean;
  canSetVolume: boolean;
}

export class PlayerController {
  private readonly audio: HTMLAudioElement;
  private readonly media: MediaSessionBridge;
  private run: Run | null = null;
  private load: Load | null = null;
  private queue: Queue | null = null;
  /** Cancels page fetches of the current queue when it is replaced. */
  private queueAbort = new AbortController();
  private intent: Intent = 'pause';
  private errorMessage: string | null = null;
  private buffering = false;
  private bufferTimer = 0;
  private stallTimer = 0;
  /** Position of a restored track that hasn't been loaded yet. */
  private pendingPosition = 0;
  private preferredQuality = '';
  private transcodeBlockedUntil = 0;
  private consecutiveSkips = 0;
  private advancing = false;
  /** Cancels a quality change that waits for its transcode. */
  private qualitySwitch: AbortController | null = null;
  /** Set around our own pause() so the resulting event isn't taken for the user's. */
  private ignoreNextPause = false;
  /** The user is dragging the position slider: don't move it from under them. */
  private scrubbing = false;
  private lastSave = 0;
  private lastEmitted = '';
  /** The page's own title (a background tab shows "▶ song" instead while playing). */
  private pageTitle = document.title;
  /** A navigation is under way: keep the page's title for the router's history entry and page announcement. */
  private titleHeld = false;
  private titleTimer = 0;
  /**
   * `pagehide` fired: the document is being unloaded or put in the back/forward cache. It turns hidden only after
   * that, and a title set then still ends up on the history entry being left, so the page keeps its own title.
   */
  private leavingPage = false;
  private syncScheduled = false;

  private readonly view: PlayerView;
  private readonly options: PlayerOptions;

  constructor(view: PlayerView, options: PlayerOptions) {
    this.view = view;
    this.options = options;
    this.audio = view.audio;
    this.media = new MediaSessionBridge({
      play: () => this.play(),
      pause: () => this.pause(),
      // System "stop" (e.g. from a notification) pauses; only the player's own close button forgets the track.
      stop: () => this.pause(),
      seekBy: (seconds) => this.seekBy(seconds),
      seekTo: (seconds) => this.seekTo(seconds),
      next: () => this.next(),
      previous: () => this.previous(),
    });
  }

  start(): void {
    this.initPreferences();
    this.bindMediaEvents();
    this.bindControls();
    this.bindDocument();
    this.restoreSession();
  }

  /** A track is shown (playing, paused or restored): keyboard shortcuts apply. */
  get active(): boolean {
    return this.run !== null && this.view.isVisible;
  }

  /** Track length in seconds, when known. */
  get trackLength(): number | null {
    return this.run ? this.length() : null;
  }

  /** Playing, or about to (loading/buffering after the user pressed play). */
  get playing(): boolean {
    return this.run !== null && this.errorMessage === null && this.intent === 'play';
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Actions (buttons, keyboard, media keys, row buttons)

  play(): void {
    if (!this.run) return;
    if (this.errorMessage !== null) {
      this.retry();
      return;
    }
    this.intent = 'play';
    if (!this.load) {
      // A restored session: nothing has been loaded yet.
      this.beginLoad({ startAt: this.pendingPosition, play: true });
      return;
    }
    if (this.load.seekTarget === null) this.callPlay();
    this.render();
  }

  pause(): void {
    if (!this.run) return;
    this.intent = 'pause';
    this.clearBuffering();
    if (!this.audio.paused) this.audio.pause();
    this.render();
    this.saveSession(true);
  }

  togglePlay(): void {
    if (this.playing) this.pause();
    else this.play();
  }

  next(): void {
    void this.advance(1);
  }

  previous(): void {
    void this.advance(-1);
  }

  retry(): void {
    const run = this.run;
    if (!run) return;
    const position = this.load ? (this.load.seekTarget ?? this.load.position) : this.pendingPosition;
    this.errorMessage = null;
    this.view.clearError();
    run.retried = false;
    run.retriedBusy = false;
    this.intent = 'play';
    this.beginLoad({ startAt: position, play: true });
  }

  seekBy(seconds: number): void {
    this.seekTo(this.position() + seconds);
  }

  seekTo(seconds: number): void {
    if (!this.run || !Number.isFinite(seconds)) return;
    const length = this.length();
    const target = Math.max(0, length !== null ? Math.min(seconds, length) : seconds);
    const load = this.load;
    if (!load) {
      this.pendingPosition = target;
      this.updateTime(target);
      this.saveSession(true);
      return;
    }
    if (this.audio.readyState === HTMLMediaElement.HAVE_NOTHING) {
      // Metadata is still loading: seek as soon as it is there.
      load.startAt = target;
      load.position = target;
      this.updateTime(target);
      return;
    }
    if (this.canSeekTo(target)) {
      const wasHolding = load.seekTarget !== null;
      load.seekTarget = null;
      load.position = target;
      this.audio.currentTime = target;
      if (wasHolding && this.intent === 'play') this.callPlay();
      this.render();
      return;
    }
    // Out of reach for now: `load.position` keeps the position from before the seek.
    if (this.isLiveStream() && !load.forSeek) void this.seekWhenCached(load, target);
    else this.seekAsFarAsPossible(target);
  }

  setQuality(value: string): void {
    if (!QUALITIES.includes(value)) return;
    this.preferredQuality = value;
    preferences.setQuality(value);
    const run = this.run;
    if (!run) return;
    // An explicit choice undoes this song's fallback and asks the transcoder again.
    run.qualityOverride = null;
    this.transcodeBlockedUntil = 0;
    this.qualitySwitch?.abort();
    this.qualitySwitch = null;
    const load = this.load;
    if (!load) return;
    const quality = this.effectiveQuality();
    if (this.errorMessage === null && load.ready && load.seekTarget === null) {
      if (quality === load.quality) return;
      if (quality) {
        // A transcode that isn't finished yet can't start in the middle: the current stream keeps playing until it
        // can.
        void this.switchQualityWhenReady(run, load, quality);
        return;
      }
    }
    this.errorMessage = null;
    this.view.clearError();
    this.beginLoad({ startAt: this.position(), play: this.intent === 'play', quality });
  }

  setVolumePercent(percent: number): void {
    const volume = Math.min(1, Math.max(0, percent / 100));
    if (volume === 0) {
      this.audio.muted = true;
    } else {
      this.audio.volume = volume;
      this.audio.muted = false;
      preferences.setVolume(volume);
    }
    preferences.setMuted(this.audio.muted);
  }

  toggleMute(): void {
    const mute = !(this.audio.muted || this.audio.volume === 0);
    this.audio.muted = mute;
    if (!mute && this.audio.volume === 0) this.audio.volume = preferences.volume() ?? 1;
    preferences.setMuted(mute);
  }

  /** Stops playback, forgets the track and hides the bar. */
  close(): void {
    const run = this.run;
    if (!run) return;
    const focusTarget = this.view.bar.contains(document.activeElement) ? visibleTargetFor(run.track.id) : null;
    this.load?.abort.abort();
    this.load = null;
    this.replaceQueue(null);
    this.run = null;
    this.intent = 'pause';
    this.errorMessage = null;
    this.pendingPosition = 0;
    this.clearBuffering();
    this.audio.pause();
    // Drops the buffered audio and any open request.
    this.audio.removeAttribute('src');
    this.audio.load();
    savedSession.clear();
    this.view.hide();
    syncPlayTargets(null, false);
    this.media.setTrack(null, null);
    this.media.setPlaybackState('none');
    this.media.setPosition(0, null, 1);
    this.media.setQueueEnds(false, false);
    this.updateDocumentTitle();
    this.dispatchState(run.track.id, 'stopped');
    focusTarget?.focus();
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Tracks and loads

  private startTrack(track: Track, queue: Queue, options: { play: boolean; mode: Mode; direction: Direction }): void {
    const previous = this.run;
    if (previous && previous.track.id !== track.id) this.dispatchState(previous.track.id, 'stopped');
    if (queue !== this.queue) this.replaceQueue(queue);
    this.run = newRun(track, options.mode, options.direction);
    this.errorMessage = null;
    this.view.clearError();
    this.view.showTrack(track, this.options.apiBaseUrl, this.options.pageSize);
    this.view.show();
    this.view.setQuality(this.effectiveQuality());
    this.media.setTrack(track, this.artworkUrl(track));
    this.intent = options.play ? 'play' : 'pause';
    this.beginLoad({ startAt: 0, play: options.play });
    this.saveSession(true);
  }

  /** Points the audio element at the current track. Any previous load's timers and requests are cancelled. */
  private beginLoad(options: { startAt: number; play: boolean; quality?: string; forSeek?: boolean }): void {
    const run = this.run;
    if (!run) return;
    this.load?.abort.abort();
    this.clearBuffering();
    const quality = options.quality ?? this.effectiveQuality();
    const startAt = Math.max(0, options.startAt);
    const load: Load = {
      url: this.streamUrl(run.track.id, quality),
      quality,
      abort: new AbortController(),
      startAt,
      position: startAt,
      ready: false,
      failed: false,
      forSeek: options.forSeek ?? false,
      seekTarget: null,
    };
    this.load = load;
    this.pendingPosition = 0;
    this.ignoreNextPause = false;
    this.view.setQuality(quality);
    this.audio.src = load.url;
    this.updateTime(startAt);
    // play() right away, inside the user's gesture where there is one (iOS requires it); a start position is
    // applied on loadedmetadata, before any audio is output.
    if (options.play) this.callPlay();
    this.render();
  }

  private callPlay(): void {
    const load = this.load;
    const attempt = this.audio.play();
    attempt?.catch((error: unknown) => {
      // AbortError: a pause() or a new source interrupted the request, which is expected.
      // NotSupportedError: the source failed; the media element's `error` event handles that.
      if (!(error instanceof DOMException) || error.name !== 'NotAllowedError' || load !== this.load) return;
      this.intent = 'pause';
      this.clearBuffering();
      this.view.showNotice('Your browser blocked playback. Press play to start.');
      this.render();
    });
  }

  /**
   * Moves through the queue; fetches the next page of the list first when needed. Resolves `moved`, `end` (no
   * song in that direction), `failed` (the next page couldn't be loaded: Next stays available to retry) or
   * `superseded` (another move or a new queue took over).
   */
  private async advance(direction: Direction): Promise<Advance> {
    const queue = this.queue;
    if (!queue || this.advancing) return 'superseded';
    if (queue.needsExtension(direction)) {
      this.advancing = true;
      this.buffering = this.intent === 'play';
      this.render();
      let result: ExtendResult;
      try {
        result = await queue.extend(this.loadPage, this.queueAbort.signal);
      } finally {
        this.advancing = false;
      }
      if (queue !== this.queue) return 'superseded';
      this.buffering = false;
      if (result === 'failed') {
        this.view.showNotice(MESSAGES.nextPage);
        this.render();
        return 'failed';
      }
    }
    const index = queue.index + direction;
    const track = queue.tracks[index];
    if (!track) {
      this.render();
      return 'end';
    }
    queue.index = index;
    this.startTrack(track, queue, { play: true, mode: 'continuous', direction });
    return 'moved';
  }

  private readonly loadPage = async (continuation: Continuation, limit: number, signal: AbortSignal): Promise<Page> => {
    const params = new URLSearchParams(continuation.params);
    params.set('limit', String(limit));
    params.set('offset', String(continuation.offset));
    const response = await fetch(`${this.options.apiBaseUrl}/eras/${continuation.era.eraId}/songs?${params}`, {
      headers: { Accept: 'application/json' },
      signal: anySignal([signal, AbortSignal.timeout(PAGE_FETCH_TIMEOUT_MS)]),
    });
    if (!response.ok) throw new Error(`Song list request failed with status ${response.status}`);
    const body: unknown = await response.json();
    const totalHeader = response.headers.get('x-total-count');
    const total = totalHeader === null ? Number.NaN : Number(totalHeader);
    return { songs: Array.isArray(body) ? body : [], total: Number.isSafeInteger(total) ? total : null };
  };

  private replaceQueue(queue: Queue | null): void {
    this.queueAbort.abort();
    this.queueAbort = new AbortController();
    this.queue = queue;
  }

  private effectiveQuality(): string {
    if (this.run?.qualityOverride != null) return this.run.qualityOverride;
    if (!this.options.canPlayOpus || Date.now() < this.transcodeBlockedUntil) return '';
    return this.preferredQuality;
  }

  private streamUrl(songId: number, quality: string): string {
    const url = `${this.options.apiBaseUrl}/songs/${songId}/stream`;
    return quality ? `${url}?quality=${quality}` : url;
  }

  private artworkUrl(track: Track): string | null {
    return track.hasCover && track.eraId !== null
      ? coverUrl(this.options.apiBaseUrl, track.eraId, track.coverVersion)
      : null;
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Seeking

  private position(): number {
    const load = this.load;
    if (!load) return this.pendingPosition;
    if (load.seekTarget !== null) return load.seekTarget;
    if (this.audio.readyState === HTMLMediaElement.HAVE_NOTHING) return load.startAt || load.position;
    return this.audio.currentTime;
  }

  /** Length in seconds: the stream's own duration when known, else the list's, else the file's from the API. */
  private length(): number | null {
    const duration = this.audio.duration;
    if (this.load && Number.isFinite(duration) && duration > 0) return duration;
    return this.run?.track.length ?? this.run?.fileDuration ?? null;
  }

  /** A transcode streamed while it is being produced: no known length, only the buffered part is seekable. */
  private isLiveStream(): boolean {
    return this.audio.duration === Number.POSITIVE_INFINITY;
  }

  private canSeekTo(target: number): boolean {
    // Chrome reports [0, ∞) as seekable for live streams, but can only seek within what it has buffered.
    const ranges = this.isLiveStream() ? this.audio.buffered : this.audio.seekable;
    for (let index = 0; index < ranges.length; index += 1) {
      if (target >= ranges.start(index) - SEEK_TOLERANCE_S && target <= ranges.end(index) + SEEK_TOLERANCE_S) {
        return true;
      }
    }
    return false;
  }

  /**
   * The server finishes transcodes at full speed and then serves them from its cache with range support, so a live
   * transcode that can't reach `target` yet usually can soon. Playback holds while the stream loads up to the target
   * or the cache fills (then the finished file is loaded at the target). If neither happens in time, playback goes
   * on from where it was, or from as far toward the target as the stream has loaded; it never starts over.
   */
  private async seekWhenCached(load: Load, target: number): Promise<void> {
    const alreadyWaiting = load.seekTarget !== null;
    load.seekTarget = target;
    this.updateTime(target);
    if (alreadyWaiting) return;
    if (!this.audio.paused) {
      this.ignoreNextPause = true;
      this.audio.pause();
    }
    this.render();
    const outcome = await waitForStream(load.url, load.abort.signal, () =>
      load.seekTarget === null ? true : this.canSeekTo(load.seekTarget),
    );
    const wanted = load.seekTarget;
    if (load !== this.load || wanted === null) return;
    if (outcome === 'cached') {
      this.beginLoad({ startAt: wanted, play: this.intent === 'play', quality: load.quality, forSeek: true });
      return;
    }
    load.seekTarget = null;
    if (outcome === 'reached') {
      load.position = wanted;
      this.audio.currentTime = wanted;
      if (this.intent === 'play') this.callPlay();
      this.updateTime();
      this.render();
      return;
    }
    this.seekAsFarAsPossible(wanted);
  }

  /** Switches the current song to a transcode once the server has finished it, keeping the position. */
  private async switchQualityWhenReady(run: Run, load: Load, quality: string): Promise<void> {
    const abort = new AbortController();
    this.qualitySwitch = abort;
    const signal = anySignal([abort.signal, load.abort.signal]);
    const url = this.streamUrl(run.track.id, quality);
    // Asking for the stream makes the server transcode the whole song; the answer itself isn't needed.
    const started = await startStream(url, signal);
    let ready = started.kind === 'ok' && started.cached;
    if (!ready && started.kind === 'ok') {
      ready = (await waitForStream(url, signal, undefined, QUALITY_SWITCH_WAIT_MS)) === 'cached';
    }
    if (signal.aborted || load !== this.load || this.run !== run) return;
    this.qualitySwitch = null;
    if (ready) {
      this.beginLoad({ startAt: this.position(), play: this.intent === 'play', quality });
      return;
    }
    // This song stays on its current stream; the new quality applies from the next song on.
    if (started.kind === 'unavailable') this.transcodeBlockedUntil = Date.now() + TRANSCODE_BLOCK_MS;
    run.qualityOverride = load.quality;
    this.view.setQuality(load.quality);
    const current = load.quality ? `at ${qualityLabel(load.quality)}` : 'the original file';
    const reason = started.kind === 'ok' ? "isn't ready for this song yet" : "isn't available right now";
    this.view.showNotice(`${qualityLabel(quality)} ${reason}, so this song keeps playing ${current}.`);
  }

  private seekAsFarAsPossible(target: number): void {
    const ranges = this.isLiveStream() ? this.audio.buffered : this.audio.seekable;
    const limit = ranges.length > 0 ? ranges.end(ranges.length - 1) : 0;
    const reachable = Math.max(0, Math.min(target, limit - 1));
    const current = this.audio.currentTime;
    if (this.load) this.load.seekTarget = null;
    if (target > current && reachable > current + 1) {
      this.audio.currentTime = reachable;
      this.view.showNotice(`Jumped to ${formatDuration(reachable)}: the rest of this stream hasn't loaded yet.`);
    } else {
      this.view.showNotice("This stream can't jump to that point yet. Try again in a moment.");
    }
    if (this.load) this.load.position = this.audio.currentTime;
    if (this.intent === 'play' && this.audio.paused) this.callPlay();
    this.updateTime();
    this.render();
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Failures

  private async recover(load: Load, cause: FailureCause): Promise<void> {
    const run = this.run;
    if (!run || load.failed) return;
    load.failed = true;
    this.clearBuffering();
    const position = load.seekTarget ?? load.position;
    // Keep reading "Buffering…" (not an error) while the failure is diagnosed.
    this.buffering = this.intent === 'play';
    this.render();
    const result = await probe(load.url, load.abort.signal);
    if (load !== this.load) return;

    if (result.kind === 'missing') {
      this.giveUp(run, MESSAGES.missing);
      return;
    }
    if (load.quality) {
      if (result.kind === 'busy' && !run.retriedBusy) {
        run.retriedBusy = true;
        const seconds = Math.min(Math.max(result.retryAfter, 1), MAX_RETRY_AFTER_S);
        this.view.showNotice(`The ${qualityLabel(load.quality)} stream is busy. Trying again in ${seconds} s…`);
        await sleep(seconds * 1000, load.abort.signal);
        if (load !== this.load) return;
        this.beginLoad({ startAt: position, play: this.intent === 'play', quality: load.quality });
        return;
      }
      // Transcode failed: play the original file for this song (the preference itself is kept).
      if (result.kind === 'unavailable') this.transcodeBlockedUntil = Date.now() + TRANSCODE_BLOCK_MS;
      run.qualityOverride = '';
      this.view.showNotice(
        cause === 'truncated'
          ? `The ${qualityLabel(load.quality)} stream stopped early, so the original file is playing.`
          : `${qualityLabel(load.quality)} isn't available right now, so the original file is playing.`,
      );
      this.beginLoad({ startAt: position, play: this.intent === 'play', quality: '' });
      return;
    }
    if (!run.retried) {
      run.retried = true;
      await sleep(RETRY_DELAY_MS, load.abort.signal);
      if (load !== this.load) return;
      this.beginLoad({ startAt: position, play: this.intent === 'play', quality: '' });
      return;
    }
    this.giveUp(run, failureMessage(result, cause));
  }

  /** Continuous playback skips a song that can't be loaded; otherwise the error is shown with a Retry button. */
  private giveUp(run: Run, message: string): void {
    this.buffering = false;
    const queue = this.queue;
    const canSkip =
      run.mode === 'continuous' &&
      this.intent === 'play' &&
      this.consecutiveSkips < MAX_CONSECUTIVE_SKIPS &&
      queue !== null &&
      (run.direction === 1 ? queue.hasNext() : queue.hasPrevious());
    if (canSkip) {
      this.consecutiveSkips += 1;
      this.view.showNotice(`Skipped “${run.track.title}”: ${message}`);
      void this.advance(run.direction).then((result) => {
        if (result !== 'moved' && this.run === run) this.showFailure(message);
      });
      return;
    }
    this.showFailure(message);
  }

  private showFailure(message: string): void {
    this.errorMessage = message;
    this.intent = 'pause';
    this.clearBuffering();
    if (!this.audio.paused) {
      this.ignoreNextPause = true;
      this.audio.pause();
    }
    this.view.showError(message);
    this.render();
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Media element events

  private bindMediaEvents(): void {
    const audio = this.audio;
    audio.addEventListener('loadedmetadata', () => this.onMetadata());
    audio.addEventListener('durationchange', () => this.updateTime());
    audio.addEventListener('playing', () => this.onPlaying());
    audio.addEventListener('pause', () => this.onPause());
    audio.addEventListener('waiting', () => this.onWaiting());
    audio.addEventListener('timeupdate', () => this.onTimeUpdate());
    audio.addEventListener('seeked', () => {
      this.updateTime();
      this.syncPositionState();
      this.saveSession(true);
    });
    audio.addEventListener('ratechange', () => this.syncPositionState());
    audio.addEventListener('ended', () => this.onEnded());
    audio.addEventListener('error', () => this.onError());
    audio.addEventListener('volumechange', () => this.view.setVolume(audio.volume, audio.muted));
  }

  private onMetadata(): void {
    const load = this.load;
    if (!load) return;
    load.ready = true;
    this.requestLengthIfUnknown();
    this.updateTime();
    this.syncPositionState();
    if (load.startAt > 0) {
      const target = load.startAt;
      load.startAt = 0;
      this.seekTo(target);
    }
  }

  private onPlaying(): void {
    const run = this.run;
    if (!run || !this.load) return;
    this.clearBuffering();
    this.consecutiveSkips = 0;
    if (!run.announced) {
      run.announced = true;
      this.view.announce(`Now playing: ${run.track.title}`);
    }
    this.syncPositionState();
    this.render();
  }

  private onPause(): void {
    if (this.ignoreNextPause) {
      this.ignoreNextPause = false;
      return;
    }
    const load = this.load;
    // Pauses while a source loads, fails or ends come from the element itself; settled pauses are the user's
    // (or the system's, e.g. headphones unplugged).
    if (!load?.ready || load.failed || this.audio.error || this.audio.ended) return;
    this.intent = 'pause';
    this.clearBuffering();
    this.render();
    this.syncPositionState();
    this.saveSession(true);
  }

  private onWaiting(): void {
    if (!this.load || this.intent !== 'play' || this.bufferTimer !== 0 || this.buffering) return;
    this.bufferTimer = window.setTimeout(() => {
      this.bufferTimer = 0;
      this.buffering = true;
      this.render();
    }, BUFFERING_DELAY_MS);
    const load = this.load;
    window.clearTimeout(this.stallTimer);
    this.stallTimer = window.setTimeout(() => {
      if (load === this.load && this.intent === 'play' && (this.buffering || this.bufferTimer !== 0)) {
        void this.recover(load, 'stalled');
      }
    }, STALL_TIMEOUT_MS);
  }

  private clearBuffering(): void {
    window.clearTimeout(this.bufferTimer);
    window.clearTimeout(this.stallTimer);
    this.bufferTimer = 0;
    this.stallTimer = 0;
    this.buffering = false;
  }

  private onTimeUpdate(): void {
    const load = this.load;
    if (!load || load.seekTarget !== null) return;
    const position = this.audio.currentTime;
    if (load.ready && Number.isFinite(position)) load.position = position;
    this.updateTime(position);
    this.saveSession();
  }

  private onEnded(): void {
    const run = this.run;
    const load = this.load;
    if (!run || !load) return;
    const endedAt = this.audio.currentTime;
    // A transcode that ends well before the song's length may have been cut off (the transcoder failed mid-way).
    // The list's length can come from the catalog, which may be wrong: only the file's real length decides.
    if (load.quality && endsEarly(endedAt, run.fileDuration ?? run.track.length)) {
      void this.fileDuration(run).then((duration) => {
        if (load !== this.load) return;
        if (endsEarly(endedAt, duration)) {
          // Continue from here with the original file.
          load.position = endedAt;
          void this.recover(load, 'truncated');
        } else {
          this.finishTrack(run);
        }
      });
      return;
    }
    this.finishTrack(run);
  }

  /** Moves on after a song ended. Without a next song (end of the list, or its next page failed) it stays put. */
  private finishTrack(run: Run): void {
    void this.advance(1).then((result) => {
      if (result === 'moved' || result === 'superseded' || this.run !== run) return;
      this.intent = 'pause';
      this.render();
      // At the end of the list the song is ready to play again from the start.
      this.saveSession(true, result === 'end' ? 0 : undefined);
    });
  }

  private onError(): void {
    const load = this.load;
    const error = this.audio.error;
    if (!load || !error || load.failed || error.code === MediaError.MEDIA_ERR_ABORTED) return;
    const cause: FailureCause =
      error.code === MediaError.MEDIA_ERR_NETWORK
        ? 'network'
        : error.code === MediaError.MEDIA_ERR_DECODE
          ? 'decode'
          : 'unsupported';
    void this.recover(load, cause);
  }

  private requestLengthIfUnknown(): void {
    const run = this.run;
    if (!run || this.length() !== null) return;
    void this.fileDuration(run).then((seconds) => {
      if (this.run !== run || seconds === null) return;
      this.updateTime();
      this.syncPositionState();
    });
  }

  /** The audio file's real length in seconds as the API measured it (asked once per track); null when unknown. */
  private fileDuration(run: Run): Promise<number | null> {
    run.fileDurationRequest ??= fetch(`${this.options.apiBaseUrl}/songs/${run.track.id}/duration`, {
      headers: { Accept: 'application/json' },
      signal: AbortSignal.timeout(DURATION_TIMEOUT_MS),
    })
      .then((response) => (response.ok ? response.json() : null))
      .then((body: unknown) => positiveNumber((body as { duration?: unknown } | null)?.duration))
      .catch(() => null)
      .then((seconds) => {
        run.fileDuration = seconds;
        return seconds;
      });
    return run.fileDurationRequest;
  }

  // ---------------------------------------------------------------------------------------------------------------
  // UI wiring

  private initPreferences(): void {
    const allowed = this.options.canPlayOpus ? QUALITIES : [''];
    this.preferredQuality = preferences.quality(allowed) ?? (this.options.canPlayOpus ? DEFAULT_TRANSCODE_QUALITY : '');
    this.view.setQuality(this.preferredQuality);
    this.view.setQualityAvailable(this.options.canPlayOpus);
    if (this.options.canSetVolume) this.audio.volume = preferences.volume() ?? 1;
    this.audio.muted = preferences.muted();
    this.view.setVolumeSliderAvailable(this.options.canSetVolume);
    this.view.setVolume(this.audio.volume, this.audio.muted);
  }

  private bindControls(): void {
    const view = this.view;
    view.toggle.addEventListener('click', () => this.togglePlay());
    view.previous.addEventListener('click', () => {
      if (view.previous.getAttribute('aria-disabled') !== 'true') this.previous();
    });
    view.next.addEventListener('click', () => {
      if (view.next.getAttribute('aria-disabled') !== 'true') this.next();
    });
    view.close.addEventListener('click', () => this.close());
    view.retry.addEventListener('click', () => this.retry());
    view.mute.addEventListener('click', () => this.toggleMute());
    view.volume.addEventListener('input', () => this.setVolumePercent(Number(view.volume.value)));
    view.quality.addEventListener('change', () => this.setQuality(view.quality.value));

    const seek = view.seek;
    seek.addEventListener('pointerdown', () => {
      this.scrubbing = true;
    });
    const stopScrubbing = () => {
      this.scrubbing = false;
    };
    window.addEventListener('pointerup', stopScrubbing);
    window.addEventListener('pointercancel', stopScrubbing);
    // While dragging only the time preview follows; the seek happens on release (change).
    seek.addEventListener('input', () => this.view.setTime(Number(seek.value), this.length(), false));
    seek.addEventListener('change', () => {
      this.scrubbing = false;
      this.seekTo(Number(seek.value));
    });
  }

  private bindDocument(): void {
    document.addEventListener('click', (event) => this.onTargetClick(event));
    // ClientRouter navigations swap the page (and its <title>) under the persistent player. The router stores the
    // current title in the history entry it leaves and announces the new page by its title, so even a background
    // tab shows the page's own title from the start of a navigation until the new page has been announced.
    document.addEventListener('astro:before-preparation', () => this.holdPageTitle(TITLE_HOLD_LIMIT_MS));
    document.addEventListener('astro:after-swap', () => {
      this.pageTitle = document.title;
      this.view.publishHeight();
    });
    document.addEventListener('astro:page-load', () => {
      if (this.titleHeld) this.holdPageTitle(TITLE_RELEASE_DELAY_MS);
      this.scheduleTargetSync();
    });
    // Lists rendered on the client (search results) get the current song's state too.
    new MutationObserver((mutations) => {
      if (addsPlayTargets(mutations, this.view.bar)) this.scheduleTargetSync();
    }).observe(document.documentElement, { childList: true, subtree: true });
    window.addEventListener('pagehide', () => {
      this.leavingPage = true;
      this.updateDocumentTitle();
      this.saveSession(true);
    });
    window.addEventListener('pageshow', () => {
      this.leavingPage = false;
    });
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'hidden') this.saveSession(true);
      this.updateDocumentTitle();
    });
  }

  private onTargetClick(event: MouseEvent): void {
    const button = playTargetFrom(event.target);
    if (!button || event.defaultPrevented) return;
    if ((button instanceof HTMLButtonElement && button.disabled) || button.getAttribute('aria-disabled') === 'true') {
      return;
    }
    const track = trackFromTarget(button);
    if (!track) return;
    event.preventDefault();
    const queue = snapshotQueue(button, track);
    if (this.run?.track.id === track.id) {
      // The current song's button toggles pause; the list it was clicked in becomes the queue.
      this.replaceQueue(queue);
      this.togglePlay();
      return;
    }
    this.consecutiveSkips = 0;
    this.startTrack(track, queue, { play: true, mode: 'user', direction: 1 });
  }

  private restoreSession(): void {
    const saved = savedSession.load();
    if (!saved) return;
    const { track, queue } = saved;
    this.replaceQueue(queue);
    this.run = newRun(track, 'user', 1);
    this.intent = 'pause';
    const length = track.length;
    this.pendingPosition = length !== null && saved.position >= length - 1 ? 0 : saved.position;
    this.view.showTrack(track, this.options.apiBaseUrl, this.options.pageSize);
    this.view.show();
    this.view.setQuality(this.effectiveQuality());
    this.media.setTrack(track, this.artworkUrl(track));
    this.updateTime(this.pendingPosition);
    this.render();
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Rendering and state propagation

  private render(): void {
    const run = this.run;
    if (!run) return;
    const holding = this.load?.seekTarget != null;
    const label =
      this.errorMessage !== null
        ? 'Error'
        : this.intent === 'play' && (this.buffering || holding)
          ? 'Buffering…'
          : this.intent === 'play'
            ? 'Playing'
            : 'Paused';
    this.view.setState(label, label === 'Buffering…');
    this.view.setToggle(this.playing);
    const hasPrevious = this.queue?.hasPrevious() ?? false;
    const hasNext = this.queue?.hasNext() ?? false;
    this.view.setSkips(hasPrevious, hasNext);
    this.media.setQueueEnds(hasPrevious, hasNext);
    this.media.setPlaybackState(this.playing ? 'playing' : 'paused');
    syncPlayTargets(run.track.id, this.playing);
    this.updateDocumentTitle();
    this.dispatchState(run.track.id, this.errorMessage !== null ? 'error' : this.playing ? 'playing' : 'paused');
  }

  private updateTime(position = this.position()): void {
    this.view.setTime(position, this.length(), !this.scrubbing);
  }

  private syncPositionState(): void {
    if (this.run) this.media.setPosition(this.position(), this.length(), this.audio.playbackRate);
  }

  /**
   * The document shows "▶ song" only while the tab is hidden: the tab strip is where it helps, and the system's media
   * controls get the song from the Media Session. A visible page keeps its own title because the browser files the
   * current title under the history entry being left (on Back/Forward before any script runs), and the router
   * announces a new page by its title.
   */
  private updateDocumentTitle(): void {
    const run = this.run;
    const title =
      run && this.playing && !this.titleHeld && !this.leavingPage && document.visibilityState === 'hidden'
        ? playingDocumentTitle(run.track.title)
        : this.pageTitle;
    if (document.title !== title) document.title = title;
  }

  /** Shows the page's own title for `ms`, then the playing title again (when a song plays in a hidden tab). */
  private holdPageTitle(ms: number): void {
    this.titleHeld = true;
    this.updateDocumentTitle();
    window.clearTimeout(this.titleTimer);
    this.titleTimer = window.setTimeout(() => {
      this.titleHeld = false;
      this.updateDocumentTitle();
    }, ms);
  }

  private scheduleTargetSync(): void {
    if (this.syncScheduled) return;
    this.syncScheduled = true;
    requestAnimationFrame(() => {
      this.syncScheduled = false;
      syncPlayTargets(this.run?.track.id ?? null, this.playing);
    });
  }

  private dispatchState(songId: number, state: PlayerState): void {
    const key = `${songId}:${state}`;
    if (key === this.lastEmitted) return;
    this.lastEmitted = key;
    document.dispatchEvent(new CustomEvent('yt:player-state', { detail: { songId, state } }));
  }

  private saveSession(force = false, position?: number): void {
    const run = this.run;
    if (!run) return;
    const now = Date.now();
    if (!force && now - this.lastSave < SAVE_INTERVAL_MS) return;
    this.lastSave = now;
    savedSession.save(run.track, position ?? this.position(), this.queue);
  }
}

function newRun(track: Track, mode: Mode, direction: Direction): Run {
  return {
    track,
    mode,
    direction,
    announced: false,
    retried: false,
    retriedBusy: false,
    qualityOverride: null,
    fileDuration: null,
    fileDurationRequest: null,
  };
}

/** Whether playback that stopped at `position` ended clearly before a song of `length` seconds would. */
function endsEarly(position: number, length: number | null): boolean {
  if (length === null || !Number.isFinite(position)) return false;
  return position < length - Math.max(TRUNCATION_TOLERANCE_S, length * TRUNCATION_TOLERANCE_SHARE);
}

function failureMessage(result: Probe, cause: FailureCause): string {
  switch (result.kind) {
    case 'missing':
      return MESSAGES.missing;
    case 'network':
      return MESSAGES.network;
    case 'ok':
      if (cause === 'stalled') return MESSAGES.stalled;
      return cause === 'network' ? MESSAGES.network : MESSAGES.unplayable;
    default:
      return MESSAGES.server;
  }
}
