/**
 * The DOM contract between song lists and the player (SPEC §6.2): play buttons (`[data-play-target]`), their rows
 * (`[data-play-row]`) and list scopes (`[data-play-scope]`, with optional `data-queue-*` continuation attributes).
 * Lists never call the player; the player reads these attributes and writes back the current song's state.
 */
import { continuationFrom, Queue } from './queue.ts';
import { firstLine, positiveInt, type Track, trackFromDataset } from './track.ts';

const TARGET = '[data-play-target]';
const ROW = '[data-play-row]';
const SCOPE = '[data-play-scope]';
const ERA_PAGE_PATH = /^\/eras\/([1-9]\d*)\/?$/;

export function playTargetFrom(target: EventTarget | null): HTMLElement | null {
  return target instanceof Element ? target.closest<HTMLElement>(TARGET) : null;
}

function isUnavailable(element: HTMLElement): boolean {
  return (
    (element instanceof HTMLButtonElement && element.disabled) ||
    element.getAttribute('aria-disabled') === 'true' ||
    element.closest('[hidden], [inert]') !== null
  );
}

/** The current page, when it is the era page listing `track` (older markup has no `data-era-position`). */
function currentEraPage(eraId: number | null): string | null {
  const match = ERA_PAGE_PATH.exec(window.location.pathname);
  return match && eraId !== null && Number(match[1]) === eraId
    ? `${window.location.pathname}${window.location.search}`
    : null;
}

export function trackFromTarget(button: HTMLElement): Track | null {
  // Older markup has no data-dominant-color; the era color is then only available as a CSS variable.
  const color = button.dataset.dominantColor ? null : getComputedStyle(button).getPropertyValue('--dominantColor');
  const track = trackFromDataset(button.dataset, { color });
  if (track && track.eraPosition === null) track.pageHref = currentEraPage(track.eraId);
  return track;
}

function inScope(element: Element, scope: HTMLElement | null): boolean {
  return scope === null || element.closest(SCOPE) === scope;
}

/**
 * The queue for a click on `button`: the playable, visible (not `hidden`) songs of its list in DOM order, taken
 * now, plus the list's continuation when the scope describes one. A scope with hidden rows is filtered on the
 * client (e.g. the era list's instant filter): its continuation describes the unfiltered list, so the queue ends
 * with the visible songs.
 */
export function snapshotQueue(button: HTMLElement, clicked: Track): Queue {
  const scope = button.closest<HTMLElement>(SCOPE);
  const root: ParentNode = scope ?? document;
  const tracks: Track[] = [];
  const seen = new Set<number>();
  let index = -1;
  for (const candidate of root.querySelectorAll<HTMLElement>(TARGET)) {
    if (!inScope(candidate, scope)) continue;
    const isClicked = candidate === button;
    if (!isClicked && isUnavailable(candidate)) continue;
    const track = isClicked ? clicked : trackFromTarget(candidate);
    if (!track) continue;
    if (seen.has(track.id)) {
      if (isClicked) index = tracks.findIndex((item) => item.id === track.id);
      continue;
    }
    seen.add(track.id);
    if (isClicked) index = tracks.length;
    tracks.push(track);
  }
  if (index < 0) return Queue.single(clicked);

  let continuation = null;
  const rows = scope ? Array.from(scope.querySelectorAll<HTMLElement>(ROW)).filter((row) => inScope(row, scope)) : [];
  if (scope && !rows.some((row) => row.hidden)) {
    continuation = continuationFrom(
      {
        eraId: scope.dataset.queueEraId,
        offset: scope.dataset.queueOffset,
        total: scope.dataset.queueTotal,
        params: scope.dataset.queueParams,
      },
      rows.length,
      {
        eraName: clicked.eraName,
        color: clicked.color,
        hasCover: clicked.hasCover,
        coverVersion: clicked.coverVersion,
      },
    );
  }
  return new Queue(tracks, index, continuation);
}

function setAttribute(element: Element, name: string, value: string): void {
  if (element.getAttribute(name) !== value) element.setAttribute(name, value);
}

function setPressed(button: HTMLElement, pressed: boolean): void {
  const title = firstLine(button.dataset.title) || 'Untitled';
  setAttribute(button, 'aria-pressed', String(pressed));
  setAttribute(button, 'aria-label', `${pressed ? 'Pause' : 'Play'} ${title}`);
  if (button.hasAttribute('title')) setAttribute(button, 'title', pressed ? 'Pause' : 'Play');
}

/**
 * Marks the current song in every list on the page: its rows get `data-playing="true"` + `aria-current="true"`,
 * its buttons `aria-pressed` (true while playing) and a Play/Pause label. Everything else is reset.
 */
export function syncPlayTargets(currentId: number | null, playing: boolean): void {
  for (const button of document.querySelectorAll<HTMLElement>(`${TARGET}[aria-pressed="true"]`)) {
    if (!playing || positiveInt(button.dataset.id) !== currentId) setPressed(button, false);
  }
  for (const row of document.querySelectorAll<HTMLElement>(`${ROW}[data-playing]`)) {
    if (currentId === null || !row.querySelector(`${TARGET}[data-id="${currentId}"]`)) {
      row.removeAttribute('data-playing');
      row.removeAttribute('aria-current');
    }
  }
  if (currentId === null) return;
  for (const button of document.querySelectorAll<HTMLElement>(`${TARGET}[data-id="${currentId}"]`)) {
    setPressed(button, playing);
    const row = button.closest<HTMLElement>(ROW);
    if (row) {
      setAttribute(row, 'data-playing', 'true');
      setAttribute(row, 'aria-current', 'true');
    }
  }
}

/** A visible play button for the song, to return focus to when the player closes. */
export function visibleTargetFor(id: number): HTMLElement | null {
  for (const button of document.querySelectorAll<HTMLElement>(`${TARGET}[data-id="${id}"]`)) {
    if (!isUnavailable(button) && button.getClientRects().length > 0) return button;
  }
  return null;
}

/** Whether a DOM change added play buttons (e.g. search results rendered on the client). */
export function addsPlayTargets(mutations: readonly MutationRecord[], ignore: Node): boolean {
  for (const mutation of mutations) {
    if (ignore.contains(mutation.target)) continue;
    for (const node of mutation.addedNodes) {
      if (node instanceof Element && (node.matches(TARGET) || node.querySelector(TARGET))) return true;
    }
  }
  return false;
}
