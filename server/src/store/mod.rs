//! Persistent data behind one trait: the song pool, the players' games, the daily picks and the day offset.
//!
//! [`Store`] is the only way the rest of the server touches persistent data.
//! Handlers hold an `Arc<dyn Store>`, and no SQL, connection or database error
//! type appears outside a backend's own file, so replacing SQLite with MySQL,
//! Postgres or a document store means writing one new implementation and
//! nothing else.
//!
//! Three rules keep that true:
//!
//! - The methods are domain operations ("add this song", "remove it"), not
//!   queries, and they take and return the plain types defined here.
//! - Logic stays above the trait. A backend fetches and stores; anything that
//!   decides (seeding only an empty pool, the order a list is shown in, the
//!   stats, the daily pick, what a re-roll or a reset deletes and in which
//!   order) is Rust code that calls it.
//! - A backend only has to be atomic for one record at a time. A song with its
//!   genre tags is one record, and so are one player's game in one section on
//!   one day, one section's pick for one day, and the day offset. Nothing
//!   here needs a transaction that spans two kinds of record.
//!
//! [`contract`] holds the test suite every backend has to pass.

mod memory;
mod seed;
mod sqlite;

#[cfg(test)]
mod contract;

use std::{collections::BTreeSet, fmt, path::Path, sync::Arc};

use async_trait::async_trait;
use jiff::civil::Date;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(test)]
pub use self::seed::SEED_SONGS;
pub use self::{memory::MemoryStore, seed::seed_if_empty, sqlite::SqliteStore};
use crate::{config::StoreKind, game::GameState};

/// A genre a song can be tagged with. Each one is also a daily section whose
/// pool is the songs carrying the tag; every song is in the General pool
/// whatever its tags.
///
/// Serialized as its slug, the form used in URLs, the API and the database:
/// `"pop"`, `"rock"` or `"hip-hop"`. The declaration order is the order a
/// song's genres are listed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Genre {
    Pop,
    Rock,
    HipHop,
}

impl Genre {
    /// Every genre, in listing order.
    pub const ALL: [Self; 3] = [Self::Pop, Self::Rock, Self::HipHop];

    /// `"pop"`, `"rock"` or `"hip-hop"`.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Pop => "pop",
            Self::Rock => "rock",
            Self::HipHop => "hip-hop",
        }
    }

    /// The genre with this exact slug.
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|genre| genre.slug() == slug)
    }
}

impl fmt::Display for Genre {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// A daily section: its own song of the day and its own seven-attempt game.
///
/// General draws on the whole pool; every other section is one [`Genre`] and
/// draws on the songs tagged with it. The genre sits inside the section
/// instead of the four names being listed a second time, so the two cannot
/// drift apart: a genre is a section by construction, and "which songs may
/// this section play" is [`genre`](Self::genre) and nothing else.
///
/// Serialized as its slug, the form used in URLs, the API and the database:
/// `"general"`, `"pop"`, `"rock"` or `"hip-hop"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    General,
    Genre(Genre),
}

impl Section {
    /// Every section, in the order they are shown: General first, then the
    /// genres in [`Genre::ALL`]'s order.
    pub const ALL: [Self; 4] = [
        Self::General,
        Self::Genre(Genre::Pop),
        Self::Genre(Genre::Rock),
        Self::Genre(Genre::HipHop),
    ];

    /// `"general"`, or the genre's slug.
    pub fn slug(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Genre(genre) => genre.slug(),
        }
    }

    /// The section with this exact slug.
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.slug() == slug)
    }

    /// The genre whose songs this section plays; `None` for General, which
    /// plays them all.
    pub fn genre(self) -> Option<Genre> {
        match self {
            Self::General => None,
            Self::Genre(genre) => Some(genre),
        }
    }
}

impl From<Genre> for Section {
    fn from(genre: Genre) -> Self {
        Self::Genre(genre)
    }
}

impl fmt::Display for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

impl Serialize for Section {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.slug())
    }
}

impl<'de> Deserialize<'de> for Section {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let slug = String::deserialize(deserializer)?;
        Self::from_slug(&slug).ok_or_else(|| {
            serde::de::Error::unknown_variant(&slug, &["general", "pop", "rock", "hip-hop"])
        })
    }
}

/// Characters in a [`PlayerId`]: 128 bits as hexadecimal.
const PLAYER_ID_CHARS: usize = 32;

