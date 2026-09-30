export const COLOR_PATTERN = /^[\da-f]{6}$/i;

export function colorValue(value: string | null | undefined, fallback = '666666') {
  const normalized = (value ?? '').trim().replace(/^#/, '');
  return `#${COLOR_PATTERN.test(normalized) ? normalized : fallback}`;
}

/**
 * Contrast-safe colors derived from an era's dominant color, for the site's dark UI. Every value is `#rrggbb`.
 * Ratios are WCAG 2 contrast ratios, guaranteed for any input color.
 */
export interface Theme {
  /** The input color normalized (fallback `#666666`). Not contrast-checked: decoration only. */
  base: string;
  /** Dark, hue-tinted background. */
  background: string;
  /** Raised surface on the background (cards, rows, the player bar). Also dark. */
  surface: string;
  /** Primary text: ≥ 7:1 on `background` and `surface`. */
  text: string;
  /** Secondary text: ≥ 4.5:1 on `background` and `surface`. */
  mutedText: string;
  /** Era-colored non-text UI (borders, icons, fills) and large text: ≥ 3:1 on `background` and `surface`. */
  accent: string;
  /** Era-colored small text (links, labels): ≥ 4.5:1 on `background` and `surface`. */
  accentText: string;
  /** Text or icons drawn on an `accent` fill: ≥ 4.5:1 on `accent`. */
  onAccent: string;
  /** Focus outline: ≥ 3:1 on `background` and `surface` (draw it with an offset so it sits on those). */
  focusRing: string;
}

type Rgb = readonly [r: number, g: number, b: number];
type Lch = { l: number; c: number; h: number };

const WHITE: Rgb = [255, 255, 255];
const BLACK: Rgb = [0, 0, 0];

function parseHex(hex: string): Rgb {
  const value = Number.parseInt(hex.slice(1), 16);
  return [(value >> 16) & 255, (value >> 8) & 255, value & 255];
}

function toHex(rgb: Rgb): string {
  return `#${rgb.map((channel) => channel.toString(16).padStart(2, '0')).join('')}`;
}

function toLinear(channel: number): number {
  const value = channel / 255;
  return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

function fromLinear(value: number): number {
  const clamped = Math.min(1, Math.max(0, value));
  const encoded = clamped <= 0.0031308 ? 12.92 * clamped : 1.055 * clamped ** (1 / 2.4) - 0.055;
  return Math.round(encoded * 255);
}

function luminance(rgb: Rgb): number {
  return 0.2126 * toLinear(rgb[0]) + 0.7152 * toLinear(rgb[1]) + 0.0722 * toLinear(rgb[2]);
}

function contrast(a: Rgb, b: Rgb): number {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** WCAG 2 relative luminance (0–1) of a color; invalid input uses the `colorValue` fallback. */
export function relativeLuminance(color: string): number {
  return luminance(parseHex(colorValue(color)));
}

/** WCAG 2 contrast ratio (1–21) between two colors; invalid input uses the `colorValue` fallback. */
export function contrastRatio(a: string, b: string): number {
  return contrast(parseHex(colorValue(a)), parseHex(colorValue(b)));
}

// OKLab conversions (Björn Ottosson): perceptual lightness keeps the era's hue recognizable while adjusting it.
function toLch(rgb: Rgb): Lch {
  const [r, g, b] = rgb.map(toLinear) as [number, number, number];
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const lightness = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return { l: lightness, c: Math.hypot(a, bb), h: Math.atan2(bb, a) };
}

function lchToLinear(l: number, c: number, h: number): [number, number, number] {
  const a = c * Math.cos(h);
  const b = c * Math.sin(h);
  const lp = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const mp = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const sp = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * lp - 3.3077115913 * mp + 0.2309699292 * sp,
    -1.2684380046 * lp + 2.6097574011 * mp - 0.3413193965 * sp,
    -0.0041960863 * lp - 0.7034186147 * mp + 1.707614701 * sp,
  ];
}

const inGamut = (channels: readonly number[]) => channels.every((value) => value >= -1e-7 && value <= 1 + 1e-7);

/** OKLCH → 8-bit sRGB, reducing chroma (never lightness or hue) until the color fits the sRGB gamut. */
function fromLch({ l, c, h }: Lch): Rgb {
  const lightness = Math.min(1, Math.max(0, l));
  let channels = lchToLinear(lightness, c, h);
  if (!inGamut(channels)) {
    let low = 0;
    let high = c;
    for (let step = 0; step < 24; step += 1) {
      const mid = (low + high) / 2;
      if (inGamut(lchToLinear(lightness, mid, h))) low = mid;
      else high = mid;
    }
    channels = lchToLinear(lightness, low, h);
  }
  return [fromLinear(channels[0]), fromLinear(channels[1]), fromLinear(channels[2])];
}

/**
 * The first color at or above `start`'s lightness (same hue and chroma, gamut-mapped) that reaches `minRatio`
 * against every color in `against`. Checks run on the final 8-bit values; white is the last resort, and the
 * backgrounds passed in are dark enough for white to pass.
 */
function lightenUntil(start: Lch, against: readonly Rgb[], minRatio: number): Rgb {
  const passes = (rgb: Rgb) => against.every((other) => contrast(rgb, other) >= minRatio);
  const first = fromLch(start);
  if (passes(first)) return first;
  let low = start.l;
  let high = 1;
  for (let step = 0; step < 24; step += 1) {
    const mid = (low + high) / 2;
    if (passes(fromLch({ ...start, l: mid }))) high = mid;
    else low = mid;
  }
  const found = fromLch({ ...start, l: high });
  return passes(found) ? found : WHITE;
}

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

function buildTheme(baseHex: string): Theme {
  const base = parseHex(baseHex);
  const { l, c, h } = toLch(base);
  const backgroundLightness = clamp(l, 0.2, 0.25);
  const background = fromLch({ l: backgroundLightness, c: Math.min(c, 0.06), h });
  const surface = fromLch({ l: backgroundLightness + 0.06, c: Math.min(c, 0.07), h });
  const backdrops = [background, surface];
  const accent = lightenUntil({ l, c, h }, backdrops, 3);
  const accentText = lightenUntil({ ...toLch(accent), c, h }, backdrops, 4.5);
  const onAccent = [background, WHITE, BLACK].find((candidate) => contrast(candidate, accent) >= 4.5) ?? BLACK;
  return {
    base: baseHex,
    background: toHex(background),
    surface: toHex(surface),
    text: toHex(lightenUntil({ l: 0.97, c: Math.min(c, 0.012), h }, backdrops, 7)),
    mutedText: toHex(lightenUntil({ l: 0.72, c: Math.min(c, 0.035), h }, backdrops, 4.5)),
    accent: toHex(accent),
    accentText: toHex(accentText),
    onAccent: toHex(onAccent),
    focusRing: toHex(lightenUntil({ l: 0.9, c: Math.min(c, 0.1), h }, backdrops, 3)),
  };
}

const themeCache = new Map<string, Theme>();

/**
 * Contrast-safe theme for an era's dominant color (`rrggbb` with or without `#`; anything else falls back to
 * `666666`). Results are cached per color.
 */
export function themeFor(dominantColor: string | null | undefined): Theme {
  const baseHex = colorValue(dominantColor).toLowerCase();
  let theme = themeCache.get(baseHex);
  if (!theme) {
    if (themeCache.size >= 512) themeCache.clear();
    theme = buildTheme(baseHex);
    themeCache.set(baseHex, theme);
  }
  return theme;
}

const THEME_VARIABLES: ReadonlyArray<[keyof Theme, string]> = [
  ['base', 'base'],
  ['background', 'background'],
  ['surface', 'surface'],
  ['text', 'text'],
  ['mutedText', 'muted-text'],
  ['accent', 'accent'],
  ['accentText', 'accent-text'],
  ['onAccent', 'on-accent'],
  ['focusRing', 'focus-ring'],
];

/**
 * The theme as CSS custom properties: `--theme-background`, `--theme-surface`, `--theme-text`,
 * `--theme-muted-text`, `--theme-accent`, `--theme-accent-text`, `--theme-on-accent`, `--theme-focus-ring`,
 * `--theme-base` (with another `prefix` if given). Handy for `element.style.setProperty(name, value)`.
 */
export function themeVariables(theme: Theme, prefix = '--theme'): Record<string, string> {
  return Object.fromEntries(THEME_VARIABLES.map(([key, name]) => [`${prefix}-${name}`, theme[key]]));
}

/** `themeVariables(themeFor(color))` as a `style` attribute value. */
export function themeStyle(dominantColor: string | null | undefined, prefix = '--theme'): string {
  return Object.entries(themeVariables(themeFor(dominantColor), prefix))
    .map(([name, value]) => `${name}: ${value}`)
    .join('; ');
}
