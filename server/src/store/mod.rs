//! Persistent data behind one trait: the song pool today; picks, players, games and the day offset later.
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
//!   decides (seeding only an empty pool, the order a list is shown in, and
//!   later the daily pick and the stats) is Rust code that calls it.
//! - A backend only has to be atomic for one record at a time. A song with its
//!   genre tags is one record. Nothing here needs a transaction that spans
//!   two kinds of record.
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
use serde::{Deserialize, Serialize};

pub use self::{memory::MemoryStore, seed::seed_if_empty, sqlite::SqliteStore};
use crate::config::StoreKind;

/// The database file of the SQLite backend, under the data directory.
const SQLITE_FILE: &str = "needledrop.db";

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
}

/// Opens the backend the config names. For SQLite that creates the data
/// directory and the database file when they are missing and brings the
/// schema up to date; a database that cannot be used is an error, so the
/// server stops at startup instead of failing on the first request.
pub fn open(kind: StoreKind, data_dir: &Path) -> Result<Arc<dyn Store>, StoreError> {
    Ok(match kind {
        StoreKind::Sqlite => Arc::new(SqliteStore::open(&data_dir.join(SQLITE_FILE))?),
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
        let data_dir = dir.path().join("data");

        let memory = open(StoreKind::Memory, &data_dir).unwrap();
        assert!(memory.songs().await.unwrap().is_empty());
        // The in-memory backend keeps nothing on disk.
        assert!(!data_dir.exists());

        let sqlite = open(StoreKind::Sqlite, &data_dir).unwrap();
        assert!(sqlite.songs().await.unwrap().is_empty());
        assert!(data_dir.join("needledrop.db").is_file());
    }

    #[test]
    fn a_store_error_reads_as_what_failed_and_why() {
        let error = StoreError::new("opening the database", "disk full");
        assert_eq!(error.to_string(), "opening the database: disk full");
    }
}
