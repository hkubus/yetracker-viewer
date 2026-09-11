//! Exact port of `util/rankSongSearch.ts`, including the JS comparator's NaN
//! semantics (a NaN comparison is "not greater-or-equal", which the max-heap
//! path relies on) and its UTF-16 index arithmetic.

use std::cmp::Ordering;
use std::num::NonZeroUsize;
use std::sync::Mutex;

use lru::LruCache;

use crate::text;

const MAX_STRIP_CACHE: usize = 2000;

/// `⭐✨🏅🗑️🤖` with their category priorities (`🗑️` includes U+FE0F).
const CATEGORY_MARKERS: [(&str, i32); 5] = [("⭐", 0), ("✨", 1), ("🏅", 2), ("🗑️", 3), ("🤖", 4)];

pub trait Searchable {
    fn id(&self) -> i64;
    fn name(&self) -> Option<&str>;
    fn notes(&self) -> Option<&str>;
    fn quality(&self) -> Option<&str>;
    fn available_length(&self) -> Option<&str>;
    fn era_name(&self) -> Option<&str>;
    fn playable(&self) -> bool;
}

/// Bounded cache for the (expensive) category-marker stripping.
pub struct RankCache {
    strip: Mutex<LruCache<String, (String, i32)>>,
}

impl RankCache {
    pub fn new() -> Self {
        Self {
            strip: Mutex::new(LruCache::new(NonZeroUsize::new(MAX_STRIP_CACHE).expect("non-zero capacity"))),
        }
    }

    fn strip_category_markers(&self, value: Option<&str>) -> (String, i32) {
        let cache_key = value.unwrap_or("");
        let mut cache = self.strip.lock().expect("strip cache poisoned");
        if let Some(hit) = cache.get(cache_key) {
            return hit.clone();
        }

        let mut title = text::trim_start(cache_key).to_string();
        let mut category_priority = CATEGORY_MARKERS.len() as i32;
        loop {
            let mut found_marker = false;
            for (marker, priority) in CATEGORY_MARKERS {
                if title.starts_with(marker) {
                    category_priority = category_priority.min(priority);
                    found_marker = true;
                    title = text::trim_start(&title[marker.len()..]).to_string();
                    break;
                }
            }
            if !found_marker {
                break;
            }
        }

        let result = (title, category_priority);
        cache.put(cache_key.to_string(), result.clone());
        result
    }
}

impl Default for RankCache {
    fn default() -> Self {
        Self::new()
    }
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric()
}

/// `String#indexOf` position in UTF-16 code units.
fn utf16_index_of(value: &str, query: &str) -> Option<(usize, usize)> {
    let byte_index = value.find(query)?;
    Some((value[..byte_index].encode_utf16().count(), byte_index))
}

fn field_score(value: &str, query: &str, base: f64) -> f64 {
    let Some((position, byte_index)) = utf16_index_of(value, query) else {
        return f64::INFINITY;
    };

    let before = value[..byte_index].chars().next_back();
    let after = value[byte_index + query.len()..].chars().next();
    let starts_at_word = position == 0 || !before.map(is_word_char).unwrap_or(false);
    let ends_at_word = after.map(|character| !is_word_char(character)).unwrap_or(true);
    let length_difference = text::utf16_len(value).saturating_sub(text::utf16_len(query));
    let closeness = (position.min(99) as f64) / 100.0 + (length_difference.min(999) as f64) / 100_000.0;

    if value == query {
        return base;
    }
    if position == 0 {
        return base + 10.0 + closeness;
    }
    if starts_at_word && ends_at_word {
        return base + 20.0 + closeness;
    }
    if starts_at_word {
        return base + 30.0 + closeness;
    }
    base + 40.0 + closeness
}

fn split_parenthetical_text(value: Option<&str>) -> (String, String) {
    let mut depth: i32 = 0;
    let mut outside = String::new();
    let mut inside = String::new();

    for character in value.unwrap_or("").chars() {
        match character {
            '(' => {
                depth += 1;
                continue;
            }
            ')' => {
                depth = (depth - 1).max(0);
                continue;
            }
            _ => {}
        }
        if depth > 0 {
            inside.push(character);
        } else {
            outside.push(character);
        }
    }

    (text::normalize(&outside), text::normalize(&inside))
}

