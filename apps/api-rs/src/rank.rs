//! Relevance ranking for `GET /songs?q=`.
//!
//! Everything is compared in folded form ([`fold`]): the query's folded
//! phrase is looked up in the folded fields of every matching song, and the
//! song gets a [`RankKey`]. Keys order by, in this priority:
//!
//! 1. the field the phrase was found in ([`Tier`]): the title line outside
//!    parentheses, then the rest of the name (parenthetical parts, credits and
//!    alternate-title lines), phrases spanning the title line or the name, the
//!    era (name, subtitle, sub-era), the notes, quality/availability, and last
//!    songs that only contain the query's tokens scattered across fields;
//! 2. how it matched ([`Kind`]): the whole field (for titles also ignoring a
//!    trailing `[V2]`-style tag and a leading `Artist - `), a prefix ending at
//!    a word boundary, a whole word, a word start, or anywhere;
//! 3. where it matched (earlier is better);
//! 4. the song's category: best-of, special, grails, wanted, unmarked,
//!    worst-of, AI (`songs.category_rank`), so a category never outranks a
//!    better match;
//! 5. the field's length beyond the phrase (tighter is better);
//! 6. playable songs first;
//! 7. catalog position, which makes the order total and stable.
//!
//! The folded texts of the whole catalog are kept in a [`CatalogIndex`],
//! built once per catalog version, so a search only fetches the ids of its
//! matches from SQLite.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::search_text::fold;
use crate::text;

/// Field a phrase was found in, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    /// The title line outside parentheses: `NEBRASKA [V4]`.
    Title,
    /// Parenthetical parts of the title line and the other name lines
    /// (credits, alternate titles).
    NameRest,
    /// Phrase spanning the title line's parentheses.
    TitleLine,
    /// Phrase spanning several name lines.
    Name,
    /// Era name, era subtitle or sub-era.
    Era,
    Notes,
    /// Quality or available length.
    Details,
    /// Every token matched, but not as one phrase in one field.
    Scattered,
    /// Not in the index (added to the catalog after the index was built).
    Unknown,
}

/// How a phrase matched a field, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Exact,
    /// At the start, followed by a word boundary.
    Prefix,
    /// A whole word (or words) further in.
    Word,
    /// The start of a word, but not its end (`breath` in `breathe`).
    WordStart,
    Substring,
}

/// Sort key of one search result; smaller keys rank first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RankKey {
    tier: Tier,
    /// [`Kind`] for phrase matches; for scattered matches the worst field
    /// tier any token needed.
    kind: u8,
    /// Byte offset of the match; for scattered matches the sum of the
    /// tokens' field tiers.
    offset: u32,
    category: i64,
    /// Field length beyond the phrase, in bytes.
    slack: u32,
    /// `false` (playable) sorts first.
    unplayable: bool,
    position: i64,
}

/// A search query in folded form. `None` from [`SearchQuery::new`] when the
/// query has no letters or digits, i.e. nothing to match or rank by.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    phrase: String,
    tokens: Vec<String>,
}

impl SearchQuery {
    pub fn new(query: &str) -> Option<Self> {
        let phrase = fold(query);
        if phrase.is_empty() {
            return None;
        }
        let mut tokens: Vec<String> = Vec::new();
        for token in phrase.split(' ') {
            if !tokens.iter().any(|seen| seen == token) {
                tokens.push(token.to_string());
            }
        }
        Some(Self { phrase, tokens })
    }

    /// The distinct tokens; a song matches when its `search_text` contains
    /// every one of them.
    pub fn tokens(&self) -> &[String] {
        &self.tokens
    }
}

/// Folded views of a song name.
#[derive(Debug, Clone, Default)]
struct NameProfile {
    /// The whole name, as it starts `songs.search_text`.
    name: String,
    /// The title line.
    title_line: String,
    /// The title line outside parentheses.
    title: String,
    /// [`Self::title`] without trailing bracket tags (`[V2]`, `[V3-V5]`,
    /// `[Clean]`): what an exact title match compares with.
    bare_title: String,
    /// The part of [`Self::bare_title`] after a leading `Artist - `, with its
    /// byte offset in [`Self::title`].
    titled_part: Option<(u32, String)>,
    /// Parenthetical text of the title line plus every other name line.
    rest: String,
}

