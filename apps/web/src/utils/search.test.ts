import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { clampQuery, fold, MAX_QUERY_LENGTH, matchesAllTokens, normalizeQuery, tokens } from './search.ts';

describe('fold', () => {
  test('examples from the API contract', () => {
    assert.equal(fold('⭐️ NEBRASKA [V4] (feat. JAŸ-Z)'), 'nebraska v4 feat jay z');
    assert.equal(fold('can’t'), 'cant');
  });

  test('drops accents via NFKD', () => {
    assert.equal(fold('Beyoncé'), 'beyonce');
    assert.equal(fold('JAŸ-Z'), 'jay z');
    assert.equal(fold('SHŌLZ'), 'sholz');
    assert.equal(fold('İstanbul'), 'istanbul');
  });

  test('spells out letters that have no decomposition', () => {
    assert.equal(fold('Ø Æther Œuvre Straße Łódź Đorđe Þór ı'), 'o aether oeuvre strasse lodz dorde thor i');
    assert.equal(fold('øæœłđþ'), 'oaeoeldth');
  });

  test('removes every kind of apostrophe, including ones NFKD rewrites', () => {
    for (const apostrophe of ["'", '’', '‘', 'ʼ', '`', '´', '＇', '｀']) {
      assert.equal(fold(`Can${apostrophe}t`), 'cant', `apostrophe U+${apostrophe.codePointAt(0)?.toString(16)}`);
    }
  });

  test('removes zero-width characters and variation selectors instead of splitting words', () => {
    assert.equal(fold('NE​BRA‌S‍KA'), 'nebraska');
    assert.equal(fold('﻿hello⁠world'), 'helloworld');
    assert.equal(fold('⭐️'), '');
    assert.equal(fold('✨︎ Special'), 'special');
  });

  test('turns punctuation and symbol runs into single spaces and trims', () => {
    assert.equal(fold('  a -- b  '), 'a b');
    assert.equal(fold('Hurricane (w/ The Weeknd & Lil Baby) [V12]'), 'hurricane w the weeknd lil baby v12');
    assert.equal(fold('line 1\nline 2\tend'), 'line 1 line 2 end');
    assert.equal(fold(''), '');
    assert.equal(fold('!!! ??? 🏆🗑️'), '');
  });

  test('applies compatibility mappings', () => {
    assert.equal(fold('２０２２'), '2022');
    assert.equal(fold('ﬁre'), 'fire');
    assert.equal(fold('Chapter Ⅳ'), 'chapter iv');
    assert.equal(fold('x²'), 'x2');
  });

  test('keeps non-Latin letters and digits', () => {
    assert.equal(fold('中村隆宏'), '中村隆宏');
    assert.equal(fold('ΑΒΓ Δ'), 'αβγ δ');
    assert.equal(fold('ホワイト'), 'ホワイト');
  });

  test('is idempotent', () => {
    for (const text of ['⭐️ NEBRASKA [V4] (feat. JAŸ-Z)', 'Łódź ＇quote＇', 'ΑΒΓ Δ ２０２２', 'デ ﬁ']) {
      assert.equal(fold(fold(text)), fold(text));
    }
  });
});

describe('tokens', () => {
  test('splits the folded query on spaces', () => {
    assert.deepEqual(tokens('  Nebraska   V4 '), ['nebraska', 'v4']);
    assert.deepEqual(tokens('JAŸ-Z'), ['jay', 'z']);
  });

  test('has no tokens for blank or punctuation-only queries', () => {
    assert.deepEqual(tokens(''), []);
    assert.deepEqual(tokens('   '), []);
    assert.deepEqual(tokens('?!'), []);
  });
});

describe('matchesAllTokens', () => {
  const haystack = fold('NEBRASKA [V4] (feat. Pusha T) OG Filename: nebraska_v4_final');

  test('requires every token as a substring, so prefixes match', () => {
    assert.equal(matchesAllTokens(haystack, tokens('nebr v4')), true);
    assert.equal(matchesAllTokens(haystack, tokens('PUSHA nebraska')), true);
    assert.equal(matchesAllTokens(haystack, tokens('nebraska v5')), false);
  });

  test('matches everything when there are no tokens', () => {
    assert.equal(matchesAllTokens(haystack, []), true);
    assert.equal(matchesAllTokens('', []), true);
  });
});

describe('normalizeQuery / clampQuery', () => {
  test('trims and collapses whitespace', () => {
    assert.equal(normalizeQuery('  a \n  b\t'), 'a b');
    assert.equal(normalizeQuery('a\r\nb\u000bc\u000cd'), 'a b c d');
  });

  test('drops control characters other than whitespace', () => {
    assert.equal(normalizeQuery('\u0000abc'), 'abc');
    assert.equal(normalizeQuery('a\u0000b'), 'ab');
    assert.equal(normalizeQuery('\u0000'), '');
    assert.equal(normalizeQuery('\u0000 \u0000 abc \u0000 def \u0000'), 'abc def');
    // C0 (including the separators U+001C–U+001F, which are not whitespace), DEL, and C1 (including NEL).
    assert.equal(normalizeQuery('\u0001\u0008\u000e\u001b\u001c\u001f\u007f\u0080\u0085\u009fdonda'), 'donda');
    // Neighbouring characters: whitespace, non-breaking space, emoji, letters with accents.
    assert.equal(normalizeQuery('\u00a0⭐\u0000 glory\u0007é'), '⭐ gloryé');
    assert.equal(clampQuery(new URLSearchParams('q=%00abc').get('q') ?? ''), 'abc');
    assert.deepEqual(tokens(normalizeQuery('can\u0000t stop')), ['cant', 'stop']);
  });

  test('keeps everything that is not a control character', () => {
    // Format characters such as the zero-width space are not control characters (fold() drops those).
    const text = 'Beyoncé ⭐️ [V4] (feat. JAŸ-Z) – 2:22 \u200bok';
    assert.equal(normalizeQuery(text), text);
  });

  test('counts the limit after dropping control characters', () => {
    assert.equal(clampQuery(`${'\u0000'.repeat(50)}${'x'.repeat(100)}`), 'x'.repeat(100));
  });

  test('keeps short queries unchanged', () => {
    assert.equal(clampQuery('  donda  2 '), 'donda 2');
  });

  test('cuts at the API limit in code points', () => {
    assert.equal(MAX_QUERY_LENGTH, 100);
    assert.equal(clampQuery('x'.repeat(150)), 'x'.repeat(100));
    const emoji = clampQuery('😀'.repeat(101));
    assert.equal(Array.from(emoji).length, 100);
    assert.equal(emoji, '😀'.repeat(100));
    assert.equal(clampQuery(`${'a'.repeat(99)} b`), 'a'.repeat(99));
    assert.equal(clampQuery('abcdef', 3), 'abc');
  });
});