fn relevance_score<T: Searchable>(song: &T, query: &str, title_without_markers: &str) -> f64 {
    let title = text::normalize(title_without_markers);
    if title == query {
        return 0.0;
    }

    let (outside, inside) = split_parenthetical_text(Some(title_without_markers));
    let outside_score = field_score(&outside, query, 0.0);
    if outside_score.is_finite() {
        return outside_score;
    }
    let inside_score = field_score(&inside, query, 1_000.0);
    if inside_score.is_finite() {
        return inside_score;
    }
    let title_score = field_score(&title, query, 1_500.0);
    if title_score.is_finite() {
        return title_score;
    }
    let era_score = field_score(&text::normalize(song.era_name().unwrap_or("")), query, 2_000.0);
    if era_score.is_finite() {
        return era_score;
    }
    let notes_score = field_score(&text::normalize(song.notes().unwrap_or("")), query, 3_000.0);
    if notes_score.is_finite() {
        return notes_score;
    }
    let quality_score = field_score(&text::normalize(song.quality().unwrap_or("")), query, 4_000.0);
    if quality_score.is_finite() {
        return quality_score;
    }
    let availability_score = field_score(&text::normalize(song.available_length().unwrap_or("")), query, 4_100.0);
    if availability_score.is_finite() {
        return availability_score;
    }
    f64::INFINITY
}

/// Stand-in for `Intl.Collator(undefined, { sensitivity: 'base', numeric: true })`:
/// case-insensitive with numeric runs compared by value. Accent folding is not
/// reproduced (the reference implementation only uses this as a final
/// tie-breaker after relevance, category, playability and closeness).
fn collator_compare(left: &str, right: &str) -> Ordering {
    let left: Vec<char> = left.to_lowercase().chars().collect();
    let right: Vec<char> = right.to_lowercase().chars().collect();
    let (mut i, mut j) = (0usize, 0usize);

    while i < left.len() && j < right.len() {
        if left[i].is_numeric() && right[j].is_numeric() {
            let left_start = i;
            while i < left.len() && left[i].is_numeric() {
                i += 1;
            }
            let right_start = j;
            while j < right.len() && right[j].is_numeric() {
                j += 1;
            }
            let left_digits: String = left[left_start..i].iter().collect();
            let right_digits: String = right[right_start..j].iter().collect();
            let left_trimmed = left_digits.trim_start_matches('0');
            let right_trimmed = right_digits.trim_start_matches('0');
            let ordering = left_trimmed.len().cmp(&right_trimmed.len()).then_with(|| left_trimmed.cmp(right_trimmed));
            if ordering != Ordering::Equal {
                return ordering;
            }
            continue;
        }

        match left[i].cmp(&right[j]) {
            Ordering::Equal => {
                i += 1;
                j += 1;
            }
            other => return other,
        }
    }

    (left.len() - i).cmp(&(right.len() - j))
}

struct Ranked<T> {
    song: T,
    score: f64,
    category_priority: i32,
    normalized_title: String,
}

/// Mirrors the JS comparator, which returns `NaN` for two equally-ranked
/// infinite scores; callers must interpret that the way V8 does.
fn compare_ranked<T: Searchable>(left: &Ranked<T>, right: &Ranked<T>) -> f64 {
    let relevance_group_difference = left.score.trunc() - right.score.trunc();
    if relevance_group_difference != 0.0 {
        return relevance_group_difference;
    }

    if left.category_priority != right.category_priority {
        return (left.category_priority - right.category_priority) as f64;
    }

    if left.song.playable() != right.song.playable() {
        return if left.song.playable() { -1.0 } else { 1.0 };
    }

    let closeness_difference = left.score - right.score;
    if closeness_difference != 0.0 {
        return closeness_difference;
    }

    let title_difference = match collator_compare(&left.normalized_title, &right.normalized_title) {
        Ordering::Less => -1.0,
        Ordering::Greater => 1.0,
        Ordering::Equal => 0.0,
    };
    if title_difference != 0.0 {
        return title_difference;
    }
    (left.song.id() - right.song.id()) as f64
}