impl NameProfile {
    fn new(name: &str) -> Self {
        let mut lines = name.split('\n');
        let title_line = lines.next().unwrap_or_default();
        let (outside, inside) = split_parentheses(title_line);
        let bare = strip_trailing_tags(&outside);
        let title = fold(&outside);
        let split = bare
            .split_once(" - ")
            .map(|(artist, part)| (fold(artist), fold(part)))
            .filter(|(artist, _)| !artist.is_empty());
        let bare_title = match (&split, fold(bare)) {
            // `Artist - ???`: an unknown title; the artist alone is no exact
            // title match.
            (Some((_, part)), _) if part.is_empty() => title.clone(),
            (_, folded) if folded.is_empty() => title.clone(),
            (_, folded) => folded,
        };
        let titled_part = split
            .filter(|(_, part)| !part.is_empty())
            .map(|(artist, part)| (u32::try_from(artist.len() + 1).unwrap_or(u32::MAX), part));
        let mut rest_parts = vec![fold(&inside)];
        rest_parts.extend(lines.map(fold));
        rest_parts.retain(|part| !part.is_empty());
        Self {
            name: fold(name),
            title_line: fold(title_line),
            title,
            bare_title,
            titled_part,
            rest: rest_parts.join(" "),
        }
    }
}

/// Folded era texts, shared by every song of the era.
#[derive(Debug, Clone, Default)]
struct EraProfile {
    name: String,
    bare_name: String,
    subtitle: String,
}

impl EraProfile {
    fn new(name: &str, subtitle: Option<&str>) -> Self {
        let folded = fold(name);
        let bare_name = match fold(strip_trailing_tags(name)) {
            bare if bare.is_empty() => folded.clone(),
            bare => bare,
        };
        Self {
            name: folded,
            bare_name,
            subtitle: subtitle.map(fold).unwrap_or_default(),
        }
    }
}

/// The stored texts of one song, as `songs` holds them.
#[derive(Debug, Clone, Copy, Default)]
pub struct SongTexts<'a> {
    pub name: &'a str,
    /// `songs.search_text`: the folded name, notes, era name, era subtitle,
    /// sub-era, quality and available length, each non-empty part joined by
    /// a space.
    pub search_text: &'a str,
    pub era: Option<i64>,
    pub sub_era: Option<&'a str>,
    pub quality: Option<&'a str>,
    pub available_length: Option<&'a str>,
    pub category_rank: i64,
    pub position: i64,
}

/// One song of a [`CatalogIndex`], every text folded.
#[derive(Debug, Clone)]
struct IndexedSong {
    name: NameProfile,
    notes: String,
    era: Option<i64>,
    sub_era: Arc<str>,
    quality: Arc<str>,
    available_length: Arc<str>,
    category_rank: i64,
    position: i64,
}

/// The folded texts of the catalog, for ranking search matches. Tagged with
/// the catalog version it was built from (`meta.last_sheet_sha256`).
#[derive(Debug, Default)]
pub struct CatalogIndex {
    version: Option<String>,
    eras: HashMap<i64, EraProfile>,
    songs: HashMap<i64, IndexedSong>,
    /// Folded short values (sub-eras, qualities, lengths) shared by songs.
    folded: HashMap<String, Arc<str>>,
}

