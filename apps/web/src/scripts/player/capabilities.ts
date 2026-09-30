/** Browser capabilities the player adapts to. */

/** Transcoded streams are Ogg Opus; browsers that can't play it (older Safari) only get the original file. */
export function canPlayOggOpus(): boolean {
  try {
    return document.createElement('audio').canPlayType('audio/ogg; codecs=opus') !== '';
  } catch {
    return false;
  }
}

/** iOS ignores `volume` on media elements (it always reads 1: hardware buttons only), so no slider there. */
export function canSetVolume(): boolean {
  try {
    const probe = document.createElement('audio');
    probe.volume = 0.5;
    return Math.abs(probe.volume - 0.5) < 0.01;
  } catch {
    return false;
  }
}
