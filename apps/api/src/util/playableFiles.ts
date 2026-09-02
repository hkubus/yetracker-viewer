import { readdir, stat } from 'node:fs/promises';
import { basename, join } from 'node:path';
import { songsPath } from '../config.ts';

const playableFiles = new Set<string>();
const SCAN_CONCURRENCY = 32;

export async function refreshPlayableFiles() {
  const entries = await readdir(songsPath, { withFileTypes: true });
  const filenames = entries.filter((entry) => entry.isFile()).map((entry) => entry.name);
  const nextPlayableFiles = new Set<string>();
  let nextIndex = 0;

  async function worker() {
    while (nextIndex < filenames.length) {
      const filename = filenames[nextIndex++];
      try {
        const details = await stat(join(songsPath, filename));
        if (details.size > 0) nextPlayableFiles.add(filename);
      } catch {
        // Files can disappear during a scan; they simply remain unavailable.
      }
    }
  }

  await Promise.all(Array.from({ length: Math.min(SCAN_CONCURRENCY, filenames.length) }, () => worker()));
  playableFiles.clear();
  for (const filename of nextPlayableFiles) playableFiles.add(filename);
}

export function isSongPlayable(filename: string | null) {
  return filename !== null && basename(filename) === filename && playableFiles.has(filename);
}

export function setSongPlayable(filename: string, playable: boolean) {
  if (basename(filename) !== filename) return;
  if (playable) playableFiles.add(filename);
  else playableFiles.delete(filename);
}

export async function refreshSongPlayable(filename: string) {
  if (basename(filename) !== filename) return false;
  try {
    const details = await stat(join(songsPath, filename));
    const playable = details.isFile() && details.size > 0;
    setSongPlayable(filename, playable);
    return playable;
  } catch {
    setSongPlayable(filename, false);
    return false;
  }
}
