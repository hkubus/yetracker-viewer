/**
 * Entry point of the site-wide audio player (loaded by `components/Player.astro`). The player element is persisted
 * across ClientRouter navigations and this module runs once per document, so everything is set up exactly once.
 */
import { canPlayOggOpus, canSetVolume } from './capabilities.ts';
import { PlayerController } from './controller.ts';
import { installKeyboardShortcuts } from './keyboard.ts';
import { DEFAULT_PAGE_SIZE, positiveInt } from './track.ts';
import { PlayerView } from './view.ts';

function apiBaseUrl(root: HTMLElement): string {
  const value =
    root.dataset.apiBaseUrl || document.querySelector<HTMLMetaElement>('meta[name="yt-api-url"]')?.content || '';
  return value.trim().replace(/\/+$/, '');
}

const root = document.querySelector<HTMLElement>('[data-player-root]');
// The flag also stops a second copy of this module (e.g. a development hot reload) from binding twice.
if (root && root.dataset.playerReady !== 'true') {
  root.dataset.playerReady = 'true';
  const view = new PlayerView(root);
  const controller = new PlayerController(view, {
    apiBaseUrl: apiBaseUrl(root),
    pageSize: positiveInt(root.dataset.eraPageSize) ?? DEFAULT_PAGE_SIZE,
    canPlayOpus: canPlayOggOpus(),
    canSetVolume: canSetVolume(),
  });
  controller.start();
  installKeyboardShortcuts(controller, view.seek);
}
