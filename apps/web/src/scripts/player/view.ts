/**
 * The player bar's DOM: element lookups and rendering. It holds no playback logic; the controller decides what to
 * show and calls these methods.
 */
import { themeFor, themeVariables } from '../../utils/color.ts';
import { coverUrl, eraInitials } from '../../utils/cover.ts';
import { formatDuration } from '../../utils/duration.ts';
import { positionText, volumeText } from './format.ts';
import { DEFAULT_PAGE_SIZE, eraHref, type Track } from './track.ts';

const NOTICE_MS = 6_000;
const ANNOUNCEMENT_MS = 7_000;

function find<T extends Element>(root: ParentNode, selector: string, type: { new (): T; prototype: T }): T {
  const element = root.querySelector(selector);
  if (!(element instanceof type)) throw new Error(`Player markup is missing ${selector}`);
  return element;
}

export class PlayerView {
  /** The bar: a `<section>` landmark, hidden while there is no track. */
  readonly bar: HTMLElement;
  readonly audio: HTMLAudioElement;
  readonly close: HTMLButtonElement;
  readonly previous: HTMLButtonElement;
  readonly toggle: HTMLButtonElement;
  readonly next: HTMLButtonElement;
  readonly mute: HTMLButtonElement;
  readonly volume: HTMLInputElement;
  readonly quality: HTMLSelectElement;
  readonly seek: HTMLInputElement;
  readonly retry: HTMLButtonElement;
  private readonly cover: HTMLElement;
  private readonly initials: HTMLElement;
  private readonly state: HTMLElement;
  private readonly controls: HTMLElement;
  private readonly title: HTMLElement;
  private readonly era: HTMLAnchorElement;
  private readonly volumeGroup: HTMLElement;
  private readonly qualityControl: HTMLElement;
  private readonly elapsed: HTMLElement;
  private readonly duration: HTMLElement;
  private readonly error: HTMLElement;
  private readonly errorText: HTMLElement;
  private readonly notice: HTMLElement;
  private readonly announcer: HTMLElement;
  private readonly alertRegion: HTMLElement;
  private noticeTimer = 0;
  private resizeObserver: ResizeObserver | null = null;
  private shownTime = '';
  private shownDuration = '';

  /** `root` is the player bar itself (the element persisted across navigations). */
  constructor(root: HTMLElement) {
    this.bar = root;
    this.audio = find(root, '[data-player-audio]', HTMLAudioElement);
    this.close = find(root, '[data-player-close]', HTMLButtonElement);
    this.previous = find(root, '[data-player-previous]', HTMLButtonElement);
    this.toggle = find(root, '[data-player-toggle]', HTMLButtonElement);
    this.next = find(root, '[data-player-next]', HTMLButtonElement);
    this.mute = find(root, '[data-player-mute]', HTMLButtonElement);
    this.volume = find(root, '[data-player-volume]', HTMLInputElement);
    this.quality = find(root, '[data-player-quality]', HTMLSelectElement);
    this.seek = find(root, '[data-player-seek]', HTMLInputElement);
    this.retry = find(root, '[data-player-retry]', HTMLButtonElement);
    this.cover = find(root, '[data-player-cover]', HTMLElement);
    this.initials = find(this.cover, '[data-cover-initials]', HTMLElement);
    this.state = find(root, '[data-player-state]', HTMLElement);
    this.controls = find(root, '[data-player-controls]', HTMLElement);
    this.title = find(root, '[data-player-title]', HTMLElement);
    this.era = find(root, '[data-player-era]', HTMLAnchorElement);
    this.volumeGroup = find(root, '[data-player-volume-group]', HTMLElement);
    this.qualityControl = find(root, '[data-player-quality-control]', HTMLElement);
    this.elapsed = find(root, '[data-player-elapsed]', HTMLElement);
    this.duration = find(root, '[data-player-duration]', HTMLElement);
    this.error = find(root, '[data-player-error]', HTMLElement);
    this.errorText = find(root, '[data-player-error-text]', HTMLElement);
    this.notice = find(root, '[data-player-notice]', HTMLElement);
    this.announcer = find(root, '[data-player-announcer]', HTMLElement);
    this.alertRegion = find(root, '[data-player-alert]', HTMLElement);
  }

  get isVisible(): boolean {
    return !this.bar.hidden;
  }

  show(): void {
    if (!this.bar.hidden) return;
    this.bar.hidden = false;
    // Pages reserve room for the bar with --player-height (its height varies with the layout and messages).
    this.resizeObserver ??= new ResizeObserver(() => this.publishHeight());
    this.resizeObserver.observe(this.bar);
    this.publishHeight();
  }

  hide(): void {
    this.bar.hidden = true;
    this.resizeObserver?.disconnect();
    document.documentElement.style.removeProperty('--player-height');
    this.clearNotice();
    this.clearError();
  }

  /** Also called after ClientRouter swaps, which replace every attribute of <html>, inline style included. */
  publishHeight(): void {
    if (this.bar.hidden) return;
    const rect = this.bar.getBoundingClientRect();
    const height = Math.ceil(Math.max(0, window.innerHeight - rect.top));
    document.documentElement.style.setProperty('--player-height', `${height}px`);
  }

  showTrack(track: Track, apiBaseUrl: string, pageSize = DEFAULT_PAGE_SIZE): void {
    const theme = themeFor(track.color);
    for (const [name, value] of Object.entries(themeVariables(theme, '--player'))) {
      this.bar.style.setProperty(name, value);
    }
    this.title.textContent = track.title;
    // Long titles are truncated with an ellipsis; the tooltip shows the whole title.
    this.title.title = track.title;
    const href = eraHref(track, pageSize);
    if (href && track.eraName) {
      this.era.href = href;
      this.era.textContent = track.eraName;
      this.era.hidden = false;
    } else {
      this.era.hidden = true;
      this.era.removeAttribute('href');
      this.era.textContent = '';
    }
    this.showCover(track, apiBaseUrl);
  }

