import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { splitCredits } from './songTitle.ts';

describe('splitCredits', () => {
  test('splits trailing credits from the name', () => {
    assert.deepEqual(splitCredits('✨ Stronger [V5] (prod. Kanye West, MIKE DEAN & Timbaland)'), {
      main: '✨ Stronger [V5]',
      credits: '(prod. Kanye West, MIKE DEAN & Timbaland)',
    });
    assert.deepEqual(splitCredits('PABLO [V3] (feat. Future) (prod. ATL Jacob & $crim)'), {
      main: 'PABLO [V3]',
      credits: '(feat. Future) (prod. ATL Jacob & $crim)',
    });
    assert.deepEqual(splitCredits('Stronger (Albe Back Remix) (feat. Albe Back) (prod. Kanye West)'), {
      main: 'Stronger (Albe Back Remix)',
      credits: '(feat. Albe Back) (prod. Kanye West)',
    });
    assert.deepEqual(splitCredits('Hurricane [V2] (ref. The-Dream) (with KayCyy)'), {
      main: 'Hurricane [V2]',
      credits: '(ref. The-Dream) (with KayCyy)',
    });
    assert.deepEqual(splitCredits('Up From The Ashes (With Ty Dolla $ign)'), {
      main: 'Up From The Ashes',
      credits: '(With Ty Dolla $ign)',
    });
  });

  test('keeps titles without credits whole', () => {
    for (const title of [
      '🏅 Graduation (Skit)',
      'Unknown [50 Cent Collaborations]',
      '??? (¥$)',
      'Without You (prod.Kanye West)',
      '(prod. Kanye West)',
      '',
    ]) {
      assert.deepEqual(splitCredits(title), { main: title, credits: '' }, title);
    }
  });

  test('trims the parts', () => {
    assert.deepEqual(splitCredits('  Flowers   (feat. Ye)  '), { main: 'Flowers', credits: '(feat. Ye)' });
  });
});
