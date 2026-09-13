export type SongCategory = {
  id: string;
  label: string;
  emoji: string;
};

export const SONG_CATEGORIES: SongCategory[] = [
  { id: 'best-of', label: 'Best Of', emoji: '⭐' },
  { id: 'special', label: 'Special', emoji: '✨' },
  { id: 'grails', label: 'Grails', emoji: '🏆' },
  { id: 'wanted', label: 'Wanted', emoji: '🏅' },
  { id: 'worst-of', label: 'Worst Of', emoji: '🗑️' },
  { id: 'ai', label: 'AI', emoji: '🤖' },
];

export function songCategoryIds(): string[] {
  return SONG_CATEGORIES.map((category) => category.id);
}