  /** Same updates as the CoverImage component: themed tile with initials, the cover on top when there is one. */
  private showCover(track: Track, apiBaseUrl: string): void {
    const theme = themeFor(track.color);
    this.cover.style.setProperty('--cover-bg', theme.surface);
    this.cover.style.setProperty('--cover-bg-end', theme.background);
    this.cover.style.setProperty('--cover-fg', theme.accentText);
    const initials = eraInitials(track.eraName);
    this.initials.textContent = initials;
    const src = track.hasCover && track.eraId !== null ? coverUrl(apiBaseUrl, track.eraId, track.coverVersion) : null;
    let image = this.cover.querySelector('img');
    if (!src) {
      image?.remove();
      return;
    }
    if (!image) {
      const created = document.createElement('img');
      created.alt = '';
      created.width = 96;
      created.height = 96;
      created.decoding = 'async';
      // A missing cover leaves the tile underneath visible.
      created.addEventListener('error', () => created.remove());
      this.cover.append(created);
      image = created;
    }
    image.dataset.initials = initials;
    if (image.getAttribute('src') !== src) image.src = src;
  }

  /** The state line; while buffering the controls are marked busy (they stay enabled and focusable). */
  setState(label: string, busy: boolean): void {
    if (this.state.textContent !== label) this.state.textContent = label;
    if (this.controls.getAttribute('aria-busy') !== String(busy)) this.controls.setAttribute('aria-busy', String(busy));
  }

  /** The play/pause button: shows "Pause" while playing or about to play. */
  setToggle(playing: boolean): void {
    const label = playing ? 'Pause' : 'Play';
    if (this.toggle.getAttribute('aria-label') === label) return;
    this.toggle.setAttribute('aria-label', label);
    this.toggle.title = label;
    this.toggle.dataset.playing = String(playing);
  }

  /** Previous/next stay focusable at the ends of the queue (aria-disabled), so focus is never dropped. */
  setSkips(previous: boolean, next: boolean): void {
    this.previous.setAttribute('aria-disabled', String(!previous));
    this.next.setAttribute('aria-disabled', String(!next));
  }

  /** Elapsed time, length and the position slider (left alone while the user drags it). */
  setTime(position: number, duration: number | null, updateSlider = true): void {
    const time = formatDuration(Math.max(0, position));
    const length = duration !== null && duration > 0 ? formatDuration(duration) : '—';
    if (time !== this.shownTime) {
      this.elapsed.textContent = time;
      this.shownTime = time;
    }
    if (length !== this.shownDuration) {
      this.duration.textContent = length;
      this.shownDuration = length;
    }
    const max = duration !== null && duration > 0 ? String(Math.floor(duration)) : '0';
    if (this.seek.max !== max) this.seek.max = max;
    if (updateSlider) {
      const value = String(Math.floor(Math.max(0, position)));
      if (this.seek.value !== value) this.seek.value = value;
    }
    const valueText = positionText(position, duration);
    if (this.seek.getAttribute('aria-valuetext') !== valueText) this.seek.setAttribute('aria-valuetext', valueText);
  }

  setVolume(volume: number, muted: boolean): void {
    const silent = muted || volume === 0;
    const value = String(silent ? 0 : Math.round(volume * 100));
    if (this.volume.value !== value) this.volume.value = value;
    this.volume.setAttribute('aria-valuetext', volumeText(volume, silent));
    this.mute.setAttribute('aria-pressed', String(silent));
    this.mute.dataset.muted = String(silent);
  }

  /** iOS ignores `audio.volume` (hardware buttons only): hide the slider, keep the mute button. */
  setVolumeSliderAvailable(available: boolean): void {
    this.volumeGroup.dataset.slider = String(available);
    this.volume.hidden = !available;
  }

  setQuality(value: string): void {
    if (this.quality.value !== value) this.quality.value = value;
  }

  setQualityAvailable(available: boolean): void {
    this.qualityControl.hidden = !available;
  }

  showError(message: string): void {
    this.errorText.textContent = message;
    this.error.hidden = false;
    this.clearNotice();
    // Errors are announced once through the alert region (the visible row isn't a live region).
    this.alertRegion.textContent = message;
  }

  clearError(): void {
    if (this.error.hidden && !this.alertRegion.textContent) return;
    const focusWasInside = this.error.contains(document.activeElement);
    this.error.hidden = true;
    this.errorText.textContent = '';
    this.alertRegion.textContent = '';
    // The Retry button disappears with the row: keep focus in the player.
    if (focusWasInside) this.toggle.focus();
  }

  /** A brief, non-blocking message above the bar; also announced politely. */
  showNotice(message: string): void {
    window.clearTimeout(this.noticeTimer);
    this.notice.textContent = message;
    this.notice.hidden = false;
    this.announce(message);
    this.noticeTimer = window.setTimeout(() => this.clearNotice(), NOTICE_MS);
  }

  clearNotice(): void {
    window.clearTimeout(this.noticeTimer);
    this.notice.hidden = true;
    this.notice.textContent = '';
  }

  /** Polite screen reader announcement. Each message is a new node, so quick successive messages all get read. */
  announce(message: string): void {
    const line = document.createElement('p');
    line.textContent = message;
    this.announcer.append(line);
    window.setTimeout(() => line.remove(), ANNOUNCEMENT_MS);
  }
}
