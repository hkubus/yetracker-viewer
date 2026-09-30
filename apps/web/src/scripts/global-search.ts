/**
 * Home page song search (`GlobalSongSearch.astro`).
 *
 * - The search state (`q`, `eraFrom`, `eraTo`, `playable`) is kept in the page URL with `history.replaceState`, so
 *   reloads, back/forward and shared links show the same search. Results are cached in sessionStorage when the page
 *   is left, so going back restores them (and the scroll position) without waiting for the API.
 * - Typing searches after a short pause; older requests are aborted, slow ones time out with a visible error.
 *   Filters alone (era range, playable only) also search. "Show more" pages through the matches with `offset`.
 * - Keyboard: Enter searches right away and moves focus to the results, ArrowDown goes from the input to the first
 *   result, ArrowUp/ArrowDown move between results, Escape returns to the input (or clears it).
 *
 * Set up on page load and after every client-side navigation; everything registered for a page is torn down on
 * `astro:before-swap`.
 */
import { getApiBaseUrl } from '../utils/api-base-url';
import { themeFor, themeVariables } from '../utils/color';
import {
  apiSearchParams,
  type GlobalSearchState,
  hasActiveFilters,
  hasSearchTerms,
  pageSearchParams,
  parseSearchState,
  searchKey,
  searchMode,
} from '../utils/globalSearchState';
import { clampQuery, MAX_QUERY_LENGTH } from '../utils/search';
import {
  DEFAULT_ERA_PAGE_SIZE,
  type EraDisplayInfo,
  playButtonAttributes,
  positiveInteger,
  type SearchSongPayload,
  songHref,
  songTextLines,
} from '../utils/songDisplay';
import { countLabel } from '../utils/songRow';

const ROOT_SELECTOR = '[data-global-song-search]';
const DEBOUNCE_MS = 200;
const REQUEST_TIMEOUT_MS = 10_000;
/** Results per request (the API's maximum). */
const PAGE_LIMIT = 50;
/** Largest `offset` the API accepts. */
const MAX_OFFSET = 10_000;
const NOTES_PREVIEW_LENGTH = 200;
const NOTES_TOOLTIP_LENGTH = 600;
const SNAPSHOT_KEY = 'yt:global-search';
const SNAPSHOT_MAX_AGE_MS = 30 * 60_000;
/** Minimum horizontal space between the two era-range labels before they are stacked. */
const LABEL_GAP_PX = 12;
const WHITESPACE = /\s+/g;

interface EraOption extends EraDisplayInfo {
  id: number;
  name: string;
}

interface Results {
  key: string;
  /** Whether the query had search terms (changes the "no matches" wording). */
  hasText: boolean;
  songs: SearchSongPayload[];
  total: number;
  /** The API supports `offset` (older versions ignore it and always return the first page). */
  pageable: boolean;
}

interface Snapshot extends Results {
  url: string;
  scrollY: number;
  savedAt: number;
}

interface Navigation {
  /** Back/forward or reload: restore cached results and the scroll position. */
  restore: boolean;
  scrollY: number | null;
}

type FailureKind = 'timeout' | 'offline' | 'too-long' | 'bad-request' | 'busy' | 'server' | 'invalid';

interface Failure {
  kind: FailureKind;
  detail?: string;
}

interface Page {
  songs: SearchSongPayload[];
  total: number;
  pageable: boolean;
}

class HttpError extends Error {
  status: number;
  detail: string;

  constructor(status: number, detail: string) {
    super(`HTTP ${status}`);
    this.name = 'HttpError';
    this.status = status;
    this.detail = detail;
  }
}

class InvalidResponseError extends Error {
  constructor() {
    super('Unexpected search response');
    this.name = 'InvalidResponseError';
  }
}