/// An anonymous player: the only thing the server knows about who is playing.
///
/// 128 random bits, written as 32 lowercase hexadecimal characters. There is
/// no record of a player on its own: a player is the games stored under
/// their ID. One who has never made a move takes up no space, and an ID the
/// store has not seen is simply a player without games.
///
/// The ID alone opens nothing. It reaches a browser only inside the encrypted
/// cookie, and a client cannot make a cookie for an ID of its choosing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlayerId(String);

impl PlayerId {
    /// A new ID that cannot be guessed: `rand`'s thread-local generator is a
    /// cryptographic one, seeded by the operating system.
    pub fn generate() -> Self {
        Self(format!("{:032x}", rand::random::<u128>()))
    }

    /// `text` as an ID, if it has exactly the form [`generate`](Self::generate)
    /// produces. Anything else was never issued by this server.
    pub fn parse(text: &str) -> Option<Self> {
        let well_formed = text.len() == PLAYER_ID_CHARS
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        well_formed.then(|| Self(text.to_owned()))
    }

    /// The 32 hexadecimal characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The genre tags of one song: none, one or several. A set, so a tag cannot
/// be there twice and the order is always [`Genre::ALL`]'s.
pub type Genres = BTreeSet<Genre>;

/// What is known about a track when it is added to the pool: its Deezer ID
/// and the text shown for it.
///
/// No audio and no preview URL. Previews are signed and expire within
/// minutes, so the MP3 is fetched when a song is picked for a day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSong {
    /// Deezer track ID.
    pub track_id: u64,
    /// Deezer's full title, version included.
    pub title: String,
    /// The title without "(Remastered …)" and the like. May be empty.
    pub title_short: String,
    /// The primary artist.
    pub artist: String,
    pub album: String,
}

/// A song in the pool, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSong {
    /// Deezer track ID; the song's identity in the pool.
    pub track_id: u64,
    pub title: String,
    pub title_short: String,
    pub artist: String,
    pub album: String,
    pub genres: Genres,
    /// The day the daily pick last found this track without a preview, if it
    /// ever has. The song stays in the pool; this is for the owner to see.
    pub preview_failed_on: Option<Date>,
}

impl PoolSong {
    /// `song` as it is stored when first added: tagged, and with no failed
    /// preview check on record.
    pub fn new(song: NewSong, genres: Genres) -> Self {
        Self {
            track_id: song.track_id,
            title: song.title,
            title_short: song.title_short,
            artist: song.artist,
            album: song.album,
            genres,
            preview_failed_on: None,
        }
    }
}

/// The song a section plays on a day: one row of the pick history.
///
/// Only the track ID is kept. The song may leave the pool afterwards; the
/// pick stays, because the games of that day were played against it and the
/// history is what keeps a section from repeating itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pick {
    pub day: Date,
    pub section: Section,
    /// Deezer track ID.
    pub track_id: u64,
}

/// A store operation that failed: the database could not be opened, read or
/// written, or holds something this server cannot interpret.
///
/// Opaque on purpose. It carries a sentence for the log and nothing a caller
/// could match on, so no backend's error type leaks through the trait. The
/// text can name files and tracks; it must never be sent to a client.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct StoreError(String);

impl StoreError {
    /// An error reading "`what`: `cause`".
    pub fn new(what: impl fmt::Display, cause: impl fmt::Display) -> Self {
        Self(format!("{what}: {cause}"))
    }
}

/// Everything the server keeps between restarts.
///
/// "Not there" is an answer (`None` / `false`), not an error; an error means
/// the backend itself failed. Each method is atomic on its own and nothing
/// more is promised: two calls are two operations.
#[async_trait]
pub trait Store: Send + Sync {
    /// The whole pool, by ascending track ID: an order every backend can
    /// produce and that does not change between calls. Callers that show the
    /// list sort it themselves.
    async fn songs(&self) -> Result<Vec<PoolSong>, StoreError>;

    /// The song with this track ID, if it is in the pool.
    async fn song(&self, track_id: u64) -> Result<Option<PoolSong>, StoreError>;

    /// Adds a song with its genre tags and returns it as stored.
    ///
    /// If the pool already has a song with that track ID, only its genres are
    /// replaced: the stored title, artist, album and failed-preview day are
    /// kept, exactly as [`set_song_genres`](Self::set_song_genres) would
    /// leave them. Two requests adding the same song therefore agree.
    async fn add_song(&self, song: NewSong, genres: Genres) -> Result<PoolSong, StoreError>;

