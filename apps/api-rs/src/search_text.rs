//! Text folding for search, the natural sort key and the category rank of a
//! song name.
//!
//! [`fold`] has a twin in the web client (`apps/web/src/utils/search.ts`): the
//! client folds what the user types and the API matches it against the stored
//! `songs.search_text`, so both must produce identical output. Change them
//! together.

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::text;

static SEPARATORS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").expect("valid separator regex"));
static DIGIT_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]+").expect("valid digit regex"));

/// Width that digit runs are zero-padded to in [`sort_title`].
const SORT_NUMBER_WIDTH: usize = 10;

/// A song category, marked in the sheet by an emoji in front of the title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SongCategory {
    /// Public filter value (`?category=`).
    pub id: &'static str,
    /// The marker's base code point (a U+FE0F variation selector may follow).
    pub marker: &'static str,
    /// Position in the `category` sort order.
    pub rank: i64,
}

/// Every category marker, in `category` sort order. Unmarked songs sort
/// between the wanted and the worst-of songs ([`UNMARKED_RANK`]).
pub const SONG_CATEGORIES: [SongCategory; 6] = [
    SongCategory {
        id: "best-of",
        marker: "⭐",
        rank: 0,
    },
    SongCategory {
        id: "special",
        marker: "✨",
        rank: 1,
    },
    SongCategory {
        id: "grails",
        marker: "🏆",
        rank: 2,
    },
    SongCategory {
        id: "wanted",
        marker: "🏅",
        rank: 3,
    },
    SongCategory {
        id: "worst-of",
        marker: "🗑",
        rank: 5,
    },
    SongCategory {
        id: "ai",
        marker: "🤖",
        rank: 6,
    },
];

/// `category_rank` of a song without a category marker.
pub const UNMARKED_RANK: i64 = 4;

fn is_apostrophe(character: char) -> bool {
    matches!(
        character,
        '\'' | '\u{2019}' | '\u{2018}' | '\u{02BC}' | '`' | '\u{00B4}'
    )
}

/// Zero-width characters, the word joiner, the BOM and the text/emoji
/// variation selectors.
fn is_invisible(character: char) -> bool {
    text::is_zero_width(character) || matches!(character, '\u{FE0E}' | '\u{FE0F}')
}

/// Letters without a Unicode decomposition to an ASCII base letter.
fn spelled_out(character: char) -> Option<&'static str> {
    Some(match character {
        'Ø' | 'ø' => "o",
        'Æ' | 'æ' => "ae",
        'Œ' | 'œ' => "oe",
        'ß' => "ss",
        'Ł' | 'ł' => "l",
        'Đ' | 'đ' => "d",
        'Þ' | 'þ' => "th",
        'ı' => "i",
        _ => return None,
    })
}

/// Folds text for accent-, case- and punctuation-insensitive matching:
/// `fold("⭐️ NEBRASKA [V4] (feat. JAŸ-Z)") == "nebraska v4 feat jay z"`.
///
/// Steps, in the same order as the web client: strip apostrophes (before NFKD,
/// which turns `´` into a space plus a combining accent); NFKD and drop
/// combining marks; spell out letters without a decomposition (Ø→o, Æ→ae,
/// Œ→oe, ß→ss, Ł→l, Đ→d, Þ→th, ı→i); lowercase (locale-independent); strip
/// apostrophes again (NFKD turns fullwidth ones into ASCII); drop zero-width
/// characters and variation selectors; turn every run of characters that are
/// neither letters nor digits into one space; trim.
pub fn fold(value: &str) -> String {
    let mut spelled = String::with_capacity(value.len());
    for character in value
        .chars()
        .filter(|&character| !is_apostrophe(character))
        .nfkd()
        .filter(|&character| !is_combining_mark(character))
    {
        match spelled_out(character) {
            Some(replacement) => spelled.push_str(replacement),
            None => spelled.push(character),
        }
    }
    let lowered: String = spelled
        .to_lowercase()
        .chars()
        .filter(|&character| !is_apostrophe(character) && !is_invisible(character))
        .collect();
    SEPARATORS
        .replace_all(&lowered, " ")
        .trim_matches(' ')
        .to_string()
}

/// The searchable fields of a song, folded into `songs.search_text`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchFields<'a> {
    pub name: &'a str,
    pub notes: Option<&'a str>,
    pub era_name: Option<&'a str>,
    pub era_subtitle: Option<&'a str>,
    pub sub_era: Option<&'a str>,
    pub quality: Option<&'a str>,
    pub available_length: Option<&'a str>,
}

/// `songs.search_text`: the folded fields joined by single spaces. A query
/// matches when each of its folded tokens is a substring of this text.
pub fn search_text_for(fields: &SearchFields<'_>) -> String {
    fold_joined([
        Some(fields.name),
        fields.notes,
        fields.era_name,
        fields.era_subtitle,
        fields.sub_era,
        fields.quality,
        fields.available_length,
    ])
}

