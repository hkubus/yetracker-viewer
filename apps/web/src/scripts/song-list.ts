/**
 * Client behavior of the era song list (components/SongList.astro): instant filtering of the rendered page,
 * clamped notes with "More" toggles, the "more links" menus, clean search URLs, focus after a search, and centering
 * of a deep-linked song (`#song-123`).
 *
 * With the ClientRouter this module is evaluated once per document. Each page that contains a song list is set up
 * on `astro:page-load` (and right away on the first load) and everything it registers — listeners, observers,
 * timers — is released on `astro:before-swap` through one AbortController per page.
 */
import { categoryMarkersIn } from '../songCategories';
import { DEFAULT_SONG_SORT } from '../songSorts';
import { fold, matchesAllTokens, normalizeQuery, tokens } from '../utils/search';
import { countLabel } from '../utils/songRow';

type NavigationKind = 'initial' | 'push' | 'replace' | 'traverse';

interface ScrollPosition {
  x: number;
  y: number;
}

/** What a page does with the scroll position once its layout has settled. */
type ScrollIntent = { kind: 'center-linked-row' } | { kind: 'restore'; position: ScrollPosition } | null;

interface Group {
  header: HTMLTableRowElement;
  label: string;
  rows: Row[];
}

interface Row {
  element: HTMLTableRowElement;
  notes: HTMLElement | null;
  toggle: HTMLButtonElement | null;
  expanded: boolean;
  /** Whether the notes' overflow is known for the current width. */
  measured: boolean;
  group: Group | null;
  /** Folded text the instant filter matches, built on first use. */
  haystack: string | null;
  /** The title line (category markers included), read on first use. */
  title: string | null;
}

/** What a typed query asks for, like the API reads `q`: folded tokens, and category markers (⭐ …) to carry. */
interface ListQuery {
  tokens: string[];
  markers: string[];
}

/**
 * What the instant filter matches, like the API's search within an era (the song's own text): name, notes, quality
 * and availability (the length cell without its time) from these cells, plus the sub-era, added separately.
 */
const SEARCHED_CELLS = '.song-title-cell, [data-notes-text], .song-quality, .song-length';
/** Text inside the searched cells that the API doesn't search. */
const UNSEARCHED_TEXT = '.sr-only, .song-length__time';
const ANNOUNCE_DELAY_MS = 700;
const RESIZE_DELAY_MS = 150;
/**
 * The list's CSS clamps notes from the first layout where the `scripting` media feature is supported; older
 * browsers clamp once `data-enhanced` is set.
 */
const CLAMPS_WITHOUT_FLAG = window.matchMedia('(scripting: enabled)').matches;

/**
 * How the page on screen was reached. Only fresh navigations center a deep-linked row. A module first evaluated
 * after the document finished loading arrived with a client-side navigation whose events it missed: a new page.
 */
let navigation: NavigationKind = document.readyState === 'complete' ? 'push' : 'initial';
/** Scroll position of the history entry being returned to (the router keeps it in `history.state`). */
let traversalScroll: ScrollPosition | null = null;
/** The same for the document's first page (reload, or back/forward into this document). */
const initialScroll = savedScroll();
/** Set when the song search form started the navigation: the new page puts focus back into its search field. */
let focusSearchOnLoad = false;
const enhancedLists = new WeakSet<HTMLElement>();

// Registered once for the whole document: they carry what the next page needs to know about its navigation.
document.addEventListener('astro:before-preparation', (event) => {
  navigation = event.navigationType;
  traversalScroll = navigation === 'traverse' ? savedScroll() : null;
  const source = event.sourceElement;
  focusSearchOnLoad = source instanceof Element && source.closest('[data-song-search-form]') !== null;
});
// Clamp the incoming page's notes before it is swapped in, so the scroll position the router restores refers to the
// layout the page will keep.
if (!CLAMPS_WITHOUT_FLAG) {
  document.addEventListener('astro:before-swap', (event) => {
    for (const list of Array.from(event.newDocument.querySelectorAll('[data-song-list]'))) {
      list.setAttribute('data-enhanced', '');
    }
  });
}
document.addEventListener('astro:page-load', setUpPage);
// On a full page load `astro:page-load` waits for the window `load` event (images included): don't wait for it.
setUpPage();

