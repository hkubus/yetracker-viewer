// Compile-time checks (run by `tsc`, never executed): documented response examples must satisfy the types,
// nullable fields must not be usable without a check, and removed v1 fields must be rejected.
import type { Era, SearchResponse, SearchSong, Song, StatusResponse } from '@yetracker/types';

export const era = {
  id: 31,
  position: 31,
  name: 'DONDA 2 [V1]',
  subtitle: null,
  notes: 'line 1\nline 2',
  description: '…',
  dominantColor: '666666',
  hasCover: false,
  coverVersion: null,
  songsCount: 956,
} satisfies Era;

export const song = {
  id: 6751,
  eraId: 31,
  eraPosition: 412,
  catalogId: 'unreleased',
  name: 'NEBRASKA [V4]\n(feat. Pusha T)\n(Alternate titles: Nebraska 2)',
  title: 'NEBRASKA [V4]',
  subEra: '2.22.22 Sessions',
  notes: 'OG Filename: nebraska_v4_final\nAlternate mix, bounced from…',
  notesLinks: [{ text: 'the Common vs. Kanye freestyle battle', url: 'https://imgur.gg/f/nhOhAwL' }],
  fileDate: 1645488000,
  fileDatePrecision: 'day',
  leakDate: 1509494400,
  leakDatePrecision: 'month',
  availableLength: 'Full',
  trackLength: 185,
  trackLengthApprox: false,
  quality: 'CD Quality',
  url: 'https://imgur.gg/f/abc123',
  links: ['https://imgur.gg/f/abc123', 'https://youtu.be/xyz'],
  downloadState: 'downloaded',
  playable: true,
  duration: 185.2,
} satisfies Song;

export const searchSong = {
  ...song,
  eraName: 'DONDA 2 [V1]',
  dominantColor: '666666',
  eraHasCover: true,
  eraCoverVersion: 'a9cf54ab9fba',
} satisfies SearchSong;

export const searchResponse = { songs: [searchSong], total: 2609, offset: 0, limit: 50 } satisfies SearchResponse;

export const status = {
  status: 'ok',
  lastImportAt: null,
  lastImportOk: null,
  lastImportError: null,
  eras: 43,
  songs: 9650,
  playableSongs: 7,
} satisfies StatusResponse;

export function nullableFieldsNeedChecks(value: Song): number {
  // @ts-expect-error dates are null when the sheet has none
  const leak: number = value.leakDate;
  // @ts-expect-error `url` is null when the song has no link
  const link: string = value.url;
  return leak + link.length;
}

export const withoutV1Fields: Song = {
  ...song,
  // @ts-expect-error the v1 `downloaded` flag was replaced by `downloadState`
  downloaded: 1,
};

// @ts-expect-error every key is always present (null when empty), so none may be left out
export const missingKeys: Era = { id: 1, name: 'x' };