    /// Replaces the genre tags of a song and returns it as stored, or `None`
    /// when the pool has no such song. An empty set leaves the song in the
    /// General pool only.
    async fn set_song_genres(
        &self,
        track_id: u64,
        genres: Genres,
    ) -> Result<Option<PoolSong>, StoreError>;

    /// Removes a song, genre tags included. `false` when it was not there.
    async fn remove_song(&self, track_id: u64) -> Result<bool, StoreError>;

    /// Records the day a song's preview last failed the daily check, or
    /// clears the record with `None`. `false` when the pool has no such song.
    async fn set_preview_failed_on(
        &self,
        track_id: u64,
        day: Option<Date>,
    ) -> Result<bool, StoreError>;

    /// The game `player` has in `section` on `day`, or `None` when they have
    /// made no move there. A game is stored with its first move, never by a
    /// visit, so "not there" means a fresh game.
    ///
    /// A stored game this server cannot read (see [`games`](Self::games)) is
    /// `None` too.
    async fn game(
        &self,
        player: &PlayerId,
        section: Section,
        day: Date,
    ) -> Result<Option<GameState>, StoreError>;

    /// Stores `game` as the player's game in `section` on the game's own day
    /// ([`GameState::day`]), replacing the one stored there, if any. A player
    /// has at most one game per section and day.
    ///
    /// The replacement is whole and unconditional: the last save wins. That a
    /// save is based on the latest state is the caller's business (see
    /// `player::MoveLocks`).
    async fn save_game(
        &self,
        player: &PlayerId,
        section: Section,
        game: &GameState,
    ) -> Result<(), StoreError>;

    /// Every game `player` has in `section`, finished or not, by ascending
    /// day. The stats are computed from this list.
    ///
    /// A stored game that cannot be read as a [`GameState`] is left out, here
    /// and in [`game`](Self::game), and the backend says so in the log
    /// instead of failing. Only something other than this server can have
    /// written it (an edit by hand, a later version), and an error would lock
    /// the player out of the day and of their stats until someone repaired
    /// the database, where leaving it out costs them one game. The next
    /// [`save_game`](Self::save_game) for that day replaces it.
    async fn games(
        &self,
        player: &PlayerId,
        section: Section,
    ) -> Result<Vec<GameState>, StoreError>;

    /// Deletes everything stored for `player`: their games in every section
    /// and on every day. Returns how many games that was; 0 for a player the
    /// store has never seen. Other players are not touched.
    async fn delete_player(&self, player: &PlayerId) -> Result<usize, StoreError>;

    /// Deletes every player's game in `section` on `day`: what a re-roll of
    /// that section's song does, since those games were played against the
    /// song that is being replaced. Returns how many games that was. Other
    /// days and other sections are not touched.
    async fn delete_games(&self, section: Section, day: Date) -> Result<usize, StoreError>;

    /// The picks made for `day`, at most one per section, in [`Section`]'s
    /// order (General first). A section that is missing has no pick yet.
    async fn picks_on(&self, day: Date) -> Result<Vec<Pick>, StoreError>;

    /// Every pick `section` has on record, on any day, by ascending day. The
    /// daily pick reads it to avoid repeating a song.
    async fn pick_history(&self, section: Section) -> Result<Vec<Pick>, StoreError>;

    /// Stores `pick` unless its day and section already have one, and returns
    /// the pick that stands: `pick` itself, or the one that was there first.
    ///
    /// This is the one conditional write the trait asks for, and it has to be
    /// atomic: two requests that both find a day without a song at midnight
    /// each draw one, and both must end up playing the same.
    async fn save_pick(&self, pick: Pick) -> Result<Pick, StoreError>;

    /// Removes the pick of `section` on `day`, so that the next
    /// [`save_pick`](Self::save_pick) for them stands. `false` when there was
    /// none. The games of that day are not touched; see
    /// [`delete_games`](Self::delete_games).
    async fn remove_pick(&self, day: Date, section: Section) -> Result<bool, StoreError>;

    /// How many days the server's day is ahead of the real UTC date (behind
    /// it, when negative). 0 until it is set.
    async fn day_offset(&self) -> Result<i64, StoreError>;

    /// Replaces the day offset.
    async fn set_day_offset(&self, days: i64) -> Result<(), StoreError>;

