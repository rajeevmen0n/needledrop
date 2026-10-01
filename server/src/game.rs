//! Pure game logic: the clip ladder, state transitions, and title/artist normalization for matching.
//!
//! Nothing here knows how the day's track was chosen or where the state is
//! kept, so other modes (random, genre) can reuse it. No I/O and no clock:
//! callers pass in today's date.

use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Clip length unlocked on each turn, in milliseconds. A miss (wrong guess or
/// skip) moves the player one step along.
pub const LADDER_MS: [u32; 7] = [100, 300, 1_000, 3_000, 8_000, 16_000, 30_000];

/// Misses that end the game: one per ladder step.
pub const MAX_ATTEMPTS: usize = LADDER_MS.len();

/// The whole preview, which a finished game unlocks.
pub const FULL_CLIP_MS: u32 = LADDER_MS[MAX_ATTEMPTS - 1];

/// Longest title or artist kept in an [`Attempt`], in characters.
pub const MAX_FIELD_CHARS: usize = 80;

/// Longest title or artist kept in an [`Attempt`], in bytes of JSON.
///
/// 80 characters of Japanese are 240 bytes, so a character limit alone does
/// not bound the cookie. With both limits a lost game of seven wrong guesses
/// stays under 2.7 kB of JSON, about 3.7 kB once encrypted and base64-encoded,
/// inside the 4 kB a browser accepts.
pub const MAX_FIELD_BYTES: usize = 160;

/// The ladder in seconds, as the API reports it: `0.1, 0.3, 1, 3, 8, 16, 30`.
pub fn ladder_seconds() -> [f64; MAX_ATTEMPTS] {
    LADDER_MS.map(|ms| f64::from(ms) / 1000.0)
}

/// 1-based number of the daily game: the launch day is No. 1. Zero or
/// negative when `today` is before the launch.
pub fn day_number(launch: Date, today: Date) -> i64 {
    // Subtracting civil dates yields a span in whole days and cannot fail.
    i64::from((today - launch).get_days()) + 1
}

/// Where a game stands. Serialized as `"playing"`, `"won"` or `"lost"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Playing,
    Won,
    Lost,
}

/// One used-up turn, as the player sees it in their history.
///
/// Serialized as `{"kind":"skip"}` or
/// `{"kind":"wrong","title":"…","artist":"…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Attempt {
    Skip,
    Wrong { title: String, artist: String },
}

impl Attempt {
    /// A wrong guess. The title and artist are cut to [`MAX_FIELD_CHARS`] and
    /// [`MAX_FIELD_BYTES`], because every attempt lives in the session cookie.
    pub fn wrong(title: &str, artist: &str) -> Self {
        Self::Wrong {
            title: clip_field(title),
            artist: clip_field(artist),
        }
    }
}

/// Trims `text`, drops control characters and cuts it, on a character
/// boundary, to what an [`Attempt`] may store.
fn clip_field(text: &str) -> String {
    let mut clipped = String::new();
    let mut bytes = 0;
    let visible = text.trim().chars().filter(|c| !c.is_control());
    for c in visible.take(MAX_FIELD_CHARS) {
        // Count the size in JSON, where `"` and `\` are escaped.
        bytes += if matches!(c, '"' | '\\') {
            2
        } else {
            c.len_utf8()
        };
        if bytes > MAX_FIELD_BYTES {
            break;
        }
        clipped.push(c);
    }
    clipped
}

/// A move was made on a game that is already won or lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GameError {
    #[error("the game is already finished")]
    Finished,
}

/// A stored state that no sequence of moves can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("impossible game state: {attempts} attempts with status {status:?}")]
pub struct InvalidState {
    attempts: usize,
    status: Status,
}

/// One player's game for one day. This is the whole session: it is what the
/// encrypted cookie holds, as JSON.
///
/// ```json
/// {"day":"2026-10-01","attempts":[{"kind":"skip"},{"kind":"wrong","title":"Under Pressure","artist":"Queen"}],"status":"playing"}
/// ```
///
/// `day` is the UTC date as `YYYY-MM-DD`. The field names are spelled out
/// rather than abbreviated: the worst case is still well inside a cookie (see
/// [`MAX_FIELD_BYTES`]), and `attempts` and `status` can go into the API
/// response as they are.
///
/// Deserializing checks that the attempts and status agree (see
/// [`InvalidState`]), so the methods can rely on it. Treat a state that fails
/// to deserialize like a missing cookie: start a new game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "StoredState")]
pub struct GameState {
    day: Date,
    attempts: Vec<Attempt>,
    status: Status,
}

/// [`GameState`] as read from the cookie, before its invariants are checked.
#[derive(Deserialize)]
struct StoredState {
    day: Date,
    attempts: Vec<Attempt>,
    status: Status,
}

impl TryFrom<StoredState> for GameState {
    type Error = InvalidState;

    fn try_from(stored: StoredState) -> Result<Self, Self::Error> {
        let attempts = stored.attempts.len();
        let possible = match stored.status {
            Status::Playing | Status::Won => attempts < MAX_ATTEMPTS,
            Status::Lost => attempts == MAX_ATTEMPTS,
        };
        if !possible {
            return Err(InvalidState {
                attempts,
                status: stored.status,
            });
        }
        Ok(Self {
            day: stored.day,
            attempts: stored.attempts,
            status: stored.status,
        })
    }
}

impl GameState {
    /// A fresh game for `day`: no attempts, the shortest clip unlocked.
    pub fn new(day: Date) -> Self {
        Self {
            day,
            attempts: Vec::new(),
            status: Status::Playing,
        }
    }

