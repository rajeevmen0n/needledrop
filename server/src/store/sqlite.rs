//! The SQLite backend: one database file under the data directory, created and migrated at startup.
//!
//! SQLite is compiled into the server (rusqlite's `bundled` feature), so there
//! is no system library to install and no service to run. Its API blocks, so
//! every operation runs on tokio's blocking pool, on the one connection this
//! store owns. One connection is enough: an operation is a handful of rows,
//! and it makes every method atomic without further thought.
//!
//! The schema is this backend's own business. It is versioned with SQLite's
//! `user_version`, and [`MIGRATIONS`] lists the script that leads to each
//! version; a later task that needs a new table adds a script, and existing
//! databases catch up the next time the server starts.

use std::{
    collections::BTreeMap,
    fmt,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use async_trait::async_trait;
use jiff::civil::Date;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use super::{Genre, Genres, NewSong, PoolSong, Store, StoreError};

/// The script that brings the schema to each version, in order. Version 0 is
/// a database without a schema version: a new file, or the one that was built
/// by hand before the server made its own (see the first script).
const MIGRATIONS: [(i64, &str); 1] = [(1, include_str!("sqlite/001_songs.sql"))];

/// The version this server reads and writes.
const SCHEMA_VERSION: i64 = MIGRATIONS[MIGRATIONS.len() - 1].0;

/// How long a statement waits for another process's lock (someone looking at
/// the file with a SQLite client) before it gives up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The columns [`song_from_row`] reads, in its order.
const SONG_COLUMNS: &str = "track_id, title, title_short, artist, album, preview_failed_on";

/// A [`Store`] kept in a SQLite file.
pub struct SqliteStore {
    /// Locked for the length of one operation, on a blocking thread.
    connection: Arc<Mutex<Connection>>,
}

/// Why an operation failed, before it is flattened into a [`StoreError`].
enum Failure {
    /// SQLite refused or could not do it.
    Sqlite(rusqlite::Error),
    /// The database answered, but with something this server cannot use.
    Data(String),
}

impl From<rusqlite::Error> for Failure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => error.fmt(f),
            Self::Data(message) => f.write_str(message),
        }
    }
}

impl SqliteStore {
    /// Opens the database at `path`, creating the file and its directory when
    /// they are missing, and brings its schema to [`SCHEMA_VERSION`].
    ///
    /// Fails, without changing the file, when it is not a SQLite database,
    /// was written by a newer server, or holds tables of another shape.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|error| {
                StoreError::new(
                    format!("creating the data directory {}", dir.display()),
                    error,
                )
            })?;
        }
        let failed = |failure: Failure| {
            StoreError::new(format!("opening the database {}", path.display()), failure)
        };
        let mut connection = Connection::open(path).map_err(|error| failed(error.into()))?;
        prepare(&mut connection).map_err(failed)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Runs `work` on the connection, off the async threads. `what` names the
    /// operation in the error.
    async fn run<T, F>(&self, what: &'static str, work: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, Failure> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        let outcome = tokio::task::spawn_blocking(move || {
            // A panic in an earlier operation rolled its transaction back as
            // it unwound, so the connection behind a poisoned lock is usable.
            let mut connection = connection.lock().unwrap_or_else(PoisonError::into_inner);
            work(&mut connection)
        })
        .await;
        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(failure)) => Err(StoreError::new(what, failure)),
            Err(error) => Err(StoreError::new(what, error)),
        }
    }
}

/// Sets the connection up and migrates the schema, all or nothing.
fn prepare(connection: &mut Connection) -> Result<(), Failure> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    // A setting of the connection, not of the file, and off in a stock SQLite.
    connection.pragma_update(None, "foreign_keys", true)?;

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !(0..=SCHEMA_VERSION).contains(&found) {
        return Err(Failure::Data(format!(
            "its schema version is {found} and this server knows versions up to \
             {SCHEMA_VERSION}; it was written by a newer server"
        )));
    }
    for (version, script) in MIGRATIONS {
        if version > found {
            transaction.execute_batch(script)?;
        }
    }
    check_schema(&transaction)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