/// `Array.prototype.sort` coerces a `NaN` comparator result to `+0`.
fn compare_for_sort<T: Searchable>(left: &Ranked<T>, right: &Ranked<T>) -> Ordering {
    let result = compare_ranked(left, right);
    if result.is_nan() || result == 0.0 {
        Ordering::Equal
    } else if result < 0.0 {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

pub fn rank_song_search<T: Searchable>(songs: Vec<T>, query: &str, limit: usize, cache: &RankCache) -> Vec<T> {
    let mut ranked_songs: Vec<Ranked<T>> = songs
        .into_iter()
        .map(|song| {
            let (title, category_priority) = cache.strip_category_markers(song.name());
            let score = relevance_score(&song, query, &title);
            let normalized_title = text::normalize(song.name().unwrap_or(""));
            Ranked {
                song,
                score,
                category_priority,
                normalized_title,
            }
        })
        .collect();

    if limit < ranked_songs.len() {
        let mut heap: Vec<Ranked<T>> = Vec::with_capacity(limit);

        for ranked_song in ranked_songs {
            if heap.len() < limit {
                heap.push(ranked_song);
                // moveUp
                let mut index = heap.len() - 1;
                while index > 0 {
                    let parent_index = (index - 1) / 2;
                    if compare_ranked(&heap[parent_index], &heap[index]) >= 0.0 {
                        break;
                    }
                    heap.swap(parent_index, index);
                    index = parent_index;
                }
            } else if compare_ranked(&ranked_song, &heap[0]) < 0.0 {
                heap[0] = ranked_song;
                // moveDown
                let mut index = 0usize;
                loop {
                    let left_index = index * 2 + 1;
                    if left_index >= heap.len() {
                        break;
                    }
                    let right_index = left_index + 1;
                    let worse_child_index = if right_index < heap.len()
                        && compare_ranked(&heap[right_index], &heap[left_index]) > 0.0
                    {
                        right_index
                    } else {
                        left_index
                    };
                    if compare_ranked(&heap[worse_child_index], &heap[index]) <= 0.0 {
                        break;
                    }
                    heap.swap(index, worse_child_index);
                    index = worse_child_index;
                }
            }
        }

        heap.sort_by(compare_for_sort);
        return heap.into_iter().map(|ranked| ranked.song).collect();
    }

    ranked_songs.sort_by(compare_for_sort);
    ranked_songs.into_iter().map(|ranked| ranked.song).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Song {
        id: i64,
        name: Option<String>,
        notes: Option<String>,
        quality: Option<String>,
        available_length: Option<String>,
        era_name: Option<String>,
        playable: bool,
    }

    impl Song {
        fn new(id: i64, name: &str) -> Self {
            Song {
                id,
                name: Some(name.to_string()),
                notes: None,
                quality: None,
                available_length: None,
                era_name: None,
                playable: false,
            }
        }
    }

    impl Searchable for Song {
        fn id(&self) -> i64 {
            self.id
        }
        fn name(&self) -> Option<&str> {
            self.name.as_deref()
        }
        fn notes(&self) -> Option<&str> {
            self.notes.as_deref()
        }
        fn quality(&self) -> Option<&str> {
            self.quality.as_deref()
        }
        fn available_length(&self) -> Option<&str> {
            self.available_length.as_deref()
        }
        fn era_name(&self) -> Option<&str> {
            self.era_name.as_deref()
        }
        fn playable(&self) -> bool {
            self.playable
        }
    }

    #[test]
    fn exact_title_beats_substring_and_markers_are_stripped() {
        let cache = RankCache::new();
        let songs = vec![
            Song::new(1, "a love song"),
            Song::new(2, "⭐ Love"),
            Song::new(3, "Love"),
        ];
        let ranked = rank_song_search(songs, "love", 10, &cache);
        assert_eq!(ranked[0].id, 2, "⭐-prefixed exact title wins on category priority");
        assert_eq!(ranked[1].id, 3);
        assert_eq!(ranked[2].id, 1);
    }

    #[test]
    fn limit_uses_max_heap_and_stays_sorted() {
        let cache = RankCache::new();
        let songs: Vec<Song> = (0..20).map(|index| Song::new(index, &format!("song {index} love"))).collect();
        let ranked = rank_song_search(songs, "love", 5, &cache);
        assert_eq!(ranked.len(), 5);
        let ids: Vec<i64> = ranked.iter().map(|song| song.id).collect();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "ids stay ascending: {ids:?}");
    }

    #[test]
    fn non_matching_songs_rank_last() {
        let cache = RankCache::new();
        let songs = vec![Song::new(1, "nothing here"), Song::new(2, "love")];
        let ranked = rank_song_search(songs, "love", 10, &cache);
        assert_eq!(ranked[0].id, 2);
        assert_eq!(ranked[1].id, 1);
    }

    #[test]
    fn parenthetical_matches_rank_below_outside_matches() {
        let cache = RankCache::new();
        let songs = vec![Song::new(1, "song (love mix)"), Song::new(2, "love song")];
        let ranked = rank_song_search(songs, "love", 10, &cache);
        assert_eq!(ranked[0].id, 2);
        assert_eq!(ranked[1].id, 1);
    }

    #[test]
    fn collator_is_case_insensitive_and_numeric() {
        assert_eq!(collator_compare("Track 2", "track 10"), Ordering::Less);
        assert_eq!(collator_compare("ABC", "abc"), Ordering::Equal);
    }
}