/** Same fixed locale as the server-rendered counts. */
function formatNumber(value: number): string {
  return value.toLocaleString('en-US');
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function finiteOrNull(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function truncate(text: string, length: number): string {
  return text.length <= length ? text : `${text.slice(0, length - 1).trimEnd()}…`;
}

function parseEras(raw: string | undefined): EraOption[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw ?? '[]');
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed.flatMap((item): EraOption[] => {
    if (!isRecord(item)) return [];
    const id = positiveInteger(item.id);
    if (id === null) return [];
    return [
      {
        id,
        name: typeof item.name === 'string' && item.name ? item.name : `Era ${id}`,
        dominantColor: typeof item.dominantColor === 'string' ? item.dominantColor : null,
        hasCover: typeof item.hasCover === 'boolean' ? item.hasCover : null,
        coverVersion: typeof item.coverVersion === 'string' ? item.coverVersion : null,
      },
    ];
  });
}

function describeFailure(error: unknown, timedOut: boolean): Failure {
  if (timedOut) return { kind: 'timeout' };
  if (error instanceof HttpError) {
    if (error.status === 400) {
      return /too long/i.test(error.detail) ? { kind: 'too-long' } : { kind: 'bad-request', detail: error.detail };
    }
    if (error.status === 429 || error.status === 503) return { kind: 'busy' };
    return { kind: 'server', detail: String(error.status) };
  }
  if (error instanceof InvalidResponseError || error instanceof SyntaxError) return { kind: 'invalid' };
  return { kind: 'offline' };
}

function failureText(failure: Failure): string {
  switch (failure.kind) {
    case 'timeout':
      return 'The search took too long to answer.';
    case 'offline':
      return navigator.onLine === false
        ? 'You appear to be offline.'
        : 'Couldn’t reach the catalog. Check your connection.';
    case 'too-long':
      return `That search is too long: use at most ${MAX_QUERY_LENGTH} characters.`;
    case 'bad-request':
      return failure.detail ? `This search couldn’t be run (${failure.detail}).` : 'This search couldn’t be run.';
    case 'busy':
      return 'Search is busy right now.';
    case 'server':
      return 'Search failed because of a server error.';
    case 'invalid':
      return 'The catalog sent a response this page couldn’t read.';
  }
}

function canRetry(failure: Failure): boolean {
  return failure.kind !== 'too-long' && failure.kind !== 'bad-request';
}

function summaryText(results: Pick<Results, 'songs' | 'total'>): string {
  const shown = results.songs.length;
  if (results.total === 0) return 'No matches';
  if (shown >= results.total) return countLabel(results.total, 'match', 'matches');
  return `${formatNumber(shown)} of ${countLabel(results.total, 'match', 'matches')}`;
}

/** Keeps what the results need, so cached snapshots stay small. */
function slimSong(song: SearchSongPayload): SearchSongPayload {
  return {
    id: song.id,
    eraId: song.eraId,
    eraPosition: song.eraPosition,
    name: song.name,
    title: song.title,
    notes: typeof song.notes === 'string' ? truncate(song.notes, NOTES_TOOLTIP_LENGTH) : null,
    eraName: song.eraName,
    dominantColor: song.dominantColor,
    eraHasCover: song.eraHasCover,
    eraCoverVersion: song.eraCoverVersion,
    playable: song.playable,
    trackLength: song.trackLength,
    duration: song.duration,
  };
}

function readSnapshot(url: string): Snapshot | null {
  let raw: string | null;
  try {
    raw = sessionStorage.getItem(SNAPSHOT_KEY);
  } catch {
    return null;
  }
  if (!raw) return null;
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (
    !isRecord(value) ||
    value.url !== url ||
    typeof value.key !== 'string' ||
    !Array.isArray(value.songs) ||
    typeof value.total !== 'number' ||
    typeof value.savedAt !== 'number' ||
    Date.now() - value.savedAt > SNAPSHOT_MAX_AGE_MS
  ) {
    return null;
  }
  return {
    url,
    key: value.key,
    hasText: value.hasText === true,
    songs: value.songs.filter(isRecord) as SearchSongPayload[],
    total: value.total,
    pageable: value.pageable === true,
    scrollY: finiteOrNull(value.scrollY) ?? 0,
    savedAt: value.savedAt,
  };
}

function writeSnapshot(snapshot: Snapshot | null): void {
  try {
    if (snapshot) sessionStorage.setItem(SNAPSHOT_KEY, JSON.stringify(snapshot));
    else sessionStorage.removeItem(SNAPSHOT_KEY);
  } catch {
    // Storage full or unavailable: going back simply searches again.
  }
}

interface EraRange {
  readonly from: number;
  readonly to: number;
  set(from: number, to: number): void;
}

/**
 * The era range: two native range inputs over one track. Adds what native inputs lack here: a click on the track
 * moves the nearest thumb (and keeps dragging it), era names under the thumbs that are re-measured on resize and
 * font loads, and colors from each era's theme.
 */
function setupEraRange(
  root: HTMLElement,
  eras: readonly EraOption[],
  signal: AbortSignal,
  onChange: () => void,
): EraRange | null {
  const container = root.querySelector<HTMLElement>('[data-era-range]');
  const start = root.querySelector<HTMLInputElement>('[data-era-range-start]');
  const end = root.querySelector<HTMLInputElement>('[data-era-range-end]');
  const names = root.querySelector<HTMLElement>('[data-era-range-names]');
  const startName = root.querySelector<HTMLElement>('[data-era-start-name]');
  const endName = root.querySelector<HTMLElement>('[data-era-end-name]');
  const status = root.querySelector<HTMLElement>('[data-era-range-status]');
  if (!container || !start || !end || !names || !startName || !endName || !status || eras.length < 2) return null;

  const last = eras.length - 1;
  const accents = eras.map((era) => themeFor(era.dominantColor).accentText);
  const indexOf = (input: HTMLInputElement) => Math.min(Math.max(Math.round(Number(input.value)) || 0, 0), last);

  const layoutLabels = () => {
    const width = names.clientWidth;
    if (width === 0) return;
    // The names row is inset by half a thumb on each side; labels may use that space too.
    const inset = Math.max((container.clientWidth - width) / 2, 0);
    const from = indexOf(start);
    const to = indexOf(end);
    // One era selected: a single label under both thumbs.
    endName.hidden = from === to;
    names.setAttribute('data-measured', '');
    names.removeAttribute('data-stacked');
    const place = (label: HTMLElement, index: number) => {
      const labelWidth = label.offsetWidth;
      const left = Math.min(Math.max((index / last) * width - labelWidth / 2, -inset), width + inset - labelWidth);
      return { label, left, right: left + labelWidth };
    };
    const positions = from === to ? [place(startName, from)] : [place(startName, from), place(endName, to)];
    const [first, second] = positions;
    // Labels that would collide go on two lines rather than being cut short.
    if (first && second && first.right + LABEL_GAP_PX > second.left) names.setAttribute('data-stacked', '');
    for (const { label, left } of positions) {
      label.style.left = `${left}px`;
      label.style.right = 'auto';
    }
  };

  const render = () => {
    const from = indexOf(start);
    const to = indexOf(end);
    container.style.setProperty('--range-start', `${(from / last) * 100}%`);
    container.style.setProperty('--range-end', `${(to / last) * 100}%`);
    container.style.setProperty('--range-start-color', accents[from] ?? '#cfcfcf');
    container.style.setProperty('--range-end-color', accents[to] ?? '#cfcfcf');
    // With both thumbs on one era, keep the one that can still move on top.
    container.dataset.front = from === to && to === last ? 'start' : 'end';
    const fromName = eras[from]?.name ?? '';
    const toName = eras[to]?.name ?? '';
    start.setAttribute('aria-valuetext', fromName);
    end.setAttribute('aria-valuetext', toName);
    startName.textContent = fromName;
    endName.textContent = toName;
    status.textContent = from === 0 && to === last ? 'All eras' : countLabel(to - from + 1, 'era');
    layoutLabels();
  };

  start.addEventListener(
    'input',
    () => {
      if (indexOf(start) > indexOf(end)) end.value = start.value;
      render();
      onChange();
    },
    { signal },
  );
  end.addEventListener(
    'input',
    () => {
      if (indexOf(end) < indexOf(start)) start.value = end.value;
      render();
      onChange();
    },
    { signal },
  );

  const indexAt = (clientX: number) => {
    const rect = start.getBoundingClientRect();
    const thumb = rect.height;
    const ratio = (clientX - rect.left - thumb / 2) / Math.max(rect.width - thumb, 1);
    return Math.round(Math.min(Math.max(ratio, 0), 1) * last);
  };
  const moveTo = (input: HTMLInputElement, index: number) => {
    if (indexOf(input) === index) return;
    input.value = String(index);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };

  // Pointer presses on a thumb reach its input; anything else on the slider lands here.
  container.addEventListener(
    'pointerdown',
    (event) => {
      if (event.button !== 0 || event.target === start || event.target === end) return;
      const index = indexAt(event.clientX);
      const from = indexOf(start);
      const to = indexOf(end);
      const input = index <= from ? start : index >= to ? end : index - from <= to - index ? start : end;
      event.preventDefault();
      input.focus({ preventScroll: true });
      moveTo(input, index);
      try {
        container.setPointerCapture(event.pointerId);
      } catch {
        // The pointer is already gone; there is nothing to drag.
        return;
      }
      const drag = new AbortController();
      const stop = () => drag.abort();
      signal.addEventListener('abort', stop, { once: true, signal: drag.signal });
      container.addEventListener('pointermove', (move) => moveTo(input, indexAt(move.clientX)), {
        signal: drag.signal,
      });
      container.addEventListener('pointerup', stop, { signal: drag.signal });
      container.addEventListener('pointercancel', stop, { signal: drag.signal });
      container.addEventListener('lostpointercapture', stop, { signal: drag.signal });
    },
    { signal },
  );

  // Observe the slider, not the names row: stacking the labels changes the row's height from inside the callback.
  const resizeObserver = new ResizeObserver(() => layoutLabels());
  resizeObserver.observe(container);
  signal.addEventListener('abort', () => resizeObserver.disconnect(), { once: true });
  if ('fonts' in document) {
    document.fonts.addEventListener('loadingdone', layoutLabels, { signal });
    void document.fonts.ready.then(() => {
      if (!signal.aborted) layoutLabels();
    });
  }

  render();
  return {
    get from() {
      return indexOf(start);
    },
    get to() {
      return indexOf(end);
    },
    set(from: number, to: number) {
      start.value = String(from);
      end.value = String(to);
      render();
    },
  };
}

function setupSearch(root: HTMLElement, signal: AbortSignal, navigation: Navigation): void {
  const form = root.querySelector<HTMLFormElement>('[data-global-search-form]');
  const input = root.querySelector<HTMLInputElement>('[data-global-search-input]');
  const count = root.querySelector<HTMLElement>('[data-global-search-count]');
  const status = root.querySelector<HTMLElement>('[data-global-search-status]');
  const panel = root.querySelector<HTMLElement>('[data-global-search-results]');
  const list = root.querySelector<HTMLElement>('[data-global-search-list]');
  const message = root.querySelector<HTMLElement>('[data-global-search-message]');
  const messageText = root.querySelector<HTMLElement>('[data-global-search-message-text]');
  const retry = root.querySelector<HTMLButtonElement>('[data-global-search-retry]');
  const more = root.querySelector<HTMLElement>('[data-global-search-more]');
  const moreButton = root.querySelector<HTMLButtonElement>('[data-global-search-more-button]');
  const moreProgress = root.querySelector<HTMLElement>('[data-global-search-more-progress]');
  const playable = root.querySelector<HTMLInputElement>('[data-playable-filter]');
  const clear = root.querySelector<HTMLButtonElement>('[data-search-clear]');
  const playIcon = root.querySelector<HTMLTemplateElement>('[data-global-search-play-icon]');
  if (
    !form ||
    !input ||
    !count ||
    !status ||
    !panel ||
    !list ||
    !message ||
    !messageText ||
    !retry ||
    !more ||
    !moreButton ||
    !moreProgress ||
    !playable ||
    !clear
  ) {
    return;
  }

  const eras = parseEras(root.dataset.eras);
  const eraIds = eras.map((era) => era.id);
  const eraById = new Map(eras.map((era) => [era.id, era]));
  const apiBaseUrl = (root.dataset.apiBaseUrl || getApiBaseUrl()).replace(/\/+$/, '');
  const eraPageSize = positiveInteger(root.dataset.eraPageSize) ?? DEFAULT_ERA_PAGE_SIZE;
  const idleCount = root.dataset.idleCount ?? '';
  const lastEra = Math.max(eras.length - 1, 0);

  let results: Results | null = null;
  let request: { controller: AbortController; key: string; kind: 'search' | 'more' } | null = null;
  let debounceTimer: number | undefined;
  let announceTimer: number | undefined;
  let focusResultsWhenReady = false;
  let retryAction: (() => void) | null = null;
  // The URL of this page's history entry (without the hash), kept here because `location` already points at the
  // next page while a back/forward navigation away from this one is being prepared.
  let pageUrl = location.pathname + location.search;

  const range = setupEraRange(root, eras, signal, () => scheduleSearch());

  signal.addEventListener(
    'abort',
    () => {
      window.clearTimeout(debounceTimer);
      window.clearTimeout(announceTimer);
      request?.controller.abort();
      request = null;
    },
    { once: true },
  );

  const readState = (): GlobalSearchState => ({
    q: clampQuery(input.value),
    from: range?.from ?? 0,
    to: range?.to ?? lastEra,
    playable: playable.checked,
  });

  const applyState = (state: GlobalSearchState) => {
    if (clampQuery(input.value) !== state.q) input.value = state.q;
    range?.set(state.from, state.to);
    playable.checked = state.playable;
  };

  const updateClear = (state: GlobalSearchState) => {
    clear.disabled = input.value === '' && !hasActiveFilters(state, eras.length);
  };

  const syncUrl = (state: GlobalSearchState) => {
    const query = pageSearchParams(state, eraIds).toString();
    const url = `${location.pathname}${query ? `?${query}` : ''}`;
    if (url === pageUrl) return;
    try {
      // Keep the router's state (history index, scroll position) and only change the URL.
      history.replaceState(history.state, '', `${url}${location.hash}`);
      pageUrl = url;
    } catch {
      // Browsers throttle history updates; the next change tries again.
    }
  };

  const announce = (text: string) => {
    window.clearTimeout(announceTimer);
    status.textContent = '';
    // A short delay makes screen readers announce the text even when it equals the previous announcement.
    announceTimer = window.setTimeout(() => {
      status.textContent = text;
    }, 100);
  };

  const setBusy = (busy: boolean) => {
    if (busy) panel.setAttribute('aria-busy', 'true');
    else panel.removeAttribute('aria-busy');
  };

  const focusResults = () => {
    focusResultsWhenReady = false;
    if (!panel.hidden) panel.focus();
  };

  // Hiding a container that holds focus would drop focus to the page: move it to the results (or the input) first.
  const releaseFocus = (container: HTMLElement) => {
    if (!container.contains(document.activeElement)) return;
    if (panel.hidden) input.focus();
    else panel.focus();
  };

  const showMessage = (text: string, onRetry: (() => void) | null = null) => {
    messageText.textContent = text;
    retryAction = onRetry;
    retry.hidden = onRetry === null;
    message.hidden = false;
  };

  const hideMessage = () => {
    releaseFocus(message);
    message.hidden = true;
    retryAction = null;
  };

  const hideMore = () => {
    releaseFocus(more);
    more.hidden = true;
  };

  const clearList = () => {
    releaseFocus(list);
    list.replaceChildren();
  };

  const showIdle = () => {
    if (panel.contains(document.activeElement)) input.focus();
    results = null;
    list.replaceChildren();
    panel.hidden = true;
    hideMessage();
    hideMore();
    setBusy(false);
    count.textContent = idleCount;
  };

  const showNoTerms = () => {
    results = null;
    clearList();
    hideMore();
    setBusy(false);
    panel.hidden = false;
    count.textContent = idleCount;
    showMessage('Type a word or a number to search.');
  };

  const showFailure = (failure: Failure) => {
    results = null;
    clearList();
    hideMore();
    panel.hidden = false;
    count.textContent = 'Search failed';
    const text = failureText(failure);
    showMessage(text, canRetry(failure) ? () => scheduleSearch({ immediate: true, force: true }) : null);
    announce(`Search failed. ${text}`);
  };

  const renderResult = (song: SearchSongPayload): HTMLLIElement => {
    const item = document.createElement('li');
    item.className = 'global-search__result';
    item.dataset.playRow = '';
    const id = positiveInteger(song.id);
    if (id !== null) item.dataset.songId = String(id);
    const eraId = positiveInteger(song.eraId);
    const era = eraId === null ? undefined : eraById.get(eraId);
    const theme = themeFor(song.dominantColor ?? era?.dominantColor);
    for (const [name, value] of Object.entries(themeVariables(theme, '--result'))) {
      item.style.setProperty(name, value);
    }

    const { title, details } = songTextLines(song);
    const eraName = (typeof song.eraName === 'string' && song.eraName.trim()) || era?.name || 'Unknown era';
    const href = songHref(song, eraPageSize);
    const heading = document.createElement(href ? 'a' : 'span');
    heading.className = 'global-search__result-title';
    if (href && heading instanceof HTMLAnchorElement) {
      heading.href = href;
      heading.dataset.resultLink = '';
    }
    const titleText = document.createElement('span');
    titleText.textContent = title;
    const eraHint = document.createElement('span');
    eraHint.className = 'sr-only';
    eraHint.textContent = `, ${eraName}`;
    heading.append(titleText, eraHint);

    const eraLabel = document.createElement('span');
    eraLabel.className = 'global-search__result-era';
    eraLabel.setAttribute('aria-hidden', 'true');
    eraLabel.textContent = eraName;
    item.append(heading, eraLabel);

    if (details.length > 0) {
      const detailsText = document.createElement('span');
      detailsText.className = 'global-search__result-details';
      detailsText.textContent = details.join(' · ');
      item.append(detailsText);
    }

    const notes = typeof song.notes === 'string' ? song.notes.replace(WHITESPACE, ' ').trim() : '';
    if (notes) {
      const notesText = document.createElement('p');
      notesText.className = 'global-search__result-notes';
      notesText.textContent = truncate(notes, NOTES_PREVIEW_LENGTH);
      if (notes.length > NOTES_PREVIEW_LENGTH) notesText.title = truncate(notes, NOTES_TOOLTIP_LENGTH);
      item.append(notesText);
    }

    const attributes = song.playable === true ? playButtonAttributes(song, era) : null;
    if (attributes) {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'global-search__result-play';
      for (const [name, value] of Object.entries(attributes)) button.setAttribute(name, value);
      button.setAttribute('aria-label', `Play ${title}`);
      if (playIcon) button.append(playIcon.content.cloneNode(true));
      item.append(button);
    }

    return item;
  };

  const updateMore = () => {
    const shown = results?.songs.length ?? 0;
    const remaining = results ? results.total - shown : 0;
    if (!results || remaining <= 0) {
      hideMore();
      return;
    }
    more.hidden = false;
    if (results.pageable && shown <= MAX_OFFSET) {
      moreButton.hidden = false;
      moreButton.textContent = `Show ${formatNumber(Math.min(PAGE_LIMIT, remaining))} more`;
      moreProgress.textContent = `${formatNumber(shown)} of ${formatNumber(results.total)} shown`;
    } else {
      releaseFocus(more);
      moreButton.hidden = true;
      moreProgress.textContent = `Showing the first ${formatNumber(shown)} matches. Add words or narrow the era range to find the rest.`;
    }
  };

  const renderResults = (appendFrom = 0) => {
    if (!results) return;
    const items = results.songs.slice(appendFrom).map(renderResult);
    panel.hidden = false;
    if (appendFrom === 0) {
      releaseFocus(list);
      list.replaceChildren(...items);
    } else {
      list.append(...items);
    }
    count.textContent = summaryText(results);
    if (results.songs.length === 0) {
      showMessage(results.hasText ? 'No songs match your search.' : 'No songs match these filters.');
    } else {
      hideMessage();
    }
    updateMore();
  };

  const fetchPage = async (state: GlobalSearchState, offset: number, requestSignal: AbortSignal): Promise<Page> => {
    const params = apiSearchParams(state, eraIds, { offset, limit: PAGE_LIMIT });
    const response = await fetch(`${apiBaseUrl}/songs?${params}`, {
      signal: requestSignal,
      headers: { Accept: 'application/json' },
    });
    if (!response.ok) {
      const plainText = response.headers.get('content-type')?.startsWith('text/plain') ?? false;
      const detail = plainText ? (await response.text()).trim().slice(0, 160) : '';
      throw new HttpError(response.status, detail);
    }
    const body: unknown = await response.json();
    if (!isRecord(body) || !Array.isArray(body.songs)) throw new InvalidResponseError();
    const songs = body.songs.filter(isRecord) as SearchSongPayload[];
    const total = finiteOrNull(body.total);
    return {
      songs,
      total: total !== null && total >= songs.length + offset ? total : songs.length + offset,
      pageable: typeof body.offset === 'number',
    };
  };

  /** Runs `load` as the current request, with a timeout; resolves to null when superseded or torn down. */
  const track = async <T>(
    key: string,
    kind: 'search' | 'more',
    load: (requestSignal: AbortSignal) => Promise<T>,
  ): Promise<{ ok: true; value: T } | { ok: false; failure: Failure } | null> => {
    request?.controller.abort();
    const controller = new AbortController();
    const current = { controller, key, kind };
    request = current;
    let timedOut = false;
    const timer = window.setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, REQUEST_TIMEOUT_MS);
    try {
      const value = await load(controller.signal);
      return request === current && !signal.aborted ? { ok: true, value } : null;
    } catch (error) {
      if (request !== current || signal.aborted || (controller.signal.aborted && !timedOut)) return null;
      return { ok: false, failure: describeFailure(error, timedOut) };
    } finally {
      window.clearTimeout(timer);
      if (request === current) request = null;
    }
  };

  const runSearch = async (state: GlobalSearchState, key: string, restoreScrollY: number | null = null) => {
    syncUrl(state);
    setBusy(true);
    const scrollBefore = window.scrollY;
    const outcome = await track(key, 'search', (requestSignal) => fetchPage(state, 0, requestSignal));
    if (outcome === null) return;
    setBusy(false);
    if (outcome.ok) {
      results = { key, hasText: hasSearchTerms(state.q), ...outcome.value };
      renderResults();
      announce(
        results.songs.length === 0
          ? `No songs match ${results.hasText ? 'your search' : 'these filters'}.`
          : summaryText(results),
      );
      // Back/forward without cached results: restore the scroll position once they are in, unless the reader has
      // scrolled in the meantime.
      if (restoreScrollY !== null && Math.abs(window.scrollY - scrollBefore) < 2) {
        window.scrollTo({ top: restoreScrollY, behavior: 'instant' });
      }
    } else {
      showFailure(outcome.failure);
    }
    if (focusResultsWhenReady) focusResults();
  };

  const scheduleSearch = (options: { immediate?: boolean; force?: boolean } = {}): void => {
    window.clearTimeout(debounceTimer);
    const state = readState();
    updateClear(state);
    const mode = searchMode(state, eras.length);
    if (mode !== 'search') {
      request?.controller.abort();
      request = null;
      if (mode === 'idle') showIdle();
      else showNoTerms();
      debounceTimer = window.setTimeout(() => syncUrl(state), DEBOUNCE_MS);
      if (focusResultsWhenReady) focusResults();
      return;
    }
    const key = searchKey(state, eraIds);
    if (!options.force && results?.key === key) {
      // Back to what is already shown (e.g. a character typed and deleted again). A "Show more" request for these
      // results is still wanted; a search for another query is not.
      if (request?.kind === 'search') {
        request.controller.abort();
        request = null;
      }
      setBusy(false);
      count.textContent = summaryText(results);
      debounceTimer = window.setTimeout(() => syncUrl(state), DEBOUNCE_MS);
      if (focusResultsWhenReady) focusResults();
      return;
    }
    if (!options.force && request?.kind === 'search' && request.key === key) return;
    // Whatever is in flight is for an older query: drop it so only the final results (and count) show up.
    request?.controller.abort();
    request = null;
    count.textContent = 'Searching…';
    setBusy(true);
    if (options.immediate) void runSearch(state, key);
    else debounceTimer = window.setTimeout(() => void runSearch(state, key), DEBOUNCE_MS);
  };

  const loadMore = async () => {
    const current = results;
    if (!current?.pageable || request) return;
    const state = readState();
    if (searchKey(state, eraIds) !== current.key) return;
    const offset = current.songs.length;
    if (offset > MAX_OFFSET) return;
    moreButton.setAttribute('aria-busy', 'true');
    moreProgress.textContent = 'Loading…';
    const outcome = await track(current.key, 'more', (requestSignal) => fetchPage(state, offset, requestSignal));
    moreButton.removeAttribute('aria-busy');
    if (results !== current) return;
    if (outcome === null) {
      // Superseded (e.g. by a search that was dropped again): offer the button again instead of "Loading…".
      if (!signal.aborted) updateMore();
      return;
    }
    if (!outcome.ok) {
      updateMore();
      const text = `Couldn’t load more results. ${failureText(outcome.failure)}`;
      moreProgress.textContent = text;
      announce(text);
      return;
    }
    const known = new Set(current.songs.map((song) => song.id));
    const fresh = outcome.value.songs.filter((song) => !known.has(song.id));
    current.songs.push(...fresh);
    current.total = Math.max(outcome.value.total, current.songs.length);
    // An API that ignores `offset` returns the first page again: stop offering more.
    if (fresh.length === 0) current.pageable = false;
    renderResults(offset);
    announce(`Showing ${summaryText(current)}.`);
    list.children[offset]?.querySelector<HTMLElement>('[data-result-link]')?.focus();
  };

  const resultLinks = () => Array.from(list.querySelectorAll<HTMLElement>('[data-result-link]'));

  input.addEventListener(
    'input',
    () => {
      focusResultsWhenReady = false;
      scheduleSearch();
    },
    { signal },
  );

  input.addEventListener(
    'keydown',
    (event) => {
      if (event.isComposing || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
      if (event.key === 'ArrowDown') {
        const first = panel.hidden ? undefined : resultLinks()[0];
        if (first) {
          event.preventDefault();
          first.focus();
        }
      } else if (event.key === 'Escape' && input.value !== '') {
        event.preventDefault();
        input.value = '';
        scheduleSearch({ immediate: true });
      }
    },
    { signal },
  );

  form.addEventListener(
    'submit',
    (event) => {
      event.preventDefault();
      focusResultsWhenReady = true;
      scheduleSearch({ immediate: true });
    },
    { signal },
  );

  playable.addEventListener('change', () => scheduleSearch({ immediate: true }), { signal });

  clear.addEventListener(
    'click',
    () => {
      input.value = '';
      range?.set(0, lastEra);
      playable.checked = false;
      // Focus moves before the button is disabled.
      input.focus();
      scheduleSearch({ immediate: true });
    },
    { signal },
  );

  panel.addEventListener(
    'keydown',
    (event) => {
      if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
      if (!(event.target instanceof HTMLElement)) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        input.focus();
        return;
      }
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
      const links = resultLinks();
      if (links.length === 0) return;
      const row = event.target.closest('[data-play-row]');
      const index = row ? links.findIndex((link) => row.contains(link)) : -1;
      let next: HTMLElement | undefined;
      if (event.key === 'ArrowDown') {
        next = index === -1 ? (event.target === panel ? links[0] : undefined) : links[index + 1];
      } else if (index === -1) {
        next = event.target === panel ? input : links.at(-1);
      } else {
        next = index === 0 ? input : links[index - 1];
      }
      if (next) {
        event.preventDefault();
        next.focus();
      }
    },
    { signal },
  );

  moreButton.addEventListener('click', () => void loadMore(), { signal });
  retry.addEventListener(
    'click',
    () => {
      // The button disappears with the message: continue from the results once they are in.
      focusResultsWhenReady = true;
      retryAction?.();
    },
    { signal },
  );

  const saveSnapshot = () => {
    writeSnapshot(
      results
        ? {
            ...results,
            songs: results.songs.map(slimSong),
            url: pageUrl,
            scrollY: Math.round(window.scrollY),
            savedAt: Date.now(),
          }
        : null,
    );
  };
  document.addEventListener(
    'astro:before-preparation',
    (event) => {
      // Leaving through a link: record typing that hasn't reached the URL yet. (During back/forward `location`
      // already belongs to the next entry, so the URL is left alone.)
      if ((event as Event & { navigationType?: string }).navigationType !== 'traverse') {
        window.clearTimeout(debounceTimer);
        syncUrl(readState());
      }
      saveSnapshot();
    },
    { signal },
  );
  window.addEventListener('pagehide', saveSnapshot, { signal });

  // Initial state: the URL is the source of truth (the server pre-filled the controls from it too).
  const initial = parseSearchState(new URLSearchParams(location.search), eraIds);
  applyState(initial);
  updateClear(initial);
  // Drop what the state doesn't use (unknown era ids, bogus values, stray parameters) so the address matches the
  // page. Without the era list (catalog outage) era ids can't be checked, so the address is left as it is.
  if (eras.length > 0) syncUrl(initial);
  const mode = searchMode(initial, eras.length);
  if (mode === 'idle') {
    showIdle();
  } else if (mode === 'no-terms') {
    showNoTerms();
  } else {
    const key = searchKey(initial, eraIds);
    const snapshot = navigation.restore ? readSnapshot(pageUrl) : null;
    if (snapshot && snapshot.key === key) {
      results = {
        key,
        hasText: snapshot.hasText,
        songs: snapshot.songs,
        total: snapshot.total,
        pageable: snapshot.pageable,
      };
      renderResults();
      window.scrollTo({ top: snapshot.scrollY, behavior: 'instant' });
    } else {
      count.textContent = 'Searching…';
      void runSearch(initial, key, navigation.restore ? navigation.scrollY : null);
    }
  }
}

