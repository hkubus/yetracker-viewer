export type Quality = 'Low Quality' | 'High Quality' | 'CD Quality' | 'Not Available' | 'Recording';
export type AvailableLength = 'Full' | 'Snippet' | 'Confirmed' | 'Beat Only' | 'Partial' | 'Tagged' | 'OG File';
export type Song = {
  id?: number;
  eraId?: number;
  name?: string;
  notes?: string;
  trackLength?: number;
  fileDate?: number;
  leakDate?: number;
  url?: string;
  availableLength?: AvailableLength;
  quality?: Quality;
};