function setUpPage(): void {
  const lists = Array.from(document.querySelectorAll<HTMLElement>('[data-song-list]')).filter(
    (list) => !enhancedLists.has(list),
  );
  const focusSearch = focusSearchOnLoad;
  focusSearchOnLoad = false;
  if (lists.length === 0) return;

  const controller = new AbortController();
  document.addEventListener('astro:before-swap', () => controller.abort(), { once: true, signal: controller.signal });
  const scroll = scrollIntent();
  for (const list of lists) {
    enhancedLists.add(list);
    enhanceList(list, controller.signal, { scroll, focusSearch });
  }
}

function savedScroll(): ScrollPosition | null {
  const state: unknown = history.state;
  if (typeof state !== 'object' || state === null) return null;
  const { scrollX, scrollY } = state as { scrollX?: unknown; scrollY?: unknown };
  return typeof scrollX === 'number' && typeof scrollY === 'number' ? { x: scrollX, y: scrollY } : null;
}

/**
 * New navigations center a deep-linked row. Back/forward and reloads go back to where the reader was: the router
 * restores that position before the notes are measured, so it is applied again once the layout has settled.
 */
function scrollIntent(): ScrollIntent {
  let position: ScrollPosition | null;
  if (navigation === 'initial') {
    const [entry] = performance.getEntriesByType('navigation') as PerformanceNavigationTiming[];
    if (entry === undefined || entry.type === 'navigate') return { kind: 'center-linked-row' };
    position = initialScroll;
  } else if (navigation === 'traverse') {
    position = traversalScroll;
  } else {
    return { kind: 'center-linked-row' };
  }
  return position ? { kind: 'restore', position } : null;
}

/** Text of an element without the parts the search doesn't cover (visually hidden helper text, lengths). */
function searchableText(element: Element): string {
  let text = '';
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!node.parentElement?.closest(UNSEARCHED_TEXT)) text += ` ${node.nodeValue ?? ''}`;
  }
  return text;
}

function sameTokens(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && [...a].sort().join(' ') === [...b].sort().join(' ');
}

/** The search box's value as the submitted search would read it (`normalizeQuery`: control characters dropped). */
function listQuery(value: string): ListQuery {
  const query = normalizeQuery(value);
  return { tokens: tokens(query), markers: categoryMarkersIn(query) };
}

function sameQuery(a: ListQuery, b: ListQuery): boolean {
  return sameTokens(a.tokens, b.tokens) && sameTokens(a.markers, b.markers);
}