impl CatalogIndex {
    pub fn new(version: Option<String>) -> Self {
        Self {
            version,
            ..Self::default()
        }
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Adds an era; add every era before its songs.
    pub fn add_era(&mut self, id: i64, name: &str, subtitle: Option<&str>) {
        self.eras.insert(id, EraProfile::new(name, subtitle));
    }

    fn fold_shared(&mut self, value: Option<&str>) -> Arc<str> {
        let value = value.unwrap_or_default();
        if let Some(folded) = self.folded.get(value) {
            return folded.clone();
        }
        let folded: Arc<str> = fold(value).into();
        self.folded.insert(value.to_string(), folded.clone());
        folded
    }

    pub fn add_song(&mut self, id: i64, song: &SongTexts<'_>) {
        let name = NameProfile::new(song.name);
        let sub_era = self.fold_shared(song.sub_era);
        let quality = self.fold_shared(song.quality);
        let available_length = self.fold_shared(song.available_length);
        let (era_name, era_subtitle) = song
            .era
            .and_then(|era| self.eras.get(&era))
            .map_or(("", ""), |era| (era.name.as_str(), era.subtitle.as_str()));
        let notes = notes_of(
            song.search_text,
            &name.name,
            &[
                &available_length,
                &quality,
                &sub_era,
                era_subtitle,
                era_name,
            ],
        )
        .to_string();
        self.songs.insert(
            id,
            IndexedSong {
                name,
                notes,
                era: song.era,
                sub_era,
                quality,
                available_length,
                category_rank: song.category_rank,
                position: song.position,
            },
        );
    }

    /// Orders search matches, given as `(song id, playable)`, best first and
    /// returns the indexes of the first `keep` of them. Songs missing from the
    /// index (added by an import after it was built) rank last.
    pub fn rank(&self, query: &SearchQuery, matches: &[(i64, bool)], keep: usize) -> Vec<usize> {
        let mut keys: Vec<(RankKey, usize)> = matches
            .iter()
            .enumerate()
            .map(|(index, &(id, playable))| {
                let key = match self.songs.get(&id) {
                    Some(song) => rank_key(
                        query,
                        &Candidate {
                            name: &song.name,
                            notes: &song.notes,
                            era: song.era.and_then(|era| self.eras.get(&era)),
                            sub_era: &song.sub_era,
                            quality: &song.quality,
                            available_length: &song.available_length,
                            category_rank: song.category_rank,
                            playable,
                            position: song.position,
                        },
                    ),
                    None => RankKey {
                        tier: Tier::Unknown,
                        kind: 0,
                        offset: 0,
                        category: 0,
                        slack: 0,
                        unplayable: !playable,
                        position: id,
                    },
                };
                (key, index)
            })
            .collect();
        if keep < keys.len() {
            if keep == 0 {
                return Vec::new();
            }
            keys.select_nth_unstable(keep - 1);
            keys.truncate(keep);
        }
        keys.sort_unstable();
        keys.into_iter().map(|(_, index)| index).collect()
    }
}

/// The folded notes, cut out of `search_text` between the folded name and
/// the folded tail fields (`tail` in reverse order: available length,
/// quality, sub-era, era subtitle, era name). When the stored text doesn't
/// line up (a row written by an older version), everything after the name
/// counts as notes, which can only make a notes match rank as one.
fn notes_of<'a>(search_text: &'a str, name: &str, tail: &[&str]) -> &'a str {
    let text = match search_text.strip_prefix(name) {
        Some(rest) if !name.is_empty() => rest.strip_prefix(' ').unwrap_or(rest),
        _ => search_text,
    };
    let mut notes = text;
    for part in tail.iter().filter(|part| !part.is_empty()) {
        let Some(rest) = notes.strip_suffix(part) else {
            return text;
        };
        notes = if rest.is_empty() {
            rest
        } else {
            match rest.strip_suffix(' ') {
                Some(rest) => rest,
                None => return text,
            }
        };
    }
    notes
}

/// What ranking needs to know about one matching song; every text folded.
#[derive(Debug, Clone, Copy)]
struct Candidate<'a> {
    name: &'a NameProfile,
    notes: &'a str,
    era: Option<&'a EraProfile>,
    sub_era: &'a str,
    quality: &'a str,
    available_length: &'a str,
    category_rank: i64,
    playable: bool,
    position: i64,
}

fn rank_key(query: &SearchQuery, candidate: &Candidate<'_>) -> RankKey {
    let key = |tier: Tier, (kind, offset, slack): (Kind, u32, u32)| RankKey {
        tier,
        kind: kind as u8,
        offset,
        category: candidate.category_rank,
        slack,
        unplayable: !candidate.playable,
        position: candidate.position,
    };
    let phrase = query.phrase.as_str();
    let name = candidate.name;

    if let Some(found) = title_match(name, phrase) {
        return key(Tier::Title, found);
    }
    for (tier, field) in [
        (Tier::NameRest, name.rest.as_str()),
        (Tier::TitleLine, name.title_line.as_str()),
        (Tier::Name, name.name.as_str()),
    ] {
        if let Some(found) = find(field, phrase) {
            return key(tier, found);
        }
    }
    if let Some(found) = era_match(candidate, phrase) {
        return key(Tier::Era, found);
    }
    if let Some(found) = find(candidate.notes, phrase) {
        return key(Tier::Notes, found);
    }
    if let Some(found) =
        best([candidate.quality, candidate.available_length].map(|field| find(field, phrase)))
    {
        return key(Tier::Details, found);
    }

    // The tokens are spread over several fields: rank by the worst field any
    // token needed, then by all of them together.
    let token_tiers: Vec<Tier> = query
        .tokens
        .iter()
        .map(|token| token_tier(candidate, token))
        .collect();
    RankKey {
        tier: Tier::Scattered,
        kind: token_tiers.iter().max().map_or(0, |tier| *tier as u8),
        offset: token_tiers.iter().map(|tier| *tier as u32).sum(),
        category: candidate.category_rank,
        slack: 0,
        unplayable: !candidate.playable,
        position: candidate.position,
    }
}