    /// This state if it is for `today`, otherwise a fresh game for `today`.
    /// Yesterday's cookie says nothing about today's song.
    pub fn for_day(self, today: Date) -> Self {
        if self.day == today {
            self
        } else {
            Self::new(today)
        }
    }

    /// The UTC date this game belongs to.
    pub fn day(&self) -> Date {
        self.day
    }

    /// The misses so far, oldest first. A winning guess is not in the list.
    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }

    /// Whether the game is still being played, won or lost.
    pub fn status(&self) -> Status {
        self.status
    }

    /// Whether the game is won or lost, which is when the answer may be shown.
    pub fn is_finished(&self) -> bool {
        self.status != Status::Playing
    }

    /// How much of the preview the player may hear, in milliseconds: the
    /// ladder step for the current turn, or all of it once the game is over.
    pub fn unlocked_ms(&self) -> u32 {
        match self.status {
            Status::Playing => LADDER_MS
                .get(self.attempts.len())
                .copied()
                .unwrap_or(FULL_CLIP_MS),
            Status::Won | Status::Lost => FULL_CLIP_MS,
        }
    }

    /// Gives up the current turn. Returns the status afterwards: `Playing`
    /// with a longer clip unlocked, or `Lost` if that was the last turn.
    pub fn skip(&mut self) -> Result<Status, GameError> {
        self.miss(Attempt::Skip)
    }

    /// Plays `guessed` against `answer`. Returns the status afterwards: `Won`
    /// for a match (nothing is added to the attempts), otherwise `Playing` or
    /// `Lost` with the wrong guess recorded.
    pub fn guess(&mut self, answer: &TrackMeta, guessed: &TrackMeta) -> Result<Status, GameError> {
        if self.is_finished() {
            return Err(GameError::Finished);
        }
        if is_match(answer, guessed) {
            self.status = Status::Won;
            return Ok(self.status);
        }
        self.miss(Attempt::wrong(&guessed.title, &guessed.artist))
    }

    fn miss(&mut self, attempt: Attempt) -> Result<Status, GameError> {
        if self.is_finished() {
            return Err(GameError::Finished);
        }
        self.attempts.push(attempt);
        if self.attempts.len() >= MAX_ATTEMPTS {
            self.status = Status::Lost;
        }
        Ok(self.status)
    }
}

/// What matching needs to know about a track, the answer or a guess alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMeta {
    /// Deezer's `title`, which may end in "(Remastered 2011)" and the like.
    /// This is the one shown to the player.
    pub title: String,
    /// Deezer's `title_short`: the title without the version. May be empty.
    pub title_short: String,
    /// Name of the primary artist.
    pub artist: String,
}

impl TrackMeta {
    pub fn new(
        title: impl Into<String>,
        title_short: impl Into<String>,
        artist: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            title_short: title_short.into(),
            artist: artist.into(),
        }
    }

    /// The normalized (title, artist) pair. Two tracks with the same key are
    /// the same song for the game, whatever their Deezer IDs; search results
    /// can be de-duplicated on it.
    pub fn match_key(&self) -> (String, String) {
        // Deezer has already taken the version off `title_short`.
        let title = if self.title_short.trim().is_empty() {
            &self.title
        } else {
            &self.title_short
        };
        (normalize_title(title), normalize_artist(&self.artist))
    }
}

/// Whether `guess` is the same song as `answer`: normalized title and
/// normalized primary artist both equal. Comparing IDs would reject the
/// remaster or the live cut of the right song.
///
/// There is deliberately nothing fuzzy here, and no edit distance: the player
/// picks from autocomplete, so there are no typos to forgive, and a different
/// song must never match.
pub fn is_match(answer: &TrackMeta, guess: &TrackMeta) -> bool {
    let answer = answer.match_key();
    // A blank title is a failed lookup, not a song; never let two of them match.
    !answer.0.is_empty() && answer == guess.match_key()
}

/// Reduces a title to a comparison key: case, diacritics, punctuation and
/// version markers removed.
///
/// "Under Pressure - Remastered 2011", "Under Pressure (Live)" and
/// "Under Pressure (feat. David Bowie)" all become `underpressure`.
///
/// A title with nothing left after that ("!!!", "(Untitled)") keeps its
/// lowercased original text instead, so that symbol-only titles do not all
/// match each other.
pub fn normalize_title(title: &str) -> String {
    let folded = fold(title);
    let unbracketed = strip_brackets(&folded);
    let key = squash(strip_version_suffix(strip_featuring(&unbracketed)));
    if key.is_empty() { fallback(title) } else { key }
}

/// Reduces an artist name to a comparison key: case, diacritics and
/// punctuation removed, `&` read as "and". Same fallback as
/// [`normalize_title`] for names like "!!!".
pub fn normalize_artist(artist: &str) -> String {
    let key = squash(&fold(artist));
    if key.is_empty() {
        fallback(artist)
    } else {
        key
    }
}

/// The key for text that normalizes to nothing.
fn fallback(original: &str) -> String {
    original.trim().to_lowercase()
}

/// Compatibility-decomposes `text` (so "é" becomes "e" plus an accent and
/// full-width "Ａ" becomes "A"), drops the accents and lowercases.
fn fold(text: &str) -> String {
    text.nfkd()
        .filter(|&c| !is_diacritic(c))
        .collect::<String>()
        .to_lowercase()
}

