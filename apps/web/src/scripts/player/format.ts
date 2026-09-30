import { formatDuration } from '../../utils/duration.ts';

export const SITE_NAME = 'Ye Tracker';

/** Accessible value of the position slider: "1:23 of 3:45" (or just "1:23" while the length is unknown). */
export function positionText(position: number, duration: number | null): string {
  const current = formatDuration(Math.max(0, position));
  return duration !== null && duration > 0 ? `${current} of ${formatDuration(duration)}` : current;
}

export function volumeText(volume: number, muted: boolean): string {
  return muted ? 'Muted' : `${Math.round(Math.min(1, Math.max(0, volume)) * 100)}%`;
}

export function qualityLabel(quality: string): string {
  return quality ? `${quality} kbps` : 'Original';
}

/** `document.title` while a song plays and the tab is hidden. */
export function playingDocumentTitle(title: string): string {
  return `▶ ${title} – ${SITE_NAME}`;
}