    /// Deletes every game of every player and every pick of every day: what
    /// Reset to day 1 does. The song pool, failed-preview days included, and
    /// the day offset stay as they are.
    ///
    /// The games go first. A backend that cannot delete both kinds of record
    /// at once must keep that order, so that an interruption leaves picks
    /// without games (a day nobody has played yet) and never games without
    /// the pick they were played against.
    async fn wipe_games_and_picks(&self) -> Result<(), StoreError>;
}

/// Reads a game that a backend keeps as JSON ([`GameState`]'s own
/// serialization), filed under `day`. For the backends that store it so.
///
/// `None`, with a warning in the log, when the text is not a possible game or
/// is a game for another day than the one it is filed under: the "cannot be
/// read" of [`Store::games`]. The checks are the ones [`GameState`] makes
/// whenever it is deserialized, so a state that no sequence of moves produces
/// (still playing after seven misses, say) is refused here as it was when the
/// state lived in a cookie.
fn decode_game(player: &PlayerId, section: Section, day: &str, json: &str) -> Option<GameState> {
    match serde_json::from_str::<GameState>(json) {
        Ok(game) if game.day().to_string() == day => Some(game),
        Ok(game) => {
            tracing::warn!(
                %player, %section, day, stored_day = %game.day(),
                "ignoring a stored game that is filed under another day than its own"
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                %player, %section, day, %error,
                "ignoring a stored game that cannot be read"
            );
            None
        }
    }
}