/// Whether `c` is a combining accent of the kind Latin, Greek and Cyrillic
/// use: the Combining Diacritical Marks blocks.
///
/// Only these are dropped. The combining marks of other scripts carry
/// meaning that is not an accent: Japanese voicing marks tell ガ from カ, and
/// Devanagari and Thai write their vowels with them. Dropping those would
/// make different titles equal.
fn is_diacritic(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}'
        | '\u{1AB0}'..='\u{1AFF}'
        | '\u{1DC0}'..='\u{1DFF}'
        | '\u{20D0}'..='\u{20FF}'
        | '\u{FE20}'..='\u{FE2F}')
}

/// Removes `(...)` and `[...]` segments, nested ones included, leaving a
/// space where each stood. A bracket that is never closed takes the rest of
/// the text with it.
fn strip_brackets(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                kept.push(' ');
            }
            _ if depth == 0 => kept.push(c),
            _ => {}
        }
    }
    kept
}

/// Cuts `text` at a "feat.", "ft." or "featuring" credit. Expects lowercase.
/// The credit has to follow at least one other word, so a title that merely
/// starts with "Featuring" is left alone.
fn strip_featuring(text: &str) -> &str {
    let mut offset = 0;
    let mut seen_word = false;
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end();
        if seen_word && matches!(word, "feat." | "feat" | "ft." | "featuring") {
            return &text[..offset];
        }
        seen_word |= !word.is_empty();
        offset += piece.len();
    }
    text
}

/// Words that mark a trailing " - …" segment as a version, not part of the name.
const VERSION_WORDS: [&str; 15] = [
    "remaster",
    "remastered",
    "version",
    "edit",
    "mix",
    "mono",
    "stereo",
    "live",
    "single",
    "radio",
    "bonus",
    "deluxe",
    "anniversary",
    "demo",
    "acoustic",
];

/// Removes trailing " - …" segments that name a version: "- Remastered 2011",
/// "- Live at Wembley", "- 2015 Remaster". Expects lowercase. Other dashed
/// segments stay ("Symphony No. 5 - Allegro"), as does anything that would
/// leave no title at all.
fn strip_version_suffix(text: &str) -> &str {
    let mut title = text;
    while let Some((head, tail)) = split_last_dash(title) {
        if !is_version_tag(tail) || !head.chars().any(char::is_alphanumeric) {
            break;
        }
        title = head;
    }
    title
}

/// Splits at the last hyphen or dash that has whitespace on both sides, so
/// "Anti-Hero" is not split.
fn split_last_dash(text: &str) -> Option<(&str, &str)> {
    text.rmatch_indices(['-', '\u{2013}', '\u{2014}'])
        .find_map(|(at, dash)| {
            let (head, tail) = (&text[..at], &text[at + dash.len()..]);
            (head.ends_with(char::is_whitespace) && tail.starts_with(char::is_whitespace))
                .then_some((head, tail))
        })
}

/// Whether `segment` contains a version word or a year (1900–2099) as a
/// whole word.
fn is_version_tag(segment: &str) -> bool {
    segment
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| VERSION_WORDS.contains(&word) || is_year(word))
}

fn is_year(word: &str) -> bool {
    word.len() == 4
        && (word.starts_with("19") || word.starts_with("20"))
        && word.bytes().all(|b| b.is_ascii_digit())
}