function enhanceList(
  list: HTMLElement,
  signal: AbortSignal,
  options: { scroll: ScrollIntent; focusSearch: boolean },
): void {
  const form = list.querySelector<HTMLFormElement>('[data-song-search-form]');
  const input = list.querySelector<HTMLInputElement>('[data-song-search]');
  const table = list.querySelector<HTMLTableElement>('[data-song-table]');
  const count = list.querySelector<HTMLElement>('[data-song-count]');
  const status = list.querySelector<HTMLElement>('[data-song-status]');
  const empty = list.querySelector<HTMLElement>('[data-song-empty]');

  const rows: Row[] = [];
  const groups: Group[] = [];
  const rowsByNotes = new Map<Element, Row>();
  const rowsByToggle = new Map<Element, Row>();
  for (const body of Array.from(table?.tBodies ?? [])) {
    const header = body.querySelector<HTMLTableRowElement>('tr.song-group-row');
    const group: Group | null = header ? { header, label: header.dataset.subEra ?? '', rows: [] } : null;
    if (group) groups.push(group);
    for (const element of Array.from(body.querySelectorAll<HTMLTableRowElement>('tr[data-play-row]'))) {
      const notes = element.querySelector<HTMLElement>('[data-notes-text]');
      const row: Row = {
        element,
        notes,
        toggle: null,
        expanded: false,
        measured: false,
        group,
        haystack: null,
        title: null,
      };
      rows.push(row);
      group?.rows.push(row);
      if (notes) rowsByNotes.set(notes, row);
    }
  }

  let filterFrame = 0;
  let measureFrame = 0;
  let resizeTimer = 0;
  let announceTimer = 0;
  let afterMeasure: (() => void) | null = null;
  signal.addEventListener(
    'abort',
    () => {
      cancelAnimationFrame(filterFrame);
      cancelAnimationFrame(measureFrame);
      window.clearTimeout(resizeTimer);
      window.clearTimeout(announceTimer);
    },
    { once: true },
  );

  // ---- Notes: clamped to three lines (CSS); a toggle is added where they overflow. ----

  if (!CLAMPS_WITHOUT_FLAG) list.dataset.enhanced = '';

  const scheduleMeasure = (): void => {
    if (measureFrame === 0) measureFrame = requestAnimationFrame(measureNotes);
  };

  function measureNotes(): void {
    measureFrame = 0;
    const pending = rows.filter(
      (row): row is Row & { notes: HTMLElement } =>
        row.notes !== null && !row.measured && !row.expanded && !row.element.hidden,
    );
    // All reads first, then all writes: one layout for the whole batch.
    const overflowing = pending.map((row) => row.notes.scrollHeight > row.notes.clientHeight + 1);
    pending.forEach((row, index) => {
      row.measured = true;
      if (overflowing[index]) toggleFor(row).hidden = false;
      else if (row.toggle) row.toggle.hidden = true;
    });
    afterMeasure?.();
  }

  /** "More"/"Less", named after the song: "More: notes for NEBRASKA [V4]". */
  function labelToggle(row: Row, toggle: HTMLButtonElement, expanded: boolean): void {
    const word = expanded ? 'Less' : 'More';
    const title = row.element.querySelector('.song-title')?.textContent?.trim();
    toggle.textContent = word;
    toggle.setAttribute('aria-label', title ? `${word}: notes for ${title}` : `${word}: notes`);
  }

  function toggleFor(row: Row & { notes: HTMLElement }): HTMLButtonElement {
    if (row.toggle) return row.toggle;
    const toggle = document.createElement('button');
    toggle.type = 'button';
    toggle.className = 'notes-toggle';
    toggle.setAttribute('aria-expanded', 'false');
    if (row.notes.id) toggle.setAttribute('aria-controls', row.notes.id);
    labelToggle(row, toggle, false);
    row.notes.after(toggle);
    row.toggle = toggle;
    rowsByToggle.set(toggle, row);
    return toggle;
  }

  function setExpanded(row: Row, expanded: boolean): void {
    const { notes, toggle } = row;
    if (!notes || !toggle) return;
    row.expanded = expanded;
    if (expanded) notes.dataset.expanded = '';
    else delete notes.dataset.expanded;
    toggle.setAttribute('aria-expanded', String(expanded));
    labelToggle(row, toggle, expanded);
    if (!expanded) {
      // Collapsing a long note can leave the rest of its row above the viewport.
      if (row.element.getBoundingClientRect().top < 0) row.element.scrollIntoView({ block: 'nearest' });
      if (!row.measured) scheduleMeasure();
    }
  }

  list.addEventListener(
    'click',
    (event) => {
      const target = event.target instanceof Element ? event.target.closest('.notes-toggle') : null;
      const row = target ? rowsByToggle.get(target) : undefined;
      if (row) setExpanded(row, !row.expanded);
    },
    { signal },
  );

  // Keyboard focus on a link that the clamp (or the "More" toggle on the last line) hides expands the note.
  list.addEventListener(
    'focusin',
    (event) => {
      const target = event.target instanceof Element ? event.target : null;
      const notes = target?.closest('[data-notes-text]');
      const row = notes ? rowsByNotes.get(notes) : undefined;
      if (!target || !notes || !row?.toggle || row.toggle.hidden || row.expanded) return;
      const link = target.getBoundingClientRect();
      const toggle = row.toggle.getBoundingClientRect();
      // Focusing may already have scrolled the clamped box to the link, which would leave it showing a middle part.
      const hidden =
        notes.scrollTop > 0 ||
        link.bottom > notes.getBoundingClientRect().bottom + 1 ||
        (link.bottom > toggle.top && link.right > toggle.left);
      if (!hidden) return;
      notes.scrollTop = 0;
      setExpanded(row, true);
      target.scrollIntoView({ block: 'nearest' });
    },
    { signal },
  );

  // Line wrapping depends on the width and on the web font: measure again when either changes.
  const remeasureAll = (): void => {
    for (const row of rows) row.measured = false;
    scheduleMeasure();
  };
  if (table && 'ResizeObserver' in window) {
    let width = -1;
    const observer = new ResizeObserver(([entry]) => {
      const next = Math.round(entry?.contentRect.width ?? 0);
      if (next === width) return;
      const initial = width === -1;
      width = next;
      if (initial) return;
      window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(remeasureAll, RESIZE_DELAY_MS);
    });
    observer.observe(table);
    signal.addEventListener('abort', () => observer.disconnect(), { once: true });
  }
  void document.fonts?.ready.then(() => {
    if (!signal.aborted) remeasureAll();
  });

  // ---- Instant filter over the rendered page. ----

  const defaultCount = count?.textContent ?? '';
  const serverQuery = listQuery(input?.defaultValue ?? '');
  let lastAnnouncement = '';

  const haystackOf = (row: Row): string => {
    if (row.haystack === null) {
      const parts = [row.group?.label ?? ''];
      for (const cell of Array.from(row.element.querySelectorAll(SEARCHED_CELLS))) parts.push(searchableText(cell));
      row.haystack = fold(parts.join(' '));
    }
    return row.haystack;
  };

  const matches = (row: Row, query: ListQuery): boolean => {
    if (!matchesAllTokens(haystackOf(row), query.tokens)) return false;
    if (query.markers.length === 0) return true;
    row.title ??= row.element.querySelector('.song-title')?.textContent ?? '';
    const title = row.title;
    return query.markers.every((marker) => title.includes(marker));
  };

  const announce = (message: string): void => {
    window.clearTimeout(announceTimer);
    announceTimer = window.setTimeout(() => {
      if (!status || message === lastAnnouncement) return;
      lastAnnouncement = message;
      status.textContent = message;
    }, ANNOUNCE_DELAY_MS);
  };

  function applyFilter(): void {
    filterFrame = 0;
    if (!input) return;
    const query = listQuery(input.value);
    // The rows are the server's results for its query: typing that same query again must not hide any of them.
    const active = (query.tokens.length > 0 || query.markers.length > 0) && !sameQuery(query, serverQuery);
    let visible = 0;
    for (const row of rows) {
      const show = !active || matches(row, query);
      if (row.element.hidden === show) row.element.hidden = !show;
      if (show) visible += 1;
    }
    for (const group of groups) group.header.hidden = group.rows.every((row) => row.element.hidden);

    const onPage = countLabel(rows.length, 'song');
    if (count)
      count.textContent = active ? `${visible.toLocaleString('en-US')} of ${onPage} on this page` : defaultCount;
    const nothingFound = active && visible === 0;
    if (empty) empty.hidden = !nothingFound;
    if (nothingFound) announce(empty?.textContent?.replace(/\s+/g, ' ').trim() ?? 'No songs match.');
    else if (active)
      announce(`${visible.toLocaleString('en-US')} of ${onPage} on this page ${visible === 1 ? 'matches' : 'match'}.`);
    else if (lastAnnouncement) announce(`Showing all ${onPage} on this page.`);
    scheduleMeasure();
  }

  input?.addEventListener(
    'input',
    () => {
      if (filterFrame === 0) filterFrame = requestAnimationFrame(applyFilter);
    },
    { signal },
  );
  // A value restored by the browser (form state on reload) filters right away.
  if (input && input.value !== input.defaultValue) applyFilter();

  // ---- Search form: the selects only apply on submit; the URL keeps just the non-default values. ----

  form?.addEventListener(
    'formdata',
    (event) => {
      const data = event.formData;
      const query = normalizeQuery(String(data.get('q') ?? ''));
      if (query) data.set('q', query);
      else data.delete('q');
      if (!data.get('category')) data.delete('category');
      const sort = data.get('sort');
      if (!sort || sort === DEFAULT_SONG_SORT) data.delete('sort');
    },
    { signal },
  );

  // ---- "More links" menus: one open at a time; Escape, a click elsewhere or leaving with Tab closes them. ----

  const openMenus = (): HTMLDetailsElement[] =>
    Array.from(list.querySelectorAll<HTMLDetailsElement>('details[data-more-links][open]'));
  list.addEventListener(
    'toggle',
    (event) => {
      const menu = event.target;
      if (!(menu instanceof HTMLDetailsElement) || !menu.open || !menu.matches('[data-more-links]')) return;
      for (const other of openMenus()) if (other !== menu) other.open = false;
    },
    { capture: true, signal },
  );
  list.addEventListener(
    'keydown',
    (event) => {
      if (event.key !== 'Escape' || !(event.target instanceof Element)) return;
      const menu = event.target.closest<HTMLDetailsElement>('details[data-more-links][open]');
      if (!menu) return;
      menu.open = false;
      menu.querySelector('summary')?.focus();
    },
    { signal },
  );
  list.addEventListener(
    'focusout',
    (event) => {
      const menu = event.target instanceof Element ? event.target.closest('details[data-more-links][open]') : null;
      if (
        menu instanceof HTMLDetailsElement &&
        !(event.relatedTarget instanceof Node && menu.contains(event.relatedTarget))
      ) {
        menu.open = false;
      }
    },
    { signal },
  );
  document.addEventListener(
    'click',
    (event) => {
      for (const menu of openMenus()) {
        if (!(event.target instanceof Node && menu.contains(event.target))) menu.open = false;
      }
    },
    { signal },
  );

  // ---- A link to a song on this very page (e.g. the player's era link) centers the row like a fresh deep link. ----
  // The router answers such a click with a same-page fragment navigation (done by the time this listener runs, as it
  // was registered after the router's), which would leave the row at the top.
  document.addEventListener(
    'click',
    (event) => {
      const link = event.target instanceof Element ? event.target.closest('a[href]') : null;
      if (!event.defaultPrevented || !(link instanceof HTMLAnchorElement)) return;
      const url = new URL(link.href, window.location.href);
      const here = window.location;
      if (url.origin !== here.origin || url.pathname !== here.pathname || url.search !== here.search) return;
      const row = url.hash && url.hash === here.hash ? linkedRow(list) : null;
      if (!row) return;
      const center = (): void => row.scrollIntoView({ block: 'center', inline: 'nearest', behavior: 'instant' });
      center();
      // In case the browser applies its own fragment scroll after this task.
      requestAnimationFrame(() => {
        if (!signal.aborted) center();
      });
    },
    { signal },
  );

  // ---- Focus and scroll after navigation. ----

  if (options.focusSearch && input) {
    input.focus({ preventScroll: true });
    input.setSelectionRange(input.value.length, input.value.length);
  }

  const { scroll } = options;
  const linked = scroll?.kind === 'center-linked-row' ? linkedRow(list) : null;
  let applyScroll: (() => void) | null = null;
  if (linked) {
    applyScroll = () => linked.scrollIntoView({ block: 'center', inline: 'nearest', behavior: 'instant' });
  } else if (scroll?.kind === 'restore') {
    const { x, y } = scroll.position;
    applyScroll = () => window.scrollTo({ left: x, top: y, behavior: 'instant' });
  }
  // Measuring the notes changes the height of every row above the target: apply the position again afterwards.
  if (applyScroll) afterMeasure = holdScroll(applyScroll, signal);

  scheduleMeasure();
}

