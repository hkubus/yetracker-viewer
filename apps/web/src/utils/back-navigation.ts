/**
 * "Back" links (`<a data-back-link href="/">`) that behave like the browser's Back button when the previous page in
 * this tab is the link's own target: the user returns to that page as they left it (scroll position, search
 * results) instead of getting a fresh copy. In every other case (direct entry, another site before this one, a
 * different previous page) they are ordinary links. Client-only.
 */

interface NavigationEntryLike {
  readonly url: string | null;
  readonly index: number;
}

interface NavigationLike {
  readonly currentEntry: NavigationEntryLike | null;
  entries(): NavigationEntryLike[];
}

// Without the Navigation API: pathnames of this tab's entries visited by client-side navigation in this document,
// keyed by the index the ClientRouter keeps in `history.state`. Entries before a full page load stay unknown.
const entryPaths = new Map<number, string>();
let initialized = false;

function navigationApi(): NavigationLike | undefined {
  const navigation = (window as Window & { navigation?: Partial<NavigationLike> }).navigation;
  return typeof navigation?.entries === 'function' ? (navigation as NavigationLike) : undefined;
}

function routerIndex(): number | undefined {
  const index = (history.state as { index?: unknown } | null)?.index;
  return typeof index === 'number' ? index : undefined;
}

function recordEntry(): void {
  const index = routerIndex();
  if (index !== undefined) entryPaths.set(index, location.pathname);
}

/** Pathname of the previous entry of this tab's history when it is a page of this site, else `null`. */
export function previousSameOriginPath(): string | null {
  const navigation = navigationApi();
  if (navigation) {
    // `entries()` only lists the same-origin entries around the current one, so a neighbour is same-origin.
    const current = navigation.currentEntry;
    if (!current || current.index <= 0) return null;
    const url = navigation.entries()[current.index - 1]?.url;
    return url ? new URL(url).pathname : null;
  }
  const index = routerIndex();
  return index === undefined ? null : (entryPaths.get(index - 1) ?? null);
}

function onClick(event: MouseEvent): void {
  if (event.defaultPrevented || event.button !== 0) return;
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  const link = event.target instanceof Element ? event.target.closest('a[data-back-link]') : null;
  if (!(link instanceof HTMLAnchorElement) || (link.target && link.target !== '_self')) return;
  const target = new URL(link.href);
  if (target.origin !== location.origin || previousSameOriginPath() !== target.pathname) return;
  event.preventDefault();
  history.back();
}

/**
 * Wires every current and future `a[data-back-link]` of the document (idempotent). The listener runs in the capture
 * phase so it can claim the click before the ClientRouter turns it into a navigation.
 */
export function initBackLinks(): void {
  if (initialized) return;
  initialized = true;
  document.addEventListener('click', onClick, { capture: true });
  if (!navigationApi()) {
    document.addEventListener('astro:page-load', recordEntry);
    recordEntry();
  }
}