/// Checks that the tables have the columns the queries below name, by
/// compiling a statement over each. Tables that were adopted rather than
/// created here could be anything, and startup is the time to find out.
fn check_schema(connection: &Connection) -> Result<(), Failure> {
    for sql in [
        format!("SELECT {SONG_COLUMNS} FROM songs"),
        "SELECT track_id, genre FROM song_genres".to_owned(),
    ] {
        connection.prepare(&sql).map_err(|error| {
            Failure::Data(format!(
                "its tables are not the ones this server expects ({error})"
            ))
        })?;
    }
    Ok(())
}

/// A track ID as SQLite stores it. SQLite integers are signed, so an ID above
/// `i64::MAX` cannot be in the database; Deezer's are nowhere near.
fn key(track_id: u64) -> Option<i64> {
    i64::try_from(track_id).ok()
}

/// A row of [`SONG_COLUMNS`] as a song without its genres.
fn song_from_row(row: &Row<'_>) -> Result<PoolSong, Failure> {
    let id: i64 = row.get(0)?;
    let track_id = u64::try_from(id)
        .map_err(|_| Failure::Data(format!("the track ID {id} is not a Deezer track ID")))?;
    let failed_on: Option<String> = row.get(5)?;
    let preview_failed_on = failed_on
        .map(|text| {
            text.parse::<Date>().map_err(|_| {
                Failure::Data(format!(
                    "song {id} has the preview-failed day {text:?}, which is not a date"
                ))
            })
        })
        .transpose()?;
    Ok(PoolSong {
        track_id,
        title: row.get(1)?,
        title_short: row.get(2)?,
        artist: row.get(3)?,
        album: row.get(4)?,
        genres: Genres::new(),
        preview_failed_on,
    })
}

/// A stored genre slug as a [`Genre`].
fn genre_from_slug(slug: &str) -> Result<Genre, Failure> {
    Genre::from_slug(slug).ok_or_else(|| Failure::Data(format!("{slug:?} is not a genre")))
}

/// The whole pool by ascending track ID.
fn load_songs(connection: &Connection) -> Result<Vec<PoolSong>, Failure> {
    // Sorted here rather than by the database, so the order is the trait's
    // and not a property of the query.
    let mut songs = BTreeMap::new();
    let mut statement = connection.prepare(&format!("SELECT {SONG_COLUMNS} FROM songs"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let song = song_from_row(row)?;
        songs.insert(song.track_id, song);
    }

    let mut statement = connection.prepare("SELECT track_id, genre FROM song_genres")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let slug: String = row.get(1)?;
        // A tag without its song can only come from an outside client with
        // foreign keys off; it belongs to nothing, so it is not shown.
        if let Some(song) = u64::try_from(id).ok().and_then(|id| songs.get_mut(&id)) {
            song.genres.insert(genre_from_slug(&slug)?);
        }
    }
    Ok(songs.into_values().collect())
}

/// One song with its genres, if it is there.
fn load_song(connection: &Connection, id: i64) -> Result<Option<PoolSong>, Failure> {
    let found = connection
        .query_row(
            &format!("SELECT {SONG_COLUMNS} FROM songs WHERE track_id = ?1"),
            [id],
            |row| Ok(song_from_row(row)),
        )
        .optional()?;
    let Some(mut song) = found.transpose()? else {
        return Ok(None);
    };

    let mut statement = connection.prepare("SELECT genre FROM song_genres WHERE track_id = ?1")?;
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        let slug: String = row.get(0)?;
        song.genres.insert(genre_from_slug(&slug)?);
    }
    Ok(Some(song))
}

/// Makes `genres` the tags of song `id`, dropping the ones it had.
fn replace_genres(connection: &Connection, id: i64, genres: &Genres) -> Result<(), Failure> {
    connection.execute("DELETE FROM song_genres WHERE track_id = ?1", [id])?;
    let mut insert =
        connection.prepare("INSERT INTO song_genres (track_id, genre) VALUES (?1, ?2)")?;
    for genre in genres {
        insert.execute(params![id, genre.slug()])?;
    }
    Ok(())
}

#[async_trait]
impl Store for SqliteStore {
    async fn songs(&self) -> Result<Vec<PoolSong>, StoreError> {
        self.run("listing the song pool", |connection| {
            // One transaction, so both tables are read from the same moment.
            let transaction = connection.transaction()?;
            load_songs(&transaction)
        })
        .await
    }

