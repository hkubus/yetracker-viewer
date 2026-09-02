const COLOR_PATTERN = /^[\da-f]{6}$/i;

export function colorValue(value: string | null | undefined, fallback = '666666') {
  const normalized = (value ?? '').trim().replace(/^#/, '');
  return `#${COLOR_PATTERN.test(normalized) ? normalized : fallback}`;
}