// Page lifecycle. Setup runs as soon as the new page's DOM is in place (initial load, `astro:after-swap`) and again
// on `astro:page-load` for pages this module was loaded on late; it is idempotent per search root.
let active: { root: HTMLElement; teardown: () => void } | null = null;
let pendingNavigation: Navigation | null = null;
let firstSetup = true;
const loadedWithDocument = document.readyState !== 'complete';
const initialHistoryScroll = finiteOrNull(isRecord(history.state) ? history.state.scrollY : null);

function takeNavigation(): Navigation {
  if (pendingNavigation) {
    const navigation = pendingNavigation;
    pendingNavigation = null;
    return navigation;
  }
  if (firstSetup && loadedWithDocument) {
    const entry = performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming | undefined;
    const restore = entry?.type === 'reload' || entry?.type === 'back_forward';
    return { restore, scrollY: restore ? initialHistoryScroll : null };
  }
  return { restore: false, scrollY: null };
}

function init(): void {
  const root = document.querySelector<HTMLElement>(ROOT_SELECTOR);
  if (root && root === active?.root) return;
  const navigation = takeNavigation();
  firstSetup = false;
  if (!root) return;
  active?.teardown();
  const controller = new AbortController();
  const current = {
    root,
    teardown: () => {
      controller.abort();
      if (active === current) active = null;
    },
  };
  active = current;
  setupSearch(root, controller.signal, navigation);
}

document.addEventListener('astro:before-preparation', (event) => {
  const traverse = (event as Event & { navigationType?: string }).navigationType === 'traverse';
  // For back/forward, `history.state` is already the target entry's: remember its scroll position before the
  // router overwrites it while our results aren't rendered yet.
  pendingNavigation = {
    restore: traverse,
    scrollY: traverse ? finiteOrNull(isRecord(history.state) ? history.state.scrollY : null) : null,
  };
});
document.addEventListener('astro:before-swap', () => active?.teardown());
document.addEventListener('astro:after-swap', init);
document.addEventListener('astro:page-load', init);
if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init, { once: true });
else init();