fn title_match(name: &NameProfile, phrase: &str) -> Option<(Kind, u32, u32)> {
    if name.bare_title == phrase || name.title == phrase {
        return Some((Kind::Exact, 0, 0));
    }
    let titled = name.titled_part.as_ref().and_then(|(offset, part)| {
        if part == phrase {
            Some((Kind::Exact, *offset, 0))
        } else if starts_with_word(part, phrase) {
            Some((Kind::Prefix, *offset, slack(part, phrase)))
        } else {
            None
        }
    });
    best([titled, find(&name.title, phrase)])
}

fn era_match(candidate: &Candidate<'_>, phrase: &str) -> Option<(Kind, u32, u32)> {
    let era = candidate.era;
    if era.is_some_and(|era| era.bare_name == phrase || era.name == phrase) {
        return Some((Kind::Exact, 0, 0));
    }
    let (name, subtitle) = era.map_or(("", ""), |era| (era.name.as_str(), era.subtitle.as_str()));
    best([name, subtitle, candidate.sub_era].map(|field| find(field, phrase)))
}

/// The best field tier that contains `token` (tokens have no spaces, so a
/// token never spans two fields).
fn token_tier(candidate: &Candidate<'_>, token: &str) -> Tier {
    let name = candidate.name;
    let era = candidate.era;
    if name.title_line.contains(token) {
        Tier::Title
    } else if name.rest.contains(token) {
        Tier::NameRest
    } else if era.is_some_and(|era| era.name.contains(token) || era.subtitle.contains(token))
        || candidate.sub_era.contains(token)
    {
        Tier::Era
    } else if candidate.notes.contains(token) {
        Tier::Notes
    } else if candidate.quality.contains(token) || candidate.available_length.contains(token) {
        Tier::Details
    } else {
        Tier::Scattered
    }
}

fn best<const N: usize>(found: [Option<(Kind, u32, u32)>; N]) -> Option<(Kind, u32, u32)> {
    found.into_iter().flatten().min()
}

/// The best occurrence of `phrase` in the folded `field`: `(kind, byte
/// offset, slack)`. Folded text is letters, digits and single spaces, so a
/// space (or either end) is a word boundary.
fn find(field: &str, phrase: &str) -> Option<(Kind, u32, u32)> {
    if phrase.is_empty() || field.len() < phrase.len() {
        return None;
    }
    if field == phrase {
        return Some((Kind::Exact, 0, 0));
    }
    let bytes = field.as_bytes();
    let mut best: Option<(Kind, usize)> = None;
    let mut from = 0;
    while let Some(found) = field[from..].find(phrase) {
        let start = from + found;
        let end = start + phrase.len();
        let starts_word = start == 0 || bytes[start - 1] == b' ';
        let ends_word = end == field.len() || bytes[end] == b' ';
        let kind = match (starts_word, ends_word) {
            (true, true) if start == 0 => Kind::Prefix,
            (true, true) => Kind::Word,
            (true, false) => Kind::WordStart,
            (false, _) => Kind::Substring,
        };
        if best.is_none_or(|(best_kind, _)| kind < best_kind) {
            best = Some((kind, start));
        }
        if kind <= Kind::Word {
            break;
        }
        // Step one character, not one byte: overlapping occurrences count.
        from = start + field[start..].chars().next().map_or(1, char::len_utf8);
    }
    best.map(|(kind, start)| {
        (
            kind,
            u32::try_from(start).unwrap_or(u32::MAX),
            slack(field, phrase),
        )
    })
}

fn starts_with_word(field: &str, phrase: &str) -> bool {
    field.starts_with(phrase) && field.as_bytes().get(phrase.len()) == Some(&b' ')
}

fn slack(field: &str, phrase: &str) -> u32 {
    u32::try_from(field.len().saturating_sub(phrase.len())).unwrap_or(u32::MAX)
}

/// Splits a line into its text outside and inside parentheses (nested groups
/// count as inside; an unclosed group runs to the end of the line).
fn split_parentheses(line: &str) -> (String, String) {
    let mut depth = 0usize;
    let mut outside = String::with_capacity(line.len());
    let mut inside = String::new();
    for character in line.chars() {
        match character {
            '(' => {
                depth += 1;
                inside.push(' ');
            }
            ')' => {
                depth = depth.saturating_sub(1);
                outside.push(' ');
            }
            _ if depth > 0 => inside.push(character),
            _ => outside.push(character),
        }
    }
    (outside, inside)
}

