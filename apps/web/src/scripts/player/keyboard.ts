/**
 * Keyboard shortcuts: Space plays/pauses and ←/→ seek 5 s, but only when focus isn't on something that uses those
 * keys itself, and never with Alt/Ctrl/Meta/Shift held (Alt+← is the browser's Back). The position slider moves
 * 5 s per arrow key and 30 s per Page Up/Down instead of its 1 s step.
 */

export interface KeyboardActions {
  /** A track is shown in the player. */
  readonly active: boolean;
  /** Track length in seconds, when known. */
  readonly trackLength: number | null;
  togglePlay(): void;
  seekBy(seconds: number): void;
  seekTo(seconds: number): void;
}

const ARROW_STEP_S = 5;
const PAGE_STEP_S = 30;

/** Elements that use Space or the arrow keys themselves. */
const INTERACTIVE = [
  'a[href]',
  'area[href]',
  'button',
  'input',
  'select',
  'textarea',
  'summary',
  'iframe',
  'audio[controls]',
  'video[controls]',
  '[contenteditable]:not([contenteditable="false"])',
  '[tabindex]:not([tabindex="-1"])',
  ...[
    'button',
    'checkbox',
    'combobox',
    'gridcell',
    'link',
    'listbox',
    'menuitem',
    'menuitemcheckbox',
    'menuitemradio',
    'option',
    'radio',
    'scrollbar',
    'searchbox',
    'slider',
    'spinbutton',
    'switch',
    'tab',
    'textbox',
    'treeitem',
  ].map((role) => `[role="${role}"]`),
].join(', ');

function isInteractive(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(INTERACTIVE) !== null;
}

function hasModifier(event: KeyboardEvent): boolean {
  return event.altKey || event.ctrlKey || event.metaKey || event.shiftKey;
}

export function installKeyboardShortcuts(actions: KeyboardActions, seekSlider: HTMLInputElement): void {
  document.addEventListener('keydown', (event) => {
    if (event.defaultPrevented || event.isComposing || hasModifier(event) || !actions.active) return;
    const isSpace = event.key === ' ' || event.key === 'Spacebar';
    if (!isSpace && event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    if (isInteractive(event.target)) return;
    event.preventDefault();
    if (isSpace) {
      if (!event.repeat) actions.togglePlay();
      return;
    }
    actions.seekBy(event.key === 'ArrowRight' ? ARROW_STEP_S : -ARROW_STEP_S);
  });

  seekSlider.addEventListener('keydown', (event) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const steps: Record<string, number> = {
      ArrowLeft: -ARROW_STEP_S,
      ArrowDown: -ARROW_STEP_S,
      ArrowRight: ARROW_STEP_S,
      ArrowUp: ARROW_STEP_S,
      PageDown: -PAGE_STEP_S,
      PageUp: PAGE_STEP_S,
    };
    const step = steps[event.key];
    if (step !== undefined) {
      event.preventDefault();
      actions.seekBy(step);
    } else if (event.key === 'Home') {
      event.preventDefault();
      actions.seekTo(0);
    } else if (event.key === 'End') {
      const end = actions.trackLength;
      if (end === null) return;
      event.preventDefault();
      actions.seekTo(end);
    }
  });
}
