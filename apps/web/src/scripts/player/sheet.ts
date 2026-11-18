/**
 * The expanded "now playing" view of narrow screens. Below the compact breakpoint the player is a one-row mini bar
 * whose info area (a button) expands the same element into a full-screen view; CSS does the layout
 * (`data-expanded` on the player). The collapse button, Escape, any navigation and a viewport that grows past the
 * breakpoint collapse it again. While it is open the rest of the page is inert and doesn't scroll, so it behaves
 * like a modal dialog, without touching history (the ClientRouter owns it).
 */

/** The compact layout's breakpoint: the same media query as the mini player in components/Player.astro. */
export const COMPACT_QUERY = '(max-width: 47.99rem)';

export interface SheetElements {
  root: HTMLElement;
  expand: HTMLButtonElement;
  collapse: HTMLButtonElement;
  /** Gets focus when the view collapses while focus was on something the mini bar doesn't show. */
  fallbackFocus: HTMLElement;
  /** The era link: following it collapses the view first. */
  eraLink: HTMLAnchorElement;
}

export class PlayerSheet {
  private readonly elements: SheetElements;
  private readonly compact: MediaQueryList;
  private readonly onToggle: () => void;
  /** Page elements made inert while the view is open (restored as they were on collapse). */
  private inerted: HTMLElement[] = [];

  /** `onToggle` runs after every expand and collapse (the bar's height changes). */
  constructor(elements: SheetElements, onToggle: () => void) {
    this.elements = elements;
    this.onToggle = onToggle;
    this.compact = window.matchMedia(COMPACT_QUERY);
    elements.expand.addEventListener('click', () => this.expand());
    elements.collapse.addEventListener('click', () => this.collapse(true));
    elements.eraLink.addEventListener('click', (event) => {
      // A plain click navigates in this tab: show the page, not the player (the router may also skip a same-URL
      // navigation). The link itself disappears with the view, so focus moves to the expand button (the router keeps
      // focus inside the persisted player across the swap).
      if (event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey) {
        this.collapse(true);
      }
    });
    document.addEventListener('keydown', (event) => {
      if (event.key !== 'Escape' || event.defaultPrevented || !this.expanded) return;
      event.preventDefault();
      this.collapse(true);
    });
    // Navigations (links, Back/Forward) swap the page under the persisted player. Focus in the view (e.g. on the
    // collapse button during Back) would otherwise stay on a control the mini bar hides.
    document.addEventListener('astro:before-preparation', () => this.collapse(true));
    this.compact.addEventListener('change', () => {
      if (!this.compact.matches) this.collapse(true);
    });
  }

  get expanded(): boolean {
    return this.elements.root.dataset.expanded === 'true';
  }

  expand(): void {
    const { root, collapse } = this.elements;
    if (this.expanded || root.hidden || !this.compact.matches) return;
    root.dataset.expanded = 'true';
    this.setAriaExpanded(true);
    for (const element of document.body.children) {
      if (element !== root && element instanceof HTMLElement && !element.inert) {
        element.inert = true;
        this.inerted.push(element);
      }
    }
    document.documentElement.dataset.playerExpanded = '';
    collapse.focus();
    this.onToggle();
  }

  /**
   * `restoreFocus`: when focus was in the view, return it to the expand button like a dialog does to its opener
   * (or, past the breakpoint, keep it unless the bar doesn't show that control).
   */
  collapse(restoreFocus: boolean): void {
    if (!this.expanded) return;
    const { root, expand, fallbackFocus } = this.elements;
    delete root.dataset.expanded;
    this.setAriaExpanded(false);
    for (const element of this.inerted) element.inert = false;
    this.inerted = [];
    delete document.documentElement.dataset.playerExpanded;
    this.onToggle();
    const active = document.activeElement;
    if (!restoreFocus || !(active instanceof HTMLElement) || !root.contains(active)) return;
    if (this.compact.matches) expand.focus();
    else if (active.getClientRects().length === 0) fallbackFocus.focus();
  }

  private setAriaExpanded(expanded: boolean): void {
    this.elements.expand.setAttribute('aria-expanded', String(expanded));
    this.elements.collapse.setAttribute('aria-expanded', String(expanded));
  }
}
