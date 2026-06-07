type Quality = 'Low Quality' | 'High Quality' | 'CD Quality' | 'Not Available' | 'Recording'
type AvailableLength = 'Full' | 'Snippet' | 'Confirmed' | 'Beat Only' | 'Partial' | 'Tagged' | 'OG File'
type Song = {
  era?: number;
  title?: string;
  notes?: string;
  trackLength?: number;
  fileDate?: Date;
  leakDate?: Date;
  availableLength?: AvailableLength
  quality?: Quality
}