/// `text` without trailing `[…]` groups: `Glory [V20-V21]` → `Glory`,
/// `Evolve [V7] [Clean]` → `Evolve`.
fn strip_trailing_tags(text: &str) -> &str {
    let mut rest = text::trim(text);
    while let Some(open) = rest.strip_suffix(']').and_then(|inner| inner.rfind('[')) {
        rest = text::trim(&rest[..open]);
    }
    rest
}

/// The current [`CatalogIndex`], shared by all searches.
pub struct RankCache {
    index: Mutex<Option<Arc<CatalogIndex>>>,
}

impl RankCache {
    pub fn new() -> Self {
        Self {
            index: Mutex::new(None),
        }
    }

    /// The index of catalog `version`, built with `build` when the cached
    /// one is missing or of another version. Concurrent callers wait for a
    /// single build.
    pub fn index<E>(
        &self,
        version: Option<&str>,
        build: impl FnOnce() -> Result<CatalogIndex, E>,
    ) -> Result<Arc<CatalogIndex>, E> {
        let mut current = self.index.lock().expect("rank cache poisoned");
        if let Some(index) = current.as_ref()
            && index.version() == version
        {
            return Ok(index.clone());
        }
        let index = Arc::new(build()?);
        *current = Some(index.clone());
        Ok(index)
    }
}

