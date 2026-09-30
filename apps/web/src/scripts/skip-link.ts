/**
 * "Skip to main content" moves focus to the main landmark instead of navigating to `#main-content`. A navigation would
 * add a history entry, and once a page has rewritten its URL with `history.replaceState` (the home page's search) the
 * ClientRouter no longer sees the link as pointing at the same page and fetches the page again.
 *
 * One listener for the whole session (the link itself is re-rendered with every page), in the capture phase so it
 * runs before the router's click handler.
 */
const SKIP_LINK_SELECTOR = 'a[data-skip-link]';

document.addEventListener(
  'click',
  (event) => {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>(SKIP_LINK_SELECTOR) : null;
    const target = link?.hash ? document.getElementById(decodeURIComponent(link.hash.slice(1))) : null;
    if (!target) return;
    event.preventDefault();
    target.focus();
  },
  { capture: true },
);
