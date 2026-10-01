//! The six songs a new pool starts with, and the rule for putting them there.
//!
//! The list is plain data and goes in through [`Store`], so every backend is
//! seeded the same way and none of them carries its own copy.

use super::{Genre, Genres, NewSong, Store, StoreError};

/// A song of the starting pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedSong {
    /// Deezer track ID.
    pub track_id: u64,
    /// Deezer's title. None of these has a version suffix, so it is the short
    /// title as well.
    pub title: &'static str,
    pub artist: &'static str,
    pub album: &'static str,
    pub genres: &'static [Genre],
}

/// Two songs per genre, chosen on 2026-10-01. That day each was `readable` on
/// Deezer and its preview downloaded as a full 479,827-byte MP3. Nothing
/// guarantees they stay available; the daily pick checks.
pub const SEED_SONGS: [SeedSong; 6] = [
    SeedSong {
        track_id: 4_603_408,
        title: "Billie Jean",
        artist: "Michael Jackson",
        album: "Michael Jackson's This Is It",
        genres: &[Genre::Pop],
    },
    SeedSong {
        track_id: 15_391_618,
        title: "Toxic",
        artist: "Britney Spears",
        album: "In The Zone",
        genres: &[Genre::Pop],
    },
    SeedSong {
        track_id: 4_091_937_401,
        title: "Bohemian Rhapsody",
        artist: "Queen",
        album: "A Night At The Opera",
        genres: &[Genre::Rock],
    },
    SeedSong {
        track_id: 92_720_046,
        title: "Back In Black",
        artist: "AC/DC",
        album: "Back In Black",
        genres: &[Genre::Rock],
    },
    SeedSong {
        track_id: 1_109_731,
        title: "Lose Yourself",
        artist: "Eminem",
        album: "Curtain Call: The Hits",
        genres: &[Genre::HipHop],
    },
    SeedSong {
        track_id: 3_616_616,
        title: "Juicy",
        artist: "The Notorious B.I.G.",
        album: "Greatest Hits",
        genres: &[Genre::HipHop],
    },
];

/// Adds [`SEED_SONGS`] to a pool that has no song at all, and returns how
/// many were added: six for a new database, zero otherwise.
///
/// "Empty" is the whole test, so a seed song the owner removed stays removed
/// for as long as the pool has any other song in it. The check and the adds
/// are separate operations, which is fine for the one caller: startup, before
/// the server accepts a request.
pub async fn seed_if_empty(store: &dyn Store) -> Result<usize, StoreError> {
    if !store.songs().await?.is_empty() {
        return Ok(0);
    }
    for seed in SEED_SONGS {
        let song = NewSong {
            track_id: seed.track_id,
            title: seed.title.to_owned(),
            title_short: seed.title.to_owned(),
            artist: seed.artist.to_owned(),
            album: seed.album.to_owned(),
        };
        store
            .add_song(song, Genres::from_iter(seed.genres.iter().copied()))
            .await?;
    }
    Ok(SEED_SONGS.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MemoryStore, SqliteStore};
    use std::collections::HashSet;

    #[test]
    fn the_seed_is_six_distinct_songs_two_per_genre() {
        let ids: HashSet<u64> = SEED_SONGS.iter().map(|song| song.track_id).collect();
        assert_eq!(ids.len(), 6);
        for genre in Genre::ALL {
            let tagged = SEED_SONGS
                .iter()
                .filter(|song| song.genres == [genre])
                .count();
            assert_eq!(tagged, 2, "{genre}");
        }
        for song in SEED_SONGS {
            assert!(song.track_id > 0);
            assert!(!song.title.is_empty() && !song.artist.is_empty() && !song.album.is_empty());
        }
    }

    #[tokio::test]
    async fn an_empty_pool_gets_the_six_songs() {
        let store = MemoryStore::new();
        assert_eq!(seed_if_empty(&store).await.unwrap(), 6);

        let pool = store.songs().await.unwrap();
        assert_eq!(pool.len(), 6);
        let juicy = store.song(3_616_616).await.unwrap().unwrap();
        assert_eq!(juicy.title, "Juicy");
        assert_eq!(juicy.title_short, "Juicy");
        assert_eq!(juicy.artist, "The Notorious B.I.G.");
        assert_eq!(juicy.album, "Greatest Hits");
        assert_eq!(juicy.genres, Genres::from([Genre::HipHop]));
        assert_eq!(juicy.preview_failed_on, None);
        for genre in Genre::ALL {
            let tagged = pool
                .iter()
                .filter(|song| song.genres.contains(&genre))
                .count();
            assert_eq!(tagged, 2, "{genre}");
        }
    }

    #[tokio::test]
    async fn a_pool_with_any_song_in_it_is_left_alone() {
        let store = MemoryStore::new();
        seed_if_empty(&store).await.unwrap();
        // The owner removes one and retags another.
        assert!(store.remove_song(15_391_618).await.unwrap());
        store
            .set_song_genres(4_603_408, Genres::new())
            .await
            .unwrap();
        let before = store.songs().await.unwrap();

        assert_eq!(seed_if_empty(&store).await.unwrap(), 0);
        assert_eq!(store.songs().await.unwrap(), before);
        assert_eq!(before.len(), 5);
    }

    #[tokio::test]
    async fn a_removal_sticks_across_a_restart_of_the_sqlite_backend() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("needledrop.db");

        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(seed_if_empty(&store).await.unwrap(), 6);
        assert!(store.remove_song(4_603_408).await.unwrap());
        drop(store);

        // The next start finds five songs and adds none.
        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(seed_if_empty(&store).await.unwrap(), 0);
        let pool = store.songs().await.unwrap();
        assert_eq!(pool.len(), 5);
        assert!(pool.iter().all(|song| song.track_id != 4_603_408));
    }
}
