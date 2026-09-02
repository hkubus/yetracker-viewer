import type { Song } from '@yetracker/types';
import { atom } from 'nanostores';
export const queue = atom<Song[]>([]);
export const track = atom<Song>({});
export const playing = atom(false);
export const accent = atom('#000000');