/// `songs.song_search_text`: like [`search_text_for`] without the era's name
/// and subtitle, for searches scoped to one era (where every song would
/// match a token of the era's own name).
pub fn song_search_text_for(fields: &SearchFields<'_>) -> String {
    fold_joined([
        Some(fields.name),
        fields.notes,
        fields.sub_era,
        fields.quality,
        fields.available_length,
    ])
}

fn fold_joined<'a>(parts: impl IntoIterator<Item = Option<&'a str>>) -> String {
    let folded: Vec<String> = parts
        .into_iter()
        .flatten()
        .map(fold)
        .filter(|part| !part.is_empty())
        .collect();
    folded.join(" ")
}

/// Splits the category markers off the front of a title: returns the rest of
/// the title and the lowest rank among the markers (`None` when unmarked).
/// Whitespace and variation selectors between markers are skipped.
pub fn split_category_markers(title: &str) -> (&str, Option<i64>) {
    let mut rest = title;
    let mut rank: Option<i64> = None;
    loop {
        rest = rest.trim_start_matches(|character: char| {
            text::is_js_whitespace(character) || is_invisible(character)
        });
        let Some(category) = SONG_CATEGORIES
            .iter()
            .find(|category| rest.starts_with(category.marker))
        else {
            return (rest, rank);
        };
        rank = Some(rank.map_or(category.rank, |current| current.min(category.rank)));
        rest = &rest[category.marker.len()..];
    }
}

/// `songs.category_rank`: 0 best-of, 1 special, 2 grails, 3 wanted,
/// 4 unmarked, 5 worst-of, 6 AI. Markers are read from the front of the first
/// line; a song with several markers takes the best (lowest) rank.
pub fn category_rank(name: &str) -> i64 {
    split_category_markers(text::first_line(name))
        .1
        .unwrap_or(UNMARKED_RANK)
}