    async fn song(&self, track_id: u64) -> Result<Option<PoolSong>, StoreError> {
        let Some(id) = key(track_id) else {
            return Ok(None);
        };
        self.run("reading a song", move |connection| {
            let transaction = connection.transaction()?;
            load_song(&transaction, id)
        })
        .await
    }

    async fn add_song(&self, song: NewSong, genres: Genres) -> Result<PoolSong, StoreError> {
        self.run("adding a song", move |connection| {
            let id = key(song.track_id).ok_or_else(|| {
                Failure::Data(format!(
                    "the track ID {} is too large to store",
                    song.track_id
                ))
            })?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // A song already there keeps its row: only the genres change.
            transaction.execute(
                "INSERT INTO songs (track_id, title, title_short, artist, album) \
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (track_id) DO NOTHING",
                params![id, song.title, song.title_short, song.artist, song.album],
            )?;
            replace_genres(&transaction, id, &genres)?;
            let stored = load_song(&transaction, id)?
                .ok_or_else(|| Failure::Data("the song just written is not there".to_owned()))?;
            transaction.commit()?;
            Ok(stored)
        })
        .await
    }

    async fn set_song_genres(
        &self,
        track_id: u64,
        genres: Genres,
    ) -> Result<Option<PoolSong>, StoreError> {
        let Some(id) = key(track_id) else {
            return Ok(None);
        };
        self.run("changing a song's genres", move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if load_song(&transaction, id)?.is_none() {
                return Ok(None);
            }
            replace_genres(&transaction, id, &genres)?;
            let stored = load_song(&transaction, id)?;
            transaction.commit()?;
            Ok(stored)
        })
        .await
    }

    async fn remove_song(&self, track_id: u64) -> Result<bool, StoreError> {
        let Some(id) = key(track_id) else {
            return Ok(false);
        };
        self.run("removing a song", move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Deleted by hand as well as by the foreign key's cascade, which
            // only works on a connection that switched foreign keys on.
            transaction.execute("DELETE FROM song_genres WHERE track_id = ?1", [id])?;
            let removed = transaction.execute("DELETE FROM songs WHERE track_id = ?1", [id])?;
            transaction.commit()?;
            Ok(removed > 0)
        })
        .await
    }

    async fn set_preview_failed_on(
        &self,
        track_id: u64,
        day: Option<Date>,
    ) -> Result<bool, StoreError> {
        let Some(id) = key(track_id) else {
            return Ok(false);
        };
        self.run("recording a failed preview check", move |connection| {
            let day = day.map(|day| day.to_string());
            let updated = connection.execute(
                "UPDATE songs SET preview_failed_on = ?1 WHERE track_id = ?2",
                params![day, id],
            )?;
            Ok(updated > 0)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::contract::{Fixture, contract_tests};
    use jiff::civil::date;

    /// A store on a new database in a temporary directory.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("test.db")).unwrap();
        Fixture::in_dir(store, dir)
    }

    contract_tests!(fixture());

    /// The schema of the database that was built by hand from `seed.sql`
    /// before the server created its own, with two of its rows.
    const HAND_BUILT: &str = "
        CREATE TABLE IF NOT EXISTS songs (
          track_id          INTEGER PRIMARY KEY,
          title             TEXT NOT NULL,
          title_short       TEXT NOT NULL,
          artist            TEXT NOT NULL,
          album             TEXT NOT NULL,
          added_at          TEXT NOT NULL DEFAULT (datetime('now')),
          preview_failed_on TEXT
        );
        CREATE TABLE IF NOT EXISTS song_genres (
          track_id INTEGER NOT NULL REFERENCES songs(track_id) ON DELETE CASCADE,
          genre    TEXT NOT NULL CHECK (genre IN ('pop', 'rock', 'hip-hop')),
          PRIMARY KEY (track_id, genre)
        );
        INSERT INTO songs (track_id, title, title_short, artist, album, added_at)
          VALUES (4603408, 'Billie Jean', 'Billie Jean', 'Michael Jackson',
                  'Michael Jackson''s This Is It', '2026-10-01 12:18:43');
        INSERT INTO song_genres (track_id, genre) VALUES (4603408, 'pop');
        INSERT INTO songs (track_id, title, title_short, artist, album, added_at)
          VALUES (4091937401, 'Bohemian Rhapsody', 'Bohemian Rhapsody', 'Queen',
                  'A Night At The Opera', '2026-10-01 12:18:43');
        INSERT INTO song_genres (track_id, genre) VALUES (4091937401, 'rock');
    ";

    fn new_song(track_id: u64, title: &str, artist: &str) -> NewSong {
        NewSong {
            track_id,
            title: title.to_owned(),
            title_short: title.to_owned(),
            artist: artist.to_owned(),
            album: format!("{title} (album)"),
        }
    }

    /// The schema version in the file, read with a connection of the test's own.
    fn user_version(path: &Path) -> i64 {
        Connection::open(path)
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    /// Runs `sql` on the file with a connection of the test's own.
    fn execute(path: &Path, sql: &str) {
        Connection::open(path).unwrap().execute_batch(sql).unwrap();
    }

    #[test]
    fn the_migrations_are_numbered_from_one_without_gaps() {
        for (index, (version, script)) in MIGRATIONS.iter().enumerate() {
            assert_eq!(*version, i64::try_from(index).unwrap() + 1);
            assert!(!script.trim().is_empty());
        }
        assert_eq!(SCHEMA_VERSION, i64::try_from(MIGRATIONS.len()).unwrap());
    }

    #[tokio::test]
    async fn opening_creates_the_directory_the_file_and_the_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not").join("there").join("pool.db");

        let store = SqliteStore::open(&path).unwrap();
        assert!(path.is_file());
        assert!(store.songs().await.unwrap().is_empty());
        assert_eq!(user_version(&path), SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn the_pool_survives_reopening_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool.db");

        let store = SqliteStore::open(&path).unwrap();
        store
            .add_song(
                new_song(92_720_046, "Back In Black", "AC/DC"),
                Genres::from([Genre::Rock, Genre::Pop]),
            )
            .await
            .unwrap();
        store
            .add_song(new_song(7, "Untagged", "Nobody"), Genres::new())
            .await
            .unwrap();
        store
            .set_preview_failed_on(7, Some(date(2026, 10, 4)))
            .await
            .unwrap();
        store
            .add_song(new_song(8, "Removed", "Nobody"), Genres::from([Genre::Pop]))
            .await
            .unwrap();
        store.remove_song(8).await.unwrap();
        let before = store.songs().await.unwrap();
        drop(store);

        // A restart: the same file, a new connection, nothing migrated twice.
        let reopened = SqliteStore::open(&path).unwrap();
        let after = reopened.songs().await.unwrap();
        assert_eq!(after, before);
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].track_id, 7);
        assert_eq!(after[0].preview_failed_on, Some(date(2026, 10, 4)));
        assert_eq!(after[1].genres, Genres::from([Genre::Pop, Genre::Rock]));
        assert_eq!(user_version(&path), SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn the_hand_built_database_is_adopted_with_its_songs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("needledrop.db");
        execute(&path, HAND_BUILT);
        assert_eq!(user_version(&path), 0);

        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(user_version(&path), SCHEMA_VERSION);
        let songs = store.songs().await.unwrap();
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].track_id, 4_603_408);
        assert_eq!(songs[0].title, "Billie Jean");
        assert_eq!(songs[0].album, "Michael Jackson's This Is It");
        assert_eq!(songs[0].genres, Genres::from([Genre::Pop]));
        assert_eq!(songs[0].preview_failed_on, None);
        // An ID that does not fit in 32 bits.
        assert_eq!(songs[1].track_id, 4_091_937_401);
        assert_eq!(songs[1].genres, Genres::from([Genre::Rock]));

        // And it is a working store from then on.
        store
            .add_song(
                new_song(15_391_618, "Toxic", "Britney Spears"),
                Genres::new(),
            )
            .await
            .unwrap();
        assert!(store.remove_song(4_603_408).await.unwrap());
        drop(store);
        let reopened = SqliteStore::open(&path).unwrap();
        let ids: Vec<u64> = reopened
            .songs()
            .await
            .unwrap()
            .iter()
            .map(|song| song.track_id)
            .collect();
        assert_eq!(ids, vec![15_391_618, 4_091_937_401]);
    }

    #[test]
    fn a_database_from_a_newer_server_is_refused_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool.db");
        execute(&path, HAND_BUILT);
        execute(&path, "PRAGMA user_version = 99");

        let error = SqliteStore::open(&path).err().unwrap().to_string();
        assert!(error.contains("pool.db"), "{error}");
        assert!(error.contains("schema version is 99"), "{error}");
        assert!(error.contains("newer server"), "{error}");
        assert_eq!(user_version(&path), 99);
    }

    #[test]
    fn a_file_that_is_not_a_database_is_refused_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool.db");
        let garbage = "this is a text file, not a SQLite database\n".repeat(40);
        std::fs::write(&path, &garbage).unwrap();

        let error = SqliteStore::open(&path).err().unwrap().to_string();
        assert!(error.contains("opening the database"), "{error}");
        assert!(error.contains("pool.db"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), garbage);
    }

    #[test]
    fn tables_of_another_shape_are_refused_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool.db");
        execute(
            &path,
            "CREATE TABLE songs (id INTEGER PRIMARY KEY, name TEXT)",
        );

        let error = SqliteStore::open(&path).err().unwrap().to_string();
        assert!(error.contains("pool.db"), "{error}");
        assert!(
            error.contains("not the ones this server expects"),
            "{error}"
        );
        // The migration was rolled back: no version, no second table.
        assert_eq!(user_version(&path), 0);
        let tables: i64 = Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'song_genres'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
    }

    #[tokio::test]
    async fn a_stored_day_that_is_not_a_date_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool.db");
        let store = SqliteStore::open(&path).unwrap();
        store
            .add_song(new_song(7, "Song", "Someone"), Genres::new())
            .await
            .unwrap();
        execute(
            &path,
            "UPDATE songs SET preview_failed_on = 'last tuesday' WHERE track_id = 7",
        );

        for error in [
            store.songs().await.unwrap_err(),
            store.song(7).await.unwrap_err(),
        ] {
            let message = error.to_string();
            assert!(message.contains("last tuesday"), "{message}");
            assert!(message.contains("not a date"), "{message}");
        }
    }

    #[tokio::test]
    async fn a_tag_without_its_song_is_not_listed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool.db");
        let store = SqliteStore::open(&path).unwrap();
        store
            .add_song(new_song(7, "Song", "Someone"), Genres::from([Genre::Pop]))
            .await
            .unwrap();
        // A stock SQLite client leaves foreign keys off unless told otherwise,
        // so it can leave a tag behind.
        execute(
            &path,
            "PRAGMA foreign_keys = OFF;
             INSERT INTO song_genres (track_id, genre) VALUES (99, 'rock');",
        );

        let songs = store.songs().await.unwrap();
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].genres, Genres::from([Genre::Pop]));
        // Adding that track later does not inherit the stray tag.
        let added = store
            .add_song(new_song(99, "Late", "Someone"), Genres::new())
            .await
            .unwrap();
        assert!(added.genres.is_empty());
    }

    #[tokio::test]
    async fn a_track_id_sqlite_cannot_hold_is_never_there() {
        let fixture = fixture();
        let store = fixture.store();
        let huge = u64::MAX;

        assert_eq!(store.song(huge).await.unwrap(), None);
        assert_eq!(
            store.set_song_genres(huge, Genres::new()).await.unwrap(),
            None
        );
        assert!(!store.remove_song(huge).await.unwrap());
        assert!(!store.set_preview_failed_on(huge, None).await.unwrap());

        let error = store
            .add_song(new_song(huge, "Song", "Someone"), Genres::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("too large"), "{error}");
        assert!(store.songs().await.unwrap().is_empty());

        // The largest ID it can hold is fine.
        let largest = u64::try_from(i64::MAX).unwrap();
        let stored = store
            .add_song(new_song(largest, "Song", "Someone"), Genres::new())
            .await
            .unwrap();
        assert_eq!(stored.track_id, largest);
        assert_eq!(store.song(largest).await.unwrap(), Some(stored));
    }
}
