/** Shown where a duration is unknown. */
export const MISSING_DURATION = '—';

/**
 * Formats seconds as `m:ss`, or `h:mm:ss` from one hour on. Fractions are truncated (185.9 → `3:05`), like a
 * player's elapsed time. `null`, `undefined`, NaN, ±Infinity and negative values render as `—`.
 */
export function formatDuration(seconds: number | null | undefined): string {
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds < 0) return MISSING_DURATION;
  const total = Math.floor(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secondsPart = String(total % 60).padStart(2, '0');
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, '0')}:${secondsPart}` : `${minutes}:${secondsPart}`;
}