/// `songs.sort_title`, the key of `sort=name`: the first line without its
/// category markers, folded, with every digit run zero-padded to 10 digits so
/// that numbers compare numerically (`[V3] < [V9] < [V39]`).
pub fn sort_title(name: &str) -> String {
    let (title, _) = split_category_markers(text::first_line(name));
    let folded = fold(title);
    DIGIT_RUN
        .replace_all(&folded, |captures: &regex::Captures<'_>| {
            let digits = captures[0].trim_start_matches('0');
            format!("{digits:0>SORT_NUMBER_WIDTH$}")
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_examples() {
        assert_eq!(
            fold("⭐️ NEBRASKA [V4] (feat. JAŸ-Z)"),
            "nebraska v4 feat jay z"
        );
        assert_eq!(fold("can’t"), "cant");
    }

    // The cases below mirror apps/web/src/utils/search.test.ts so the two
    // implementations are checked against the same expectations.

    #[test]
    fn drops_accents_via_nfkd() {
        assert_eq!(fold("Beyoncé"), "beyonce");
        assert_eq!(fold("JAŸ-Z"), "jay z");
        assert_eq!(fold("SHŌLZ"), "sholz");
        assert_eq!(fold("İstanbul"), "istanbul");
    }

    #[test]
    fn spells_out_letters_without_a_decomposition() {
        assert_eq!(
            fold("Ø Æther Œuvre Straße Łódź Đorđe Þór ı"),
            "o aether oeuvre strasse lodz dorde thor i"
        );
        assert_eq!(fold("øæœłđþ"), "oaeoeldth");
    }

    #[test]
    fn removes_every_kind_of_apostrophe() {
        for apostrophe in ['\'', '’', '‘', 'ʼ', '`', '´', '＇', '｀'] {
            assert_eq!(
                fold(&format!("Can{apostrophe}t")),
                "cant",
                "apostrophe U+{:04X}",
                apostrophe as u32
            );
        }
    }

    #[test]
    fn removes_zero_width_characters_and_variation_selectors() {
        assert_eq!(fold("NE\u{200B}BRA\u{200C}S\u{200D}KA"), "nebraska");
        assert_eq!(fold("\u{FEFF}hello\u{2060}world"), "helloworld");
        assert_eq!(fold("⭐️"), "");
        assert_eq!(fold("✨\u{FE0E} Special"), "special");
    }

    #[test]
    fn turns_punctuation_and_symbol_runs_into_single_spaces() {
        assert_eq!(fold("  a -- b  "), "a b");
        assert_eq!(
            fold("Hurricane (w/ The Weeknd & Lil Baby) [V12]"),
            "hurricane w the weeknd lil baby v12"
        );
        assert_eq!(fold("line 1\nline 2\tend"), "line 1 line 2 end");
        assert_eq!(fold(""), "");
        assert_eq!(fold("!!! ??? 🏆🗑️"), "");
    }

    #[test]
    fn applies_compatibility_mappings() {
        assert_eq!(fold("２０２２"), "2022");
        assert_eq!(fold("ﬁre"), "fire");
        assert_eq!(fold("Chapter Ⅳ"), "chapter iv");
        assert_eq!(fold("x²"), "x2");
    }

    #[test]
    fn keeps_non_latin_letters_and_digits() {
        assert_eq!(fold("中村隆宏"), "中村隆宏");
        assert_eq!(fold("ΑΒΓ Δ"), "αβγ δ");
        assert_eq!(fold("ホワイト"), "ホワイト");
    }

    #[test]
    fn is_idempotent() {
        for value in [
            "⭐️ NEBRASKA [V4] (feat. JAŸ-Z)",
            "Łódź ＇quote＇",
            "ΑΒΓ Δ ２０２２",
            "デ ﬁ",
        ] {
            assert_eq!(fold(&fold(value)), fold(value), "{value}");
        }
    }

    #[test]
    fn folded_output_is_letters_digits_and_single_spaces() {
        let folded = fold("  ¥$ — “Vultures” (feat. Ty Dolla $ign)\n\n🤖  ");
        assert_eq!(folded, "vultures feat ty dolla ign");
        assert_eq!(fold("¥$"), "");
    }

    #[test]
    fn search_text_joins_every_searchable_field() {
        let text = search_text_for(&SearchFields {
            name: "⭐ NEBRASKA [V4]\n(feat. Pusha T)",
            notes: Some("OG Filename: nebraska_v4_final"),
            era_name: Some("DONDA 2 [V1]"),
            era_subtitle: Some("(Donda 2: 4 Da Kidz)"),
            sub_era: Some("2.22.22 Sessions"),
            quality: Some("CD Quality"),
            available_length: Some("Full"),
        });
        assert_eq!(
            text,
            "nebraska v4 feat pusha t og filename nebraska v4 final donda 2 v1 donda 2 4 da kidz \
             2 22 22 sessions cd quality full"
        );
        let sparse = search_text_for(&SearchFields {
            name: "???",
            quality: Some("Not Available"),
            ..SearchFields::default()
        });
        assert_eq!(sparse, "not available");
    }

    #[test]
    fn song_search_text_leaves_the_era_out() {
        let fields = SearchFields {
            name: "⭐ NEBRASKA [V4]\n(feat. Pusha T)",
            notes: Some("OG Filename: nebraska_v4_final"),
            era_name: Some("DONDA 2 [V1]"),
            era_subtitle: Some("(Donda 2: 4 Da Kidz)"),
            sub_era: Some("2.22.22 Sessions"),
            quality: Some("CD Quality"),
            available_length: Some("Full"),
        };
        assert_eq!(
            song_search_text_for(&fields),
            "nebraska v4 feat pusha t og filename nebraska v4 final 2 22 22 sessions cd quality full"
        );
        assert!(!song_search_text_for(&fields).contains("kidz"));
    }

    #[test]
    fn category_rank_reads_leading_markers() {
        assert_eq!(category_rank("⭐ Song"), 0);
        assert_eq!(category_rank("⭐️ Song"), 0);
        assert_eq!(category_rank("✨ Song"), 1);
        assert_eq!(category_rank("🏆 Song"), 2);
        assert_eq!(category_rank("🏅 Song"), 3);
        assert_eq!(category_rank("Song"), UNMARKED_RANK);
        assert_eq!(category_rank("🗑️ Song"), 5);
        assert_eq!(category_rank("🤖 Song"), 6);
        // Several markers: the best one wins.
        assert_eq!(category_rank("🗑️🤖 Song"), 5);
        assert_eq!(category_rank("✨🤖 Song"), 1);
        // Only the first line and only a leading marker count.
        assert_eq!(category_rank("Song ⭐\n⭐ credits"), UNMARKED_RANK);
        assert_eq!(category_rank("  \u{200B}🏆 Song"), 2);
    }

    #[test]
    fn sort_title_strips_markers_and_pads_numbers() {
        assert_eq!(
            sort_title("⭐️ NEBRASKA [V4]\n(feat. Pusha T)"),
            "nebraska v0000000004"
        );
        assert_eq!(sort_title("🗑️🤖 Song"), "song");
        let mut titles = ["Song [V39]", "Song [V9]", "song [v3]", "Song [V03]"]
            .map(sort_title)
            .to_vec();
        titles.sort();
        assert_eq!(
            titles,
            [
                "song v0000000003",
                "song v0000000003",
                "song v0000000009",
                "song v0000000039"
            ]
        );
        assert_eq!(sort_title("Beyoncé"), sort_title("BEYONCE"));
        assert_eq!(sort_title("???"), "");
    }
}
