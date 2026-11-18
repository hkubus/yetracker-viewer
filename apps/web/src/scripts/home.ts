/**
 * Home page behaviour besides the song search: the era filter above the era grid (accent- and case-insensitive,
 * every word must match; a "5 of 43 eras" count while it filters, and a "no eras match" state with a button that
 * clears it) and keeping "Updated … ago" current. Set up per page, torn down on `astro:before-swap`.
 */
import { formatRelativeTime } from '../utils/dates';
import { fold, matchesAllTokens, tokens } from '../utils/search';
import { countLabel } from '../utils/songRow';

const ROOT_SELECTOR = '[data-home]';
/** Screen readers hear the filter result once typing pauses, not on every keystroke. */
const ANNOUNCE_DELAY_MS = 600;
const RELATIVE_TIME_REFRESH_MS = 60_000;

function setupEraFilter(root: HTMLElement, signal: AbortSignal): void {
  const input = root.querySelector<HTMLInputElement>('[data-era-filter]');
  const grid = root.querySelector<HTMLElement>('[data-era-grid]');
  const count = root.querySelector<HTMLElement>('[data-era-filter-count]');
  const status = root.querySelector<HTMLElement>('[data-era-filter-status]');
  const empty = root.querySelector<HTMLElement>('[data-era-filter-empty]');
  const emptyQuery = root.querySelector<HTMLElement>('[data-era-filter-query]');
  const clear = root.querySelector<HTMLButtonElement>('[data-era-filter-clear]');
  if (!input || !grid) return;

  const cards = Array.from(grid.querySelectorAll<HTMLElement>('[data-era-filter-text]'));
  const haystacks = cards.map((card) => fold(card.dataset.eraFilterText ?? ''));
  let announceTimer: number | undefined;
  signal.addEventListener('abort', () => window.clearTimeout(announceTimer), { once: true });

  const apply = () => {
    const queryTokens = tokens(input.value);
    let visible = 0;
    cards.forEach((card, index) => {
      const matches = matchesAllTokens(haystacks[index] ?? '', queryTokens);
      card.hidden = !matches;
      if (matches) visible += 1;
    });
    const text =
      queryTokens.length === 0
        ? countLabel(cards.length, 'era')
        : `${visible.toLocaleString('en-US')} of ${countLabel(cards.length, 'era')}`;
    // The page header already gives the total: the count only shows while the filter narrows the grid.
    if (count) count.textContent = queryTokens.length === 0 ? '' : text;
    if (emptyQuery) emptyQuery.textContent = input.value.trim();
    if (empty) {
      // Hiding the message must not drop the focus of its button to the page.
      if (visible !== 0 && empty.contains(document.activeElement)) input.focus();
      empty.hidden = visible !== 0;
    }
    window.clearTimeout(announceTimer);
    if (status) {
      announceTimer = window.setTimeout(() => {
        status.textContent = visible === 0 ? 'No eras match that filter.' : text;
      }, ANNOUNCE_DELAY_MS);
    }
  };

  input.addEventListener('input', apply, { signal });
  clear?.addEventListener(
    'click',
    () => {
      input.value = '';
      input.focus();
      apply();
    },
    { signal },
  );
  // A value the browser restored into the field (e.g. after a reload) filters right away.
  if (input.value) apply();
}

function setupRelativeTimes(root: HTMLElement, signal: AbortSignal): void {
  const times = Array.from(root.querySelectorAll<HTMLElement>('[data-relative-time]'));
  if (times.length === 0) return;
  // The server-rendered text may come from a cached page; recompute it and keep it current.
  const update = () => {
    for (const time of times) {
      const unixSeconds = Number(time.dataset.relativeTime);
      if (Number.isFinite(unixSeconds)) time.textContent = formatRelativeTime(unixSeconds);
    }
  };
  update();
  const timer = window.setInterval(update, RELATIVE_TIME_REFRESH_MS);
  signal.addEventListener('abort', () => window.clearInterval(timer), { once: true });
}

let active: { root: HTMLElement; teardown: () => void } | null = null;

function init(): void {
  const root = document.querySelector<HTMLElement>(ROOT_SELECTOR);
  if (!root || root === active?.root) return;
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
  setupEraFilter(root, controller.signal);
  setupRelativeTimes(root, controller.signal);
}

document.addEventListener('astro:before-swap', () => active?.teardown());
document.addEventListener('astro:page-load', init);
if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init, { once: true });
else init();