/// Opens the backend the config names. For SQLite that is the file at
/// `database` (`Config::store_path`): its directory and the file itself are
/// created when they are missing and the schema is brought up to date; a
/// database that cannot be used is an error, so the server stops at startup
/// instead of failing on the first request. The in-memory backend has no file
/// and does not look at the path.
pub fn open(kind: StoreKind, database: &Path) -> Result<Arc<dyn Store>, StoreError> {
    Ok(match kind {
        StoreKind::Sqlite => Arc::new(SqliteStore::open(database)?),
        StoreKind::Memory => Arc::new(MemoryStore::new()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn genres_serialize_as_their_slugs() {
        assert_eq!(
            serde_json::to_value(Genre::ALL).unwrap(),
            json!(["pop", "rock", "hip-hop"])
        );
        for genre in Genre::ALL {
            assert_eq!(serde_json::to_value(genre).unwrap(), json!(genre.slug()));
            assert_eq!(
                serde_json::from_value::<Genre>(json!(genre.slug())).unwrap(),
                genre
            );
            assert_eq!(Genre::from_slug(genre.slug()), Some(genre));
            assert_eq!(genre.to_string(), genre.slug());
        }
    }

    #[test]
    fn only_the_exact_slugs_are_genres() {
        for slug in [
            "", "general", "Pop", "hiphop", "hip_hop", "hip-hop ", "jazz",
        ] {
            assert_eq!(Genre::from_slug(slug), None, "{slug:?}");
            assert!(
                serde_json::from_value::<Genre>(json!(slug)).is_err(),
                "{slug:?}"
            );
        }
    }

    #[test]
    fn sections_serialize_as_their_slugs() {
        assert_eq!(
            serde_json::to_value(Section::ALL).unwrap(),
            json!(["general", "pop", "rock", "hip-hop"])
        );
        for section in Section::ALL {
            assert_eq!(
                serde_json::to_value(section).unwrap(),
                json!(section.slug())
            );
            assert_eq!(
                serde_json::from_value::<Section>(json!(section.slug())).unwrap(),
                section
            );
            assert_eq!(Section::from_slug(section.slug()), Some(section));
            assert_eq!(section.to_string(), section.slug());
        }
    }

    #[test]
    fn only_the_exact_slugs_are_sections() {
        for slug in ["", "General", "all", "hiphop", "hip_hop", " pop", "jazz"] {
            assert_eq!(Section::from_slug(slug), None, "{slug:?}");
            assert!(
                serde_json::from_value::<Section>(json!(slug)).is_err(),
                "{slug:?}"
            );
        }
        assert!(serde_json::from_value::<Section>(json!(1)).is_err());
        assert!(serde_json::from_value::<Section>(json!(null)).is_err());
    }

    #[test]
    fn a_section_is_general_or_one_genre() {
        assert_eq!(Section::General.genre(), None);
        // Every genre is a section with the genre's own slug, and they follow
        // General in the genres' order.
        for (genre, section) in Genre::ALL.into_iter().zip(&Section::ALL[1..]) {
            assert_eq!(Section::from(genre), *section);
            assert_eq!(section.genre(), Some(genre));
            assert_eq!(section.slug(), genre.slug());
        }
        assert_eq!(Section::ALL.len(), Genre::ALL.len() + 1);
        assert_eq!(Section::ALL[0], Section::General);
    }

    #[test]
    fn generated_player_ids_are_well_formed_and_never_the_same() {
        let ids: BTreeSet<PlayerId> = (0..1000).map(|_| PlayerId::generate()).collect();
        assert_eq!(ids.len(), 1000);
        for id in &ids {
            assert_eq!(id.as_str().len(), 32);
            assert_eq!(PlayerId::parse(id.as_str()).as_ref(), Some(id));
            assert_eq!(id.to_string(), id.as_str());
        }
    }

    #[test]
    fn only_thirty_two_lowercase_hex_characters_are_a_player_id() {
        let good = "0123456789abcdef0123456789abcdef";
        assert_eq!(PlayerId::parse(good).unwrap().as_str(), good);
        // Leading zeros are part of it: the ID is text, not a number.
        assert!(PlayerId::parse(&"0".repeat(32)).is_some());

        for bad in [
            "",
            "0123456789abcdef",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789ABCDEF0123456789ABCDEF",
            "0123456789abcdef0123456789abcdeg",
            " 123456789abcdef0123456789abcdef",
            "0123456789abcdef-0123456789abcde",
            "éééééééééééééééé",
            r#"{"day":"2026-10-01","attempts":[],"status":"playing"}"#,
        ] {
            assert_eq!(PlayerId::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_stored_game_is_read_only_when_it_is_possible_and_filed_under_its_own_day() {
        let player = PlayerId::generate();
        let read = |day: &str, json: &str| decode_game(&player, Section::General, day, json);

        let json = r#"{"day":"2026-10-01","attempts":[{"kind":"skip"}],"status":"playing"}"#;
        let game = read("2026-10-01", json).unwrap();
        assert_eq!(game.day(), jiff::civil::date(2026, 10, 1));
        assert_eq!(game.attempts().len(), 1);

        // The same game filed under another day is not that day's game.
        assert_eq!(read("2026-10-02", json), None);
        // A state no sequence of moves produces: lost without a miss.
        assert_eq!(
            read(
                "2026-10-01",
                r#"{"day":"2026-10-01","attempts":[],"status":"lost"}"#
            ),
            None
        );
        for garbage in ["", "{}", "null", "won", "{\"day\":\"2026-10-01\""] {
            assert_eq!(read("2026-10-01", garbage), None, "{garbage:?}");
        }
    }

    #[test]
    fn a_genre_set_lists_each_genre_once_in_a_fixed_order() {
        let genres: Genres = [Genre::HipHop, Genre::Pop, Genre::HipHop, Genre::Rock]
            .into_iter()
            .collect();
        assert_eq!(
            serde_json::to_value(&genres).unwrap(),
            json!(["pop", "rock", "hip-hop"])
        );
    }

    #[test]
    fn a_new_song_has_no_failed_preview_on_record() {
        let song = PoolSong::new(
            NewSong {
                track_id: 7,
                title: "Song (Live)".to_owned(),
                title_short: "Song".to_owned(),
                artist: "Someone".to_owned(),
                album: "LP".to_owned(),
            },
            Genres::from([Genre::Rock]),
        );
        assert_eq!(song.track_id, 7);
        assert_eq!(song.title, "Song (Live)");
        assert_eq!(song.title_short, "Song");
        assert_eq!(song.genres, Genres::from([Genre::Rock]));
        assert_eq!(song.preview_failed_on, None);
    }

    #[tokio::test]
    async fn the_config_chooses_the_backend() {
        let dir = tempfile::tempdir().unwrap();
        // Wherever the config says, directories that are not there included.
        let database = dir.path().join("deep/down/game.sqlite");

        let memory = open(StoreKind::Memory, &database).unwrap();
        assert!(memory.songs().await.unwrap().is_empty());
        // The in-memory backend keeps nothing on disk.
        assert!(!dir.path().join("deep").exists());

        let sqlite = open(StoreKind::Sqlite, &database).unwrap();
        assert!(sqlite.songs().await.unwrap().is_empty());
        assert!(database.is_file());
    }

    #[test]
    fn a_store_error_reads_as_what_failed_and_why() {
        let error = StoreError::new("opening the database", "disk full");
        assert_eq!(error.to_string(), "opening the database: disk full");
    }
}
