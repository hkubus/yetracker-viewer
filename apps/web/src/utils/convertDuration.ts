export function convertDuration(duration: number | null | undefined) {
  if (typeof duration !== 'number' || !Number.isFinite(duration) || duration < 0) {
    return '—';
  }
  const minutes = Math.floor(duration / 60);
  const seconds = Math.floor(duration % 60);
  return `${minutes}:${seconds.toString().padStart(2, '0')}`;
}
