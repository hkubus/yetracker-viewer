import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
  colorValue,
  contrastRatio,
  relativeLuminance,
  type Theme,
  themeFor,
  themeStyle,
  themeVariables,
} from './color.ts';

const HEX = /^#[\da-f]{6}$/;

/** [foreground role, background roles, minimum ratio] — the guarantees documented on `Theme`. */
const GUARANTEES: ReadonlyArray<[keyof Theme, ReadonlyArray<keyof Theme>, number]> = [
  ['text', ['background', 'surface'], 7],
  ['mutedText', ['background', 'surface'], 4.5],
  ['accentText', ['background', 'surface'], 4.5],
  ['accent', ['background', 'surface'], 3],
  ['focusRing', ['background', 'surface'], 3],
  ['onAccent', ['accent'], 4.5],
];

function assertGuarantees(input: string | null | undefined) {
  const theme = themeFor(input);
  for (const [role, value] of Object.entries(theme)) {
    assert.match(value, HEX, `${role} of ${input}`);
  }
  for (const [foreground, backgrounds, minimum] of GUARANTEES) {
    for (const background of backgrounds) {
      const ratio = contrastRatio(theme[foreground], theme[background]);
      assert.ok(
        ratio >= minimum,
        `${input}: ${foreground} ${theme[foreground]} on ${background} ${theme[background]} is ${ratio.toFixed(2)}:1`,
      );
    }
  }
}

describe('colorValue', () => {
  test('normalizes to #rrggbb with a fallback', () => {
    assert.equal(colorValue('abcdef'), '#abcdef');
    assert.equal(colorValue(' #ABCDEF '), '#ABCDEF');
    assert.equal(colorValue('fff'), '#666666');
    assert.equal(colorValue(null), '#666666');
    assert.equal(colorValue('nope', '000000'), '#000000');
  });
});

describe('contrast helpers', () => {
  test('match the WCAG 2 definitions', () => {
    assert.equal(relativeLuminance('#000000'), 0);
    assert.equal(relativeLuminance('#ffffff'), 1);
    assert.equal(contrastRatio('#000000', '#ffffff'), 21);
    assert.equal(contrastRatio('#ffffff', '#000000'), 21);
    assert.equal(contrastRatio('#777777', '#777777'), 1);
    assert.equal(contrastRatio('#767676', '#ffffff').toFixed(2), '4.54');
  });
});

describe('themeFor', () => {
  test('meets every contrast guarantee over a grid of input colors', () => {
    for (let r = 0; r <= 255; r += 17) {
      for (let g = 0; g <= 255; g += 17) {
        for (let b = 0; b <= 255; b += 17) {
          assertGuarantees([r, g, b].map((channel) => channel.toString(16).padStart(2, '0')).join(''));
        }
      }
    }
  });

  test('meets the guarantees for saturated primaries, extremes and invalid input', () => {
    for (const input of ['ff0000', '00ff00', '0000ff', 'ffff00', '00ffff', 'ff00ff', '000000', 'ffffff', '010101']) {
      assertGuarantees(input);
    }
    for (const input of [null, undefined, '', 'not-a-color', '#12345', '#FFFFFF']) {
      assertGuarantees(input);
    }
  });

  test('keeps backgrounds dark', () => {
    for (const input of ['ffffff', 'ffff00', '00ff00', '666666', '000000']) {
      const theme = themeFor(input);
      assert.ok(relativeLuminance(theme.background) < 0.05, `${input} background ${theme.background}`);
      assert.ok(relativeLuminance(theme.surface) < 0.07, `${input} surface ${theme.surface}`);
    }
  });

  test('keeps an accent that already contrasts enough, and lightens one that does not', () => {
    assert.equal(themeFor('ff5555').accent, '#ff5555');
    const blue = themeFor('0000ff');
    assert.notEqual(blue.accent, '#0000ff');
    assert.ok(relativeLuminance(blue.accent) > relativeLuminance('#0000ff'));
  });

  test('normalizes and falls back like colorValue', () => {
    assert.equal(themeFor('#ABCDEF').base, '#abcdef');
    assert.equal(themeFor('abcdef'), themeFor('#abcdef'));
    assert.equal(themeFor(undefined).base, '#666666');
    assert.deepEqual(themeFor('bogus'), themeFor('666666'));
  });
});

describe('theme CSS variables', () => {
  test('exposes every role under a prefix', () => {
    const theme = themeFor('c0ffee');
    const variables = themeVariables(theme);
    assert.equal(variables['--theme-muted-text'], theme.mutedText);
    assert.equal(variables['--theme-on-accent'], theme.onAccent);
    assert.equal(Object.keys(variables).length, Object.keys(theme).length);
    assert.equal(themeVariables(theme, '--era')['--era-focus-ring'], theme.focusRing);
  });

  test('renders a style attribute value', () => {
    const style = themeStyle('c0ffee');
    assert.match(style, /^--theme-base: #c0ffee; --theme-background: #[\da-f]{6}; /);
    assert.equal(style.split('; ').length, 9);
  });
});