impl Default for RankCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search_text::{SearchFields, category_rank, search_text_for};

    /// A song as the ranking sees it, built the way the importer stores it.
    struct Song {
        id: i64,
        name: String,
        notes: Option<String>,
        era: usize,
        sub_era: Option<String>,
        quality: Option<String>,
        available_length: Option<String>,
        playable: bool,
    }

    fn song(id: i64, name: &str) -> Song {
        Song {
            id,
            name: name.to_string(),
            notes: None,
            era: 0,
            sub_era: None,
            quality: None,
            available_length: None,
            playable: false,
        }
    }

    impl Song {
        fn era(mut self, era: usize) -> Self {
            self.era = era;
            self
        }
        fn notes(mut self, notes: &str) -> Self {
            self.notes = Some(notes.to_string());
            self
        }
        fn sub_era(mut self, sub_era: &str) -> Self {
            self.sub_era = Some(sub_era.to_string());
            self
        }
        fn quality(mut self, quality: &str) -> Self {
            self.quality = Some(quality.to_string());
            self
        }
        fn playable(mut self) -> Self {
            self.playable = true;
            self
        }

        fn search_text(&self) -> String {
            let (era_name, era_subtitle) = ERAS[self.era];
            search_text_for(&SearchFields {
                name: &self.name,
                notes: self.notes.as_deref(),
                era_name: Some(era_name),
                era_subtitle,
                sub_era: self.sub_era.as_deref(),
                quality: self.quality.as_deref(),
                available_length: self.available_length.as_deref(),
            })
        }
    }

    const ERAS: [(&str, Option<&str>); 5] = [
        ("Graduation", None),
        ("JESUS IS KING", Some("(Christ Jesus)")),
        ("DONDA [V1]", Some("(DONDA: WITH CHILD, God's Country)")),
        ("The Life Of Pablo", Some("(So Help Me God, SWISH, WAVES)")),
        ("DONDA 2 [V2]", Some("(PABLO 2)")),
    ];

    fn index_of(songs: &[Song]) -> CatalogIndex {
        let mut index = CatalogIndex::new(Some("v1".to_string()));
        for (era, (name, subtitle)) in ERAS.iter().enumerate() {
            index.add_era(era as i64, name, *subtitle);
        }
        for (position, song) in songs.iter().enumerate() {
            let search_text = song.search_text();
            index.add_song(
                song.id,
                &SongTexts {
                    name: &song.name,
                    search_text: &search_text,
                    era: Some(song.era as i64),
                    sub_era: song.sub_era.as_deref(),
                    quality: song.quality.as_deref(),
                    available_length: song.available_length.as_deref(),
                    category_rank: category_rank(&song.name),
                    position: position as i64 + 1,
                },
            );
        }
        index
    }

    /// Ids of the songs matching every token of `query` (the search's SQL
    /// filter), best first; songs are positioned in the order given.
    fn rank(query: &str, songs: &[Song]) -> Vec<i64> {
        let query = SearchQuery::new(query).expect("query has tokens");
        let index = index_of(songs);
        let matches: Vec<(i64, bool)> = songs
            .iter()
            .filter(|song| {
                let search_text = song.search_text();
                query
                    .tokens()
                    .iter()
                    .all(|token| search_text.contains(token.as_str()))
            })
            .map(|song| (song.id, song.playable))
            .collect();
        index
            .rank(&query, &matches, matches.len())
            .into_iter()
            .map(|position| matches[position].0)
            .collect()
    }

    #[test]
    fn find_classifies_matches() {
        assert_eq!(find("glory", "glory"), Some((Kind::Exact, 0, 0)));
        assert_eq!(find("glory v4", "glory"), Some((Kind::Prefix, 0, 3)));
        assert_eq!(find("the glory v2", "glory"), Some((Kind::Word, 4, 7)));
        assert_eq!(find("breathe", "breath"), Some((Kind::WordStart, 0, 1)));
        assert_eq!(find("gcshit v1", "hit"), Some((Kind::Substring, 3, 6)));
        // The best occurrence wins, not the first one.
        assert_eq!(
            find("looking for the king", "king"),
            Some((Kind::Word, 16, 16))
        );
        // Overlapping occurrences are found.
        assert_eq!(find("aab", "ab"), Some((Kind::Substring, 1, 1)));
        assert_eq!(find("gl", "glory"), None);
        assert_eq!(find("anything", ""), None);
    }

    #[test]
    fn name_profiles_split_the_title() {
        let profile = NameProfile::new(
            "⭐️ Our King (Trust) [V2]\n(feat. Pusha T) (prod. Kanye West)\n(Alternate titles: Nebraska 2)",
        );
        assert_eq!(profile.title, "our king v2");
        assert_eq!(profile.bare_title, "our king");
        assert_eq!(profile.title_line, "our king trust v2");
        assert_eq!(
            profile.rest,
            "trust feat pusha t prod kanye west alternate titles nebraska 2"
        );
        assert_eq!(profile.titled_part, None);

        let featured = NameProfile::new("Frank Ocean - White Ferrari [V1-V??]");
        assert_eq!(featured.bare_title, "frank ocean white ferrari");
        assert_eq!(
            featured.titled_part,
            Some((12, "white ferrari".to_string()))
        );

        let unknown = NameProfile::new("JAŸ-Z - ??? [V2]");
        assert_eq!(unknown.bare_title, "jay z v2");
        assert_eq!(unknown.titled_part, None);

        assert_eq!(strip_trailing_tags("Evolve [V7] [Clean]"), "Evolve");
        assert_eq!(strip_trailing_tags("[Untitled]"), "");
        assert_eq!(NameProfile::new("[Untitled]").bare_title, "untitled");
    }

    #[test]
    fn notes_are_cut_out_of_the_search_text() {
        let text = search_text_for(&SearchFields {
            name: "Song [V1]\n(feat. X)",
            notes: Some("OG Filename: song v1"),
            era_name: Some("DONDA 2 [V1]"),
            era_subtitle: Some("(War)"),
            sub_era: Some("Sessions"),
            quality: Some("CD Quality"),
            available_length: Some("Full"),
        });
        let name = "song v1 feat x";
        let tail = ["full", "cd quality", "sessions", "war", "donda 2 v1"];
        assert_eq!(notes_of(&text, name, &tail), "og filename song v1");
        let without_notes = "song v1 feat x donda 2 v1 war sessions cd quality full";
        assert_eq!(notes_of(without_notes, name, &tail), "");
        assert_eq!(
            notes_of("song v1 feat x something else", name, &tail),
            "something else"
        );
        // A name that folds to nothing is not part of the text.
        assert_eq!(notes_of("notes here full", "", &["full"]), "notes here");
    }

    #[test]
    fn query_tokens_are_folded_and_distinct() {
        let query = SearchQuery::new("  Donda  DONDA [V2] ").unwrap();
        assert_eq!(query.phrase, "donda donda v2");
        assert_eq!(query.tokens(), ["donda", "v2"]);
        assert!(SearchQuery::new("⭐ ???").is_none());
        assert!(SearchQuery::new("").is_none());
    }

    #[test]
    fn glory_exact_titles_ignore_version_tags_and_variation_selectors() {
        let songs = [
            song(1, "Common - The Glory [V1]"),
            song(2, "The Glory [V2]"),
            song(3, "Glory [V4]").era(1),
            song(4, "Glory [V20-V21]").era(1),
            song(5, "⭐️ Glory [V22]").era(1),
            song(6, "This Is The Glory [V1-V?]").era(1),
            song(7, "✨ Glory [V1]").era(2),
            song(8, "Donda's Glory [V4]").era(2),
            song(9, "Believe What I Say").notes("Glory sample"),
        ];
        assert_eq!(rank("glory", &songs), [5, 7, 3, 4, 2, 8, 1, 6, 9]);
    }

    #[test]
    fn breath_prefers_the_whole_word_over_longer_words() {
        let songs = [
            song(1, "Breathe In Breathe Out [V1]"),
            song(2, "Room To Breathe [V1]"),
            song(3, "I Know God Breathed On This [V3]").era(1),
            song(4, "✨ I Know God Breathed On This [V11]").era(2),
            song(5, "Breath [V16]").era(2),
            song(6, "✨ Breathe [V17]").era(2),
            song(7, "Breathe [V18]").era(2),
        ];
        assert_eq!(rank("breath", &songs), [5, 6, 7, 1, 2, 4, 3]);
    }

    #[test]
    fn hit_ranks_ai_and_worst_of_songs_below_unmarked_ones() {
        let songs = [
            song(1, "Hitmaka - Unknown [Kanye West Collaborations]"),
            song(2, "Heavy Hitters"),
            song(3, "The White Stripes - Over and Over and Over [V1]"),
            song(4, "Hit-Boy - ??? [V1]"),
            song(5, "Unknown [Hit-Boy Collaborations]"),
            song(6, "🤖 HIT [V66]"),
            song(7, "🤖 HITLER YE AND JESUS [V2]"),
            song(8, "🗑️🤖 HITLER YE AND JESUS [V9]"),
            song(9, "HITLER YE AND JESUS [V12]"),
        ];
        assert_eq!(rank("hit", &songs), [6, 4, 5, 9, 1, 8, 7, 2, 3]);
    }

    #[test]
    fn nyc_prefers_the_exact_title_over_a_best_of_prefix() {
        let songs = [
            song(1, "Nyce Vieux [V1]"),
            song(2, "⭐ Nyce Vieux [V2]"),
            song(3, "NYC [V8]"),
            song(4, "NYC [V9]"),
            song(5, "NYC [V14]"),
        ];
        assert_eq!(rank("nyc", &songs), [3, 4, 5, 2, 1]);
    }

    #[test]
    fn yikes_orders_equal_matches_by_category_then_catalog() {
        let songs = [
            song(1, "Yikes [V2]"),
            song(2, "Yikes [V3]"),
            song(3, "🏆 Yikes [V4]"),
            song(4, "🏅 Yikes [V5]"),
            song(5, "Yikes [V6]"),
            song(6, "✨ Yikes [V10]"),
            song(7, "🗑️ Yikes [V11]"),
            song(8, "🤖 Yikes [V12]"),
        ];
        assert_eq!(rank("yikes", &songs), [6, 3, 4, 1, 2, 5, 7, 8]);
    }

    #[test]
    fn new_body_matches_the_phrase_and_featured_titles() {
        let songs = [
            song(1, "New Body [V2]\n(feat. Nicki Minaj & Ty Dolla $ign)"),
            song(2, "✨ New Body [V13]"),
            song(3, "⭐ New Body [V21]"),
            song(4, "Ty Dolla $ign - New Body [V34]"),
            song(5, "Brand New [V1]\n(Alternate titles: Body)"),
            song(6, "Everything We Need").notes("Reuses the New Body beat"),
            song(7, "New Body [V28-V31]").playable(),
            // Tokens match as substrings, so `new` matches `Newton`.
            song(8, "Body Language [V1]\n(prod. Newton)"),
            song(9, "Body Language [V2]"),
        ];
        assert_eq!(rank("new body", &songs), [3, 2, 7, 1, 4, 6, 5, 8]);
    }

    #[test]
    fn pablo_puts_titles_before_era_matches() {
        let songs = [
            song(1, "Famous [V3]").era(3),
            song(2, "Saint Pablo [V4]").era(3),
            song(3, "✨ Saint Pablo [V5]").era(3),
            song(4, "🗑️ Saint Pablo [V9]").era(3),
            song(5, "PABLO [V2]").era(4),
            song(
                6,
                "Wolves [V2]\n(feat. Frank Ocean)\n(Alternate titles: Pablo Wolves)",
            )
            .era(3),
            song(7, "LOVE THY NEIGHBOR").era(4),
        ];
        assert_eq!(rank("pablo", &songs), [5, 3, 2, 4, 6, 7, 1]);
    }

    #[test]
    fn king_orders_exact_prefix_word_and_substring_matches() {
        let songs = [
            song(1, "Looking For Trouble"),
            song(2, "King Malcom [V2]"),
            song(3, "Cassie - King Of Hearts (Kanye West Remix) [V1-V3]"),
            song(4, "Northside Kings"),
            song(5, "Our King [V1]"),
            song(6, "Selah [V2]").era(1),
            song(7, "King [V2]"),
            song(8, "Still The King [V3]"),
            song(9, "✨ King [V5]"),
            song(10, "KING [V10-V??]"),
            song(11, "⭐️ THE KING OF SOUL [V8]"),
            song(12, "Wash Us In The Blood").notes("Leaked by a king of leaks"),
            song(13, "King [V4]\n(prod. Lester Nowhere)\n(Still The King)"),
        ];
        assert_eq!(
            rank("king", &songs),
            [9, 7, 10, 13, 2, 3, 11, 5, 8, 4, 1, 6, 12]
        );
    }

    #[test]
    fn donda_ranks_titles_then_eras_then_notes() {
        let songs = [
            song(1, "Donda [V16]").era(2),
            song(2, "✨ Donda [V17]").era(2),
            song(3, "Donda Donda Donda [V2]").era(2),
            song(4, "🗑️ Donda Outro [V1]").era(2),
            song(5, "Donda Outro [V2]").era(2),
            song(6, "North Of Donda").era(1),
            song(7, "The Black Guns - DONDA [Album]").era(2),
            song(8, "Hurricane [V9]").era(2),
            song(9, "⭐ Moon [V3]").era(2),
            song(10, "Life Of The Party [V2]").era(4),
            song(11, "Power [V1]").notes("Samples Donda West"),
            song(12, "Jail [V3]").era(2).playable(),
        ];
        assert_eq!(
            rank("donda", &songs),
            [2, 1, 7, 5, 3, 4, 6, 9, 12, 8, 10, 11]
        );
    }

    #[test]
    fn multi_token_queries_match_phrases_before_scattered_tokens() {
        let songs = [
            song(1, "NEBRASKA [V4]\n(feat. Pusha T)"),
            song(2, "NEBRASKA [V40]"),
            song(3, "Nebraska 2 [V1]").notes("A v4 of something else"),
            song(4, "Can't Tell Me Nothing [V2]"),
            song(5, "Power [V4]").era(1).sub_era("Nebraska Sessions"),
            song(6, "Pusha T - Nebraska [V1]").quality("CD Quality"),
        ];
        // Scattered tokens rank by the worst field they needed: the era beats
        // the notes.
        assert_eq!(rank("nebraska v4", &songs), [1, 2, 5, 3]);
        assert_eq!(rank("nebraska pusha", &songs), [6, 1]);
        assert_eq!(rank("can’t tell me nothing", &songs), [4]);
        assert_eq!(rank("cd quality nebraska", &songs), [6]);
    }

    #[test]
    fn playable_breaks_ties_after_closeness_and_position_breaks_the_rest() {
        let songs = [
            song(1, "Flowers [V1]"),
            song(2, "Flowers [V2]").playable(),
            song(3, "Flowers [V3]"),
            song(4, "Flowers Bloom [V1]").playable(),
        ];
        assert_eq!(rank("flowers", &songs), [2, 1, 3, 4]);
    }

    #[test]
    fn rank_keeps_the_best_and_puts_unknown_songs_last() {
        let songs = [
            song(1, "Love Lockdown"),
            song(2, "Love [V2]"),
            song(3, "Lovely"),
        ];
        let index = index_of(&songs);
        let query = SearchQuery::new("love").unwrap();
        let matches = [(3, false), (99, true), (1, false), (2, false)];
        assert_eq!(index.rank(&query, &matches, 4), [3, 2, 0, 1]);
        assert_eq!(index.rank(&query, &matches, 2), [3, 2]);
        assert!(index.rank(&query, &matches, 0).is_empty());
    }

    #[test]
    fn cache_rebuilds_only_for_another_catalog_version() {
        let cache = RankCache::new();
        let mut builds = 0;
        let mut build = |version: &str| {
            builds += 1;
            Ok::<_, ()>(CatalogIndex::new(Some(version.to_string())))
        };
        let first = cache.index(Some("a"), || build("a")).unwrap();
        let again = cache.index(Some("a"), || build("a")).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        let next = cache.index(Some("b"), || build("b")).unwrap();
        assert_eq!(next.version(), Some("b"));
        assert_eq!(builds, 2);
    }
}