/** The song row a `#song-…` fragment points at, if it is in this list. */
function linkedRow(list: HTMLElement): HTMLElement | null {
  const { hash } = window.location;
  if (hash.length < 2) return null;
  let id = hash.slice(1);
  try {
    id = decodeURIComponent(id);
  } catch {
    // Keep the raw fragment when it is not valid percent-encoding.
  }
  const row = document.getElementById(id);
  return row instanceof HTMLTableRowElement && row.hasAttribute('data-play-row') && list.contains(row) ? row : null;
}

/**
 * Applies a scroll position now and again while fonts and images settle the layout, until the reader scrolls or
 * interacts; returns the guarded "apply again" function. Deep-linked rows go to the middle of the free space
 * (scroll-padding keeps them clear of the fixed player) instead of flush against the top.
 */
function holdScroll(apply: () => void, signal: AbortSignal): () => void {
  let moved = false;
  const stop = (): void => {
    moved = true;
  };
  for (const type of ['wheel', 'touchstart', 'keydown', 'pointerdown'] as const) {
    window.addEventListener(type, stop, { once: true, passive: true, signal });
  }
  const run = (): void => {
    if (!moved && !signal.aborted) apply();
  };
  run();
  void document.fonts?.ready.then(run);
  if (document.readyState !== 'complete') window.addEventListener('load', run, { once: true, signal });
  const timer = window.setTimeout(run, 800);
  signal.addEventListener('abort', () => window.clearTimeout(timer), { once: true });
  return run;
}
