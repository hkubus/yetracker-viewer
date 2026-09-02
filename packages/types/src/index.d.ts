export type Quality = 'Low Quality' | 'High Quality' | 'CD Quality' | 'Lossless' | 'Not Available' | 'Recording';
export type AvailableLength =
  | 'Full'
  | 'Snippet'
  | 'Confirmed'
  | 'Beat Only'
  | 'Partial'
  | 'Tagged'
  | 'OG File'
  | 'Stem Bounce'
  | 'Rumored'
  | 'Conflicting Sources';
export type Song = {
  id?: number;
  eraId?: number;
  catalogId?: string;
  name?: string;
  notes?: string;
  trackLength?: number;
  fileDate?: number;
  leakDate?: number;
  url?: string;
  availableLength?: AvailableLength;
  quality?: Quality;
};
export type Era = {
  id: number;
  name: string;
  dominantColor: string;
  coverVersion?: string;
  notes: string;
  description: string;
  songsCount?: number;
};