/// Reads `&` as "and", then keeps only letters and digits of any script
/// (with the combining marks that [`fold`] left attached to them).
fn squash(text: &str) -> String {
    text.replace('&', " and ")
        .chars()
        .filter(|&c| c.is_alphanumeric() || is_combining_mark(c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    const DAY: Date = date(2026, 10, 1);

    fn track(title: &str, artist: &str) -> TrackMeta {
        TrackMeta::new(title, "", artist)
    }

    fn answer() -> TrackMeta {
        TrackMeta::new(
            "Under Pressure (Remastered 2011)",
            "Under Pressure",
            "Queen",
        )
    }

    fn wrong() -> TrackMeta {
        track("Bohemian Rhapsody", "Queen")
    }

    // --- ladder and day number ---------------------------------------------

    #[test]
    fn ladder_has_seven_steps_ending_at_the_full_preview() {
        assert_eq!(MAX_ATTEMPTS, 7);
        assert_eq!(FULL_CLIP_MS, 30_000);
        assert!(LADDER_MS.is_sorted_by(|a, b| a < b));
    }

    #[test]
    fn ladder_in_seconds() {
        assert_eq!(ladder_seconds(), [0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0]);
        // And it prints without float noise.
        assert_eq!(
            serde_json::to_string(&ladder_seconds()).unwrap(),
            "[0.1,0.3,1.0,3.0,8.0,16.0,30.0]"
        );
    }

    #[test]
    fn day_number_counts_from_one() {
        let launch = date(2026, 10, 1);
        assert_eq!(day_number(launch, launch), 1);
        assert_eq!(day_number(launch, date(2026, 10, 2)), 2);
        assert_eq!(day_number(launch, date(2026, 10, 31)), 31);
        // Across a month boundary.
        assert_eq!(day_number(launch, date(2026, 11, 1)), 32);
        // Across a year boundary: 31 + 30 + 31 days after launch.
        assert_eq!(day_number(launch, date(2026, 12, 31)), 92);
        assert_eq!(day_number(launch, date(2027, 1, 1)), 93);
        assert_eq!(day_number(launch, date(2027, 10, 1)), 366);
        // Across a leap day (2028-02-29).
        assert_eq!(day_number(launch, date(2028, 3, 1)), 518);
        // Before the launch.
        assert_eq!(day_number(launch, date(2026, 9, 30)), 0);
        assert_eq!(day_number(launch, date(2026, 9, 1)), -29);
    }

    // --- title normalization -----------------------------------------------

    #[test]
    fn title_is_lowercased_and_stripped_of_punctuation() {
        assert_eq!(normalize_title("Mr. Brightside"), "mrbrightside");
        assert_eq!(normalize_title("Don't Stop Me Now"), "dontstopmenow");
        assert_eq!(normalize_title("Don\u{2019}t Stop Me Now"), "dontstopmenow");
        assert_eq!(normalize_title("  HEY   JUDE  "), "heyjude");
        assert_eq!(normalize_title("Help!"), "help");
        assert_eq!(normalize_title("Summer of '69"), "summerof69");
        assert_eq!(normalize_title("99 Luftballons"), "99luftballons");
        assert_eq!(normalize_title("Anti-Hero"), "antihero");
        assert_eq!(normalize_title("Bron-Y-Aur Stomp"), "bronyaurstomp");
    }

    #[test]
    fn title_diacritics_are_folded() {
        assert_eq!(normalize_title("Déjà Vu"), "dejavu");
        assert_eq!(normalize_title("Deja Vu"), "dejavu");
        assert_eq!(normalize_title("Señorita"), "senorita");
        assert_eq!(normalize_title("Senorita"), "senorita");
        assert_eq!(normalize_title("La Vie en Rose"), "lavieenrose");
        assert_eq!(normalize_title("Ça plane pour moi"), "caplanepourmoi");
        // Precomposed and decomposed input agree.
        assert_eq!(
            normalize_title("De\u{0301}ja\u{0300} Vu"),
            normalize_title("D\u{00E9}j\u{00E0} Vu")
        );
        // Compatibility forms: full-width letters, ligatures.
        assert_eq!(normalize_title("ＡＢＣ"), "abc");
        assert_eq!(normalize_title("ﬁre"), "fire");
        // Uppercase with accents, and a Greek final sigma.
        assert_eq!(normalize_title("DÉJÀ VU"), "dejavu");
        assert_eq!(normalize_title("ΟΔΟΣ"), normalize_title("οδός"));
    }

    #[test]
    fn title_brackets_are_dropped() {
        assert_eq!(
            normalize_title("Bohemian Rhapsody (Live Aid)"),
            "bohemianrhapsody"
        );
        assert_eq!(
            normalize_title("Under Pressure (Remastered 2011)"),
            "underpressure"
        );
        assert_eq!(normalize_title("Hurt [Explicit]"), "hurt");
        assert_eq!(
            normalize_title("Heroes (Single Version) [2014 Remaster]"),
            "heroes"
        );
        // A bracket in the middle or at the start.
        assert_eq!(
            normalize_title("(I Can't Get No) Satisfaction"),
            "satisfaction"
        );
        assert_eq!(
            normalize_title("Rocket Man (I Think It's Going to Be) A Long Time"),
            "rocketmanalongtime"
        );
        // Nested, unclosed and stray brackets.
        assert_eq!(normalize_title("Song (Live (2011 Remaster))"), "song");
        assert_eq!(normalize_title("Song (Live at Wemb"), "song");
        assert_eq!(normalize_title("Song) Two"), "songtwo");
        // Full-width brackets, as in Japanese releases.
        assert_eq!(normalize_title("Song（Live）"), "song");
    }

    #[test]
    fn title_featuring_credit_is_dropped() {
        assert_eq!(normalize_title("Stan (feat. Dido)"), "stan");
        assert_eq!(normalize_title("Stan feat. Dido"), "stan");
        assert_eq!(normalize_title("Stan Feat. Dido"), "stan");
        assert_eq!(normalize_title("Stan ft. Dido"), "stan");
        assert_eq!(normalize_title("Stan featuring Dido"), "stan");
        assert_eq!(normalize_title("Stan feat Dido"), "stan");
        assert_eq!(
            normalize_title("Empire State of Mind [feat. Alicia Keys]"),
            "empirestateofmind"
        );
        // Together with a version suffix, in either order.
        assert_eq!(normalize_title("Stan feat. Dido - Radio Edit"), "stan");
        assert_eq!(normalize_title("Stan - Radio Edit feat. Dido"), "stan");
        // Not a credit: part of a word, or the first word.
        assert_eq!(normalize_title("Defeat. Again"), "defeatagain");
        assert_eq!(normalize_title("Featuring You"), "featuringyou");
        assert_eq!(normalize_title("Soft Featherbed"), "softfeatherbed");
    }

    #[test]
    fn strip_featuring_cuts_at_the_credit() {
        assert_eq!(strip_featuring("stan feat. dido"), "stan ");
        assert_eq!(strip_featuring("stan  ft.  dido & eminem"), "stan  ");
        assert_eq!(strip_featuring("stan"), "stan");
        assert_eq!(strip_featuring("featuring you"), "featuring you");
        assert_eq!(strip_featuring("  feat. dido"), "  feat. dido");
        assert_eq!(strip_featuring(""), "");
    }

    #[test]
    fn title_version_suffix_is_dropped() {
        let cases = [
            "Under Pressure - Remastered 2011",
            "Under Pressure - Remastered",
            "Under Pressure - 2011 Remaster",
            "Under Pressure - Single Version",
            "Under Pressure - Radio Edit",
            "Under Pressure - Mono",
            "Under Pressure - Stereo Mix",
            "Under Pressure - Live at Wembley Stadium, July 1986",
            "Under Pressure - Bonus Track",
            "Under Pressure - Deluxe",
            "Under Pressure - 40th Anniversary Edition",
            "Under Pressure - Demo",
            "Under Pressure - Acoustic",
            "Under Pressure - 2011",
            // Two suffixes, and other dash characters.
            "Under Pressure - Live - Remastered 2011",
            "Under Pressure \u{2013} Remastered 2011",
            "Under Pressure \u{2014} Remastered 2011",
            // Mixed with a bracket.
            "Under Pressure (Live) - Remastered 2011",
            "Under Pressure - Remastered 2011 (Bonus Track)",
        ];
        for title in cases {
            assert_eq!(normalize_title(title), "underpressure", "{title}");
        }
        assert_eq!(normalize_title("1979 - Remastered 2012"), "1979");
        assert_eq!(normalize_title("Heroes - 2017 Remaster"), "heroes");
    }

    #[test]
    fn title_dash_that_is_not_a_version_is_kept() {
        // No spaces around the hyphen.
        assert_eq!(normalize_title("Anti-Hero"), "antihero");
        assert_eq!(normalize_title("Radio-Activity"), "radioactivity");
        // A subtitle, not a version.
        assert_eq!(
            normalize_title("Symphony No. 5 - Allegro con brio"),
            "symphonyno5allegroconbrio"
        );
        // Version words only count as whole words.
        assert_eq!(normalize_title("Stayin - Alive"), "stayinalive");
        assert_eq!(
            normalize_title("Levitating - The Blessed Madonna Remix"),
            "levitatingtheblessedmadonnaremix"
        );
        // Nothing would be left of the title.
        assert_eq!(normalize_title(" - Live"), "live");
        assert_eq!(normalize_title("Live"), "live");
        assert_eq!(normalize_title("1999"), "1999");
    }

    #[test]
    fn version_tag_detection() {
        for tag in [
            " remastered 2011",
            " 2011 remaster",
            " live at leeds",
            " single version",
            " mono",
            " 1999",
            " 2024",
            " live/1972",
        ] {
            assert!(is_version_tag(tag), "{tag}");
        }
        for not_tag in [
            " alive", " remixed", " remix", " editor", " 12345", " 1899", " 2100", " 69", "",
        ] {
            assert!(!is_version_tag(not_tag), "{not_tag}");
        }
    }

    #[test]
    fn ampersand_reads_as_and() {
        assert_eq!(normalize_title("Me & Bobby McGee"), "meandbobbymcgee");
        assert_eq!(
            normalize_title("Me & Bobby McGee"),
            normalize_title("Me and Bobby McGee")
        );
        assert_eq!(normalize_title("Rock&Roll"), "rockandroll");
        assert_eq!(
            normalize_artist("Earth, Wind & Fire"),
            normalize_artist("Earth, Wind and Fire")
        );
        assert_eq!(normalize_artist("Simon ＆ Garfunkel"), "simonandgarfunkel");
    }

    #[test]
    fn non_latin_titles_survive() {
        // Japanese.
        assert_eq!(normalize_title("夜に駆ける"), normalize_title("夜に駆ける"));
        assert!(!normalize_title("夜に駆ける").is_empty());
        assert_ne!(normalize_title("夜に駆ける"), normalize_title("群青"));
        // The voicing mark is part of the letter: glass is not crow.
        assert_ne!(normalize_title("ガラス"), normalize_title("カラス"));
        // Half-width and full-width katakana are the same text.
        assert_eq!(normalize_title("ｶﾞﾗｽ"), normalize_title("ガラス"));

        // Korean, with and without the translated subtitle.
        assert_eq!(
            normalize_title("봄날 (Spring Day)"),
            normalize_title("봄날")
        );
        assert_ne!(normalize_title("봄날"), normalize_title("봄"));
        assert_eq!(
            normalize_artist("방탄소년단"),
            normalize_artist("방탄소년단")
        );

        // Cyrillic folds case; Devanagari keeps its vowel signs.
        assert_eq!(normalize_title("Группа Крови"), "группакрови");
        assert_ne!(normalize_title("तुम ही हो"), normalize_title("तम ह ह"));
        assert_ne!(normalize_title("दिल"), normalize_title("दल"));
    }

    #[test]
    fn symbol_only_titles_fall_back_to_the_original() {
        assert_eq!(normalize_title("!!!"), "!!!");
        assert_eq!(normalize_title(" ??? "), "???");
        assert_eq!(normalize_title("(Untitled)"), "(untitled)");
        assert_eq!(normalize_title("[Intro]"), "[intro]");
        assert_ne!(normalize_title("!!!"), normalize_title("???"));
        assert_ne!(
            normalize_title("(Untitled)"),
            normalize_title("(Interlude)")
        );
        assert_eq!(normalize_artist("!!!"), "!!!");
        assert_ne!(normalize_artist("!!!"), normalize_artist("+/-"));
        assert_eq!(normalize_title(""), "");
    }

    #[test]
    fn artist_normalization() {
        assert_eq!(normalize_artist("AC/DC"), "acdc");
        assert_eq!(normalize_artist("Guns N' Roses"), "gunsnroses");
        assert_eq!(normalize_artist("Beyoncé"), "beyonce");
        assert_eq!(normalize_artist("Beyonce"), "beyonce");
        assert_eq!(normalize_artist("Motörhead"), "motorhead");
        assert_eq!(normalize_artist("Sigur Rós"), "sigurros");
        assert_eq!(normalize_artist("P!nk"), "pnk");
        assert_eq!(normalize_artist("blink-182"), "blink182");
        assert_eq!(normalize_artist("  The  Beatles "), "thebeatles");
        // Brackets and dashes mean nothing special in a name.
        assert_eq!(normalize_artist("Sunn O)))"), "sunno");
        assert_eq!(
            normalize_artist("Florence + The Machine"),
            "florencethemachine"
        );
        assert_eq!(normalize_artist("A - Live"), "alive");
    }

    // --- matching ----------------------------------------------------------

    #[test]
    fn title_short_is_preferred_when_present() {
        let with_short = TrackMeta::new(
            "Under Pressure (Remastered 2011)",
            "Under Pressure",
            "Queen",
        );
        assert_eq!(
            with_short.match_key(),
            ("underpressure".to_owned(), "queen".to_owned())
        );
        // `title_short` wins even when `title` says something else.
        let odd = TrackMeta::new("Something Else Entirely", "Under Pressure", "Queen");
        assert_eq!(odd.match_key().0, "underpressure");
        // Empty or blank: fall back to `title`.
        for short in ["", "   "] {
            let without = TrackMeta::new("Under Pressure - Remastered 2011", short, "Queen");
            assert_eq!(without.match_key().0, "underpressure");
        }
    }

    #[test]
    fn variants_of_the_same_song_match() {
        let answer = answer();
        let variants = [
            TrackMeta::new("Under Pressure", "Under Pressure", "Queen"),
            TrackMeta::new(
                "Under Pressure - Remastered 2011",
                "Under Pressure - Remastered 2011",
                "Queen",
            ),
            TrackMeta::new(
                "Under Pressure (Live at Wembley '86)",
                "Under Pressure",
                "Queen",
            ),
            TrackMeta::new(
                "Under Pressure (feat. David Bowie)",
                "Under Pressure",
                "Queen",
            ),
            track("UNDER PRESSURE", "QUEEN"),
            track("Under Pressure [2011 Remaster]", "queen"),
        ];
        for variant in &variants {
            assert!(is_match(&answer, variant), "{variant:?}");
            assert!(is_match(variant, &answer), "{variant:?}");
        }

        assert!(is_match(
            &track("Déjà Vu", "Beyoncé"),
            &track("Deja Vu (feat. Jay-Z)", "Beyonce")
        ));
        assert!(is_match(
            &track("Sweet Child O' Mine", "Guns N' Roses"),
            &track("Sweet Child O’ Mine", "Guns N’ Roses")
        ));
        assert!(is_match(
            &track("Back In Black", "AC/DC"),
            &track("Back in Black", "AC⚡DC")
        ));
    }

    #[test]
    fn different_songs_do_not_match() {
        // Same title, different artist.
        assert!(!is_match(
            &track("Bohemian Rhapsody", "Queen"),
            &track("Bohemian Rhapsody", "The Braids")
        ));
        // Same artist, different title.
        assert!(!is_match(
            &track("Bohemian Rhapsody", "Queen"),
            &track("Under Pressure", "Queen")
        ));
        // One title is a prefix of the other.
        assert!(!is_match(
            &track("One", "U2"),
            &track("One Tree Hill", "U2")
        ));
        assert!(!is_match(
            &track("One Tree Hill", "U2"),
            &track("One", "U2")
        ));
        // One letter apart: nothing fuzzy.
        assert!(!is_match(
            &track("Stan", "Eminem"),
            &track("Stand", "Eminem")
        ));
        assert!(!is_match(
            &track("Hello", "Adele"),
            &track("Hallo", "Adele")
        ));
        // Artist is a prefix.
        assert!(!is_match(&track("Hello", "Adele"), &track("Hello", "Adel")));
        // A numbered sequel.
        assert!(!is_match(
            &track("Another Brick in the Wall, Pt. 2", "Pink Floyd"),
            &track("Another Brick in the Wall, Pt. 1", "Pink Floyd")
        ));
        // Symbol-only titles.
        assert!(!is_match(&track("!!!", "Artist"), &track("???", "Artist")));
    }

    #[test]
    fn blank_metadata_never_matches() {
        assert!(!is_match(&track("", ""), &track("", "")));
        assert!(!is_match(&track("", "Queen"), &track("", "Queen")));
        assert!(!is_match(&track("   ", "Queen"), &track("", "Queen")));
    }

    // --- state transitions -------------------------------------------------

    #[test]
    fn new_game() {
        let state = GameState::new(DAY);
        assert_eq!(state.day(), DAY);
        assert_eq!(state.status(), Status::Playing);
        assert!(state.attempts().is_empty());
        assert!(!state.is_finished());
        assert_eq!(state.unlocked_ms(), 100);
    }

    #[test]
    fn win_on_the_first_try() {
        let mut state = GameState::new(DAY);
        assert_eq!(state.guess(&answer(), &answer()), Ok(Status::Won));
        assert_eq!(state.status(), Status::Won);
        assert!(state.is_finished());
        // The winning guess is not an attempt.
        assert!(state.attempts().is_empty());
        assert_eq!(state.unlocked_ms(), 30_000);
    }

    #[test]
    fn win_with_another_release_of_the_song() {
        let mut state = GameState::new(DAY);
        let live = TrackMeta::new("Under Pressure (Live)", "Under Pressure", "Queen");
        assert_eq!(state.guess(&answer(), &live), Ok(Status::Won));
    }

    #[test]
    fn unlocked_clip_follows_the_ladder() {
        let mut state = GameState::new(DAY);
        for (misses, ms) in LADDER_MS.into_iter().enumerate() {
            assert_eq!(state.status(), Status::Playing);
            assert_eq!(state.attempts().len(), misses);
            assert_eq!(state.unlocked_ms(), ms);

            // Alternate skips and wrong guesses.
            let after = if misses % 2 == 0 {
                state.skip()
            } else {
                state.guess(&answer(), &wrong())
            };
            let expected = if misses + 1 == MAX_ATTEMPTS {
                Status::Lost
            } else {
                Status::Playing
            };
            assert_eq!(after, Ok(expected));
        }
        assert_eq!(state.status(), Status::Lost);
        assert_eq!(state.attempts().len(), 7);
        assert_eq!(state.unlocked_ms(), 30_000);
    }

    #[test]
    fn win_on_the_seventh_try() {
        let mut state = GameState::new(DAY);
        for _ in 0..6 {
            assert_eq!(state.skip(), Ok(Status::Playing));
        }
        assert_eq!(state.unlocked_ms(), 30_000);
        assert_eq!(state.guess(&answer(), &answer()), Ok(Status::Won));
        assert_eq!(state.status(), Status::Won);
        assert_eq!(state.attempts().len(), 6);
        assert_eq!(state.unlocked_ms(), 30_000);
    }

    #[test]
    fn lose_on_the_seventh_miss() {
        let mut state = GameState::new(DAY);
        for _ in 0..6 {
            assert_eq!(state.guess(&answer(), &wrong()), Ok(Status::Playing));
        }
        assert_eq!(state.guess(&answer(), &wrong()), Ok(Status::Lost));
        assert_eq!(state.status(), Status::Lost);
        assert!(state.is_finished());
        assert_eq!(state.attempts().len(), 7);
        assert_eq!(state.unlocked_ms(), 30_000);

        // A skip on the last turn loses too.
        let mut state = GameState::new(DAY);
        for _ in 0..6 {
            assert_eq!(state.skip(), Ok(Status::Playing));
        }
        assert_eq!(state.skip(), Ok(Status::Lost));
    }

    #[test]
    fn attempts_record_what_the_player_did() {
        let mut state = GameState::new(DAY);
        state.skip().unwrap();
        state
            .guess(
                &answer(),
                &TrackMeta::new(
                    "Bohemian Rhapsody (Remastered 2011)",
                    "Bohemian Rhapsody",
                    "Queen",
                ),
            )
            .unwrap();
        assert_eq!(
            state.attempts(),
            [
                Attempt::Skip,
                // The full title, as the player picked it.
                Attempt::Wrong {
                    title: "Bohemian Rhapsody (Remastered 2011)".to_owned(),
                    artist: "Queen".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn moves_after_the_end_are_errors() {
        let mut won = GameState::new(DAY);
        won.skip().unwrap();
        won.guess(&answer(), &answer()).unwrap();
        let before = won.clone();
        assert_eq!(won.skip(), Err(GameError::Finished));
        assert_eq!(won.guess(&answer(), &wrong()), Err(GameError::Finished));
        assert_eq!(won.guess(&answer(), &answer()), Err(GameError::Finished));
        assert_eq!(won, before);

        let mut lost = GameState::new(DAY);
        for _ in 0..7 {
            lost.skip().unwrap();
        }
        let before = lost.clone();
        assert_eq!(lost.skip(), Err(GameError::Finished));
        // Not even the right answer rescues a lost game.
        assert_eq!(lost.guess(&answer(), &answer()), Err(GameError::Finished));
        assert_eq!(lost, before);
    }

    #[test]
    fn day_rollover_resets_and_same_day_keeps() {
        let mut state = GameState::new(DAY);
        state.skip().unwrap();
        state.guess(&answer(), &wrong()).unwrap();

        let same = state.clone().for_day(DAY);
        assert_eq!(same, state);
        assert_eq!(same.unlocked_ms(), 1_000);

        let tomorrow = date(2026, 10, 2);
        let next = state.clone().for_day(tomorrow);
        assert_eq!(next, GameState::new(tomorrow));
        assert_eq!(next.unlocked_ms(), 100);

        // A finished game rolls over as well, and so does a cookie from the future.
        let mut won = GameState::new(DAY);
        won.guess(&answer(), &answer()).unwrap();
        assert_eq!(won.clone().for_day(DAY).status(), Status::Won);
        assert_eq!(won.for_day(tomorrow).status(), Status::Playing);
        assert_eq!(GameState::new(tomorrow).for_day(DAY), GameState::new(DAY));
    }

    // --- stored fields -----------------------------------------------------

    #[test]
    fn attempt_fields_are_cut_to_eighty_characters() {
        let long = "x".repeat(200);
        let Attempt::Wrong { title, artist } = Attempt::wrong(&long, &long) else {
            panic!("not a wrong guess");
        };
        assert_eq!(title.chars().count(), 80);
        assert_eq!(artist.chars().count(), 80);

        // Short text is kept as it is, apart from surrounding whitespace.
        assert_eq!(clip_field("  Under Pressure "), "Under Pressure");
        assert_eq!(clip_field(""), "");
        // Exactly at the limit.
        assert_eq!(clip_field(&"x".repeat(80)).len(), 80);
        // Two-byte characters: 80 of them are 160 bytes, which just fits.
        let accented = clip_field(&"é".repeat(200));
        assert_eq!((accented.chars().count(), accented.len()), (80, 160));
    }

    #[test]
    fn attempt_fields_are_cut_on_character_boundaries_within_the_byte_limit() {
        // Three bytes each: 53 fit in 160 bytes.
        let japanese = clip_field(&"夜".repeat(200));
        assert_eq!((japanese.chars().count(), japanese.len()), (53, 159));
        // Four bytes each.
        let emoji = clip_field(&"🎵".repeat(200));
        assert_eq!((emoji.chars().count(), emoji.len()), (40, 160));
        // Characters JSON escapes count double.
        assert_eq!(clip_field(&"\"".repeat(200)).len(), 80);
        let escaped = serde_json::to_string(&clip_field(&"\\".repeat(200))).unwrap();
        assert_eq!(escaped.len(), 160 + 2);
        // Control characters would cost six bytes each; they are dropped.
        assert_eq!(clip_field("Under\u{0}\tPressure\n"), "UnderPressure");
    }

    // --- serialization -----------------------------------------------------

    #[test]
    fn state_has_the_documented_json_shape() {
        let mut state = GameState::new(DAY);
        state.skip().unwrap();
        state
            .guess(&answer(), &track("Under Pressure", "Vanilla Ice"))
            .unwrap();
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            r#"{"day":"2026-10-01","attempts":[{"kind":"skip"},{"kind":"wrong","title":"Under Pressure","artist":"Vanilla Ice"}],"status":"playing"}"#
        );
        assert_eq!(
            serde_json::to_string(&GameState::new(DAY)).unwrap(),
            r#"{"day":"2026-10-01","attempts":[],"status":"playing"}"#
        );
        assert_eq!(serde_json::to_string(&Status::Won).unwrap(), r#""won""#);
        assert_eq!(serde_json::to_string(&Status::Lost).unwrap(), r#""lost""#);
    }

    #[test]
    fn state_round_trips_through_json() {
        let mut playing = GameState::new(DAY);
        playing.skip().unwrap();
        playing
            .guess(&answer(), &track("Déjà \"Vu\"", "Beyoncé"))
            .unwrap();

        let mut won = playing.clone();
        won.guess(&answer(), &answer()).unwrap();

        let mut lost = playing.clone();
        while lost.status() == Status::Playing {
            lost.skip().unwrap();
        }

        for state in [GameState::new(DAY), playing, won, lost] {
            let json = serde_json::to_string(&state).unwrap();
            let back: GameState = serde_json::from_str(&json).unwrap();
            assert_eq!(back, state, "{json}");
            assert_eq!(back.unlocked_ms(), state.unlocked_ms());
        }
    }

    #[test]
    fn impossible_or_malformed_states_do_not_deserialize() {
        let skips = |n: usize| vec![r#"{"kind":"skip"}"#; n].join(",");
        let state = |attempts: usize, status: &str| {
            format!(
                r#"{{"day":"2026-10-01","attempts":[{}],"status":"{status}"}}"#,
                skips(attempts)
            )
        };
        let parse = |json: &str| serde_json::from_str::<GameState>(json);

        // The reachable corners parse.
        assert!(parse(&state(0, "playing")).is_ok());
        assert!(parse(&state(6, "playing")).is_ok());
        assert!(parse(&state(0, "won")).is_ok());
        assert!(parse(&state(6, "won")).is_ok());
        assert!(parse(&state(7, "lost")).is_ok());

        // Still playing after the last turn, lost early, won after losing.
        assert!(parse(&state(7, "playing")).is_err());
        assert!(parse(&state(50, "playing")).is_err());
        assert!(parse(&state(6, "lost")).is_err());
        assert!(parse(&state(0, "lost")).is_err());
        assert!(parse(&state(7, "won")).is_err());
        assert!(parse(&state(8, "lost")).is_err());
        let error = parse(&state(7, "playing")).unwrap_err().to_string();
        assert!(error.contains("7 attempts"), "{error}");

        // Not a state at all.
        for json in [
            "",
            "{}",
            "null",
            r#"{"day":"2026-13-01","attempts":[],"status":"playing"}"#,
            r#"{"day":"yesterday","attempts":[],"status":"playing"}"#,
            r#"{"day":"2026-10-01","attempts":[],"status":"cheating"}"#,
            r#"{"day":"2026-10-01","attempts":[{"kind":"right"}],"status":"playing"}"#,
            r#"{"day":"2026-10-01","attempts":[{"kind":"wrong"}],"status":"playing"}"#,
            r#"{"day":"2026-10-01","status":"playing"}"#,
        ] {
            assert!(parse(json).is_err(), "{json}");
        }
    }

    /// A lost game whose seven wrong guesses all have `text` as title and artist.
    fn worst_case(text: &str) -> GameState {
        let mut state = GameState::new(DAY);
        let guess = track(text, text);
        while state.status() == Status::Playing {
            state.guess(&answer(), &guess).unwrap();
        }
        assert_eq!(state.attempts().len(), 7);
        state
    }

    #[test]
    fn worst_case_state_fits_in_a_cookie() {
        // Six wrong guesses with 80-character titles and artists, still playing.
        let mut six = GameState::new(DAY);
        let long = "W".repeat(300);
        for _ in 0..6 {
            six.guess(&answer(), &track(&long, &long)).unwrap();
        }
        let json = serde_json::to_string(&six).unwrap();
        assert!(json.len() < 1500, "{} bytes", json.len());

        // The seventh miss is stored as well.
        let json = serde_json::to_string(&worst_case(&long)).unwrap();
        assert!(json.len() < 1500, "{} bytes", json.len());

        // Multi-byte and escaped text is where the byte limit matters. The
        // cookie holds 12 + 16 bytes of nonce and tag on top, base64-encoded.
        for text in ["夜", "é", "🎵", "\"", "\\", "\u{7}"] {
            let json = serde_json::to_string(&worst_case(&text.repeat(300))).unwrap();
            assert!(json.len() < 2700, "{text:?}: {} bytes", json.len());
            let cookie_value = (json.len() + 28).div_ceil(3) * 4;
            assert!(cookie_value < 3700, "{text:?}: {cookie_value} bytes");
        }
    }
}
