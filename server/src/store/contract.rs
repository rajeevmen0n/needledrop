//! The contract of [`Store`], as tests: what every backend must do, whatever it is built on.
//!
//! Each case is a function over `&dyn Store` that starts from an empty store.
//! [`contract_tests!`] turns the whole list into `#[tokio::test]`s for one
//! backend; the in-memory and the SQLite backends both call it, and a new
//! backend is not done until it does too and passes.
//!
//! A new case is a function here plus its name in the macro's list. A case
//! that is not in the list is dead code, which fails `just check`.

use jiff::civil::date;

use super::{Genre, Genres, NewSong, PoolSong, Store};

/// A backend under test, with whatever has to outlive it.
pub struct Fixture {
    store: Box<dyn Store>,
    /// The directory a file-backed store lives in; removed when dropped.
    _dir: Option<tempfile::TempDir>,
}

impl Fixture {
    /// A backend that needs nothing kept around.
    pub fn new(store: impl Store + 'static) -> Self {
        Self {
            store: Box::new(store),
            _dir: None,
        }
    }

    /// A backend whose files are in `dir`.
    pub fn in_dir(store: impl Store + 'static, dir: tempfile::TempDir) -> Self {
        Self {
            store: Box::new(store),
            _dir: Some(dir),
        }
    }

    pub fn store(&self) -> &dyn Store {
        self.store.as_ref()
    }
}

/// Runs every contract case against a fresh `$fixture` (an expression that
/// builds a [`Fixture`] around an empty store), one test per case.
macro_rules! contract_tests {
    ($fixture:expr) => {
        $crate::store::contract::contract_tests!(
            @cases $fixture;
            a_new_store_has_an_empty_pool,
            an_added_song_is_stored_and_returned,
            songs_are_listed_by_ascending_track_id,
            a_song_may_have_no_genre_one_or_several,
            text_is_stored_exactly_as_given,
            adding_a_song_again_replaces_only_its_genres,
            adding_the_same_song_twice_at_once_stores_it_once,
            changing_genres_replaces_the_whole_set,
            changing_genres_touches_no_other_song,
            changing_the_genres_of_an_unknown_song_adds_nothing,
            a_removed_song_is_gone_with_its_genres,
            removing_an_unknown_song_changes_nothing,
            the_preview_failed_day_is_recorded_replaced_and_cleared,
            the_preview_failed_day_survives_a_change_of_genres,
            the_preview_failed_day_of_an_unknown_song_is_not_recorded,
        );
    };
    (@cases $fixture:expr; $($case:ident),+ $(,)?) => {
        $(
            #[tokio::test]
            async fn $case() {
                let fixture: $crate::store::contract::Fixture = $fixture;
                $crate::store::contract::$case(fixture.store()).await;
            }
        )+
    };
}
pub(crate) use contract_tests;

// --- helpers --------------------------------------------------------------------

fn new_song(track_id: u64, title: &str, artist: &str) -> NewSong {
    NewSong {
        track_id,
        title: title.to_owned(),
        title_short: title.to_owned(),
        artist: artist.to_owned(),
        album: format!("{title} (album)"),
    }
}

fn genres<const N: usize>(genres: [Genre; N]) -> Genres {
    Genres::from(genres)
}

/// Adds a song and returns it as stored.
async fn add<const N: usize>(store: &dyn Store, track_id: u64, tags: [Genre; N]) -> PoolSong {
    store
        .add_song(
            new_song(track_id, &format!("Song {track_id}"), "Someone"),
            genres(tags),
        )
        .await
        .unwrap()
}

async fn ids(store: &dyn Store) -> Vec<u64> {
    store
        .songs()
        .await
        .unwrap()
        .iter()
        .map(|song| song.track_id)
        .collect()
}

// --- adding and listing ---------------------------------------------------------

pub async fn a_new_store_has_an_empty_pool(store: &dyn Store) {
    assert_eq!(store.songs().await.unwrap(), Vec::new());
    assert_eq!(store.song(1).await.unwrap(), None);
}

pub async fn an_added_song_is_stored_and_returned(store: &dyn Store) {
    let song = NewSong {
        track_id: 15_391_618,
        title: "Toxic (Radio Edit)".to_owned(),
        title_short: "Toxic".to_owned(),
        artist: "Britney Spears".to_owned(),
        album: "In The Zone".to_owned(),
    };
    let expected = PoolSong {
        track_id: 15_391_618,
        title: "Toxic (Radio Edit)".to_owned(),
        title_short: "Toxic".to_owned(),
        artist: "Britney Spears".to_owned(),
        album: "In The Zone".to_owned(),
        genres: genres([Genre::Pop]),
        preview_failed_on: None,
    };

    let stored = store.add_song(song, genres([Genre::Pop])).await.unwrap();
    assert_eq!(stored, expected);
    assert_eq!(
        store.song(15_391_618).await.unwrap(),
        Some(expected.clone())
    );
    assert_eq!(store.songs().await.unwrap(), vec![expected]);
    // A neighbouring ID is another song.
    assert_eq!(store.song(15_391_619).await.unwrap(), None);
}

pub async fn songs_are_listed_by_ascending_track_id(store: &dyn Store) {
    // Added out of order, with an ID that does not fit in 32 bits (Deezer has
    // them: "Bohemian Rhapsody" in the seed is 4091937401) and one that does
    // not fit in 53, the largest whole number a float holds exactly.
    for track_id in [
        92_720_046,
        4_091_937_401,
        3,
        9_007_199_254_740_993,
        1_109_731,
    ] {
        add(store, track_id, []).await;
    }
    let expected = vec![
        3,
        1_109_731,
        92_720_046,
        4_091_937_401,
        9_007_199_254_740_993,
    ];
    assert_eq!(ids(store).await, expected);
    // The same answer every time.
    assert_eq!(ids(store).await, expected);

    // A change to a song does not move it.
    store
        .set_song_genres(92_720_046, genres([Genre::Rock]))
        .await
        .unwrap();
    store
        .set_preview_failed_on(3, Some(date(2026, 10, 2)))
        .await
        .unwrap();
    assert_eq!(ids(store).await, expected);
}

pub async fn a_song_may_have_no_genre_one_or_several(store: &dyn Store) {
    let none = add(store, 1, []).await;
    let one = add(store, 2, [Genre::HipHop]).await;
    let all = add(store, 3, [Genre::HipHop, Genre::Pop, Genre::Rock]).await;

    assert_eq!(none.genres, Genres::new());
    assert_eq!(one.genres, genres([Genre::HipHop]));
    assert_eq!(all.genres, genres(Genre::ALL));

    let listed = store.songs().await.unwrap();
    assert_eq!(listed, vec![none, one, all]);
    // Whatever order they were given in, they come back in `Genre::ALL`'s.
    assert_eq!(
        listed[2].genres.iter().copied().collect::<Vec<_>>(),
        Genre::ALL
    );
}

pub async fn text_is_stored_exactly_as_given(store: &dyn Store) {
    let song = NewSong {
        track_id: 42,
        title: "  Señorita \"Live\" — 東京 🎶 ".to_owned(),
        // Deezer leaves the short title empty on some tracks.
        title_short: String::new(),
        artist: "Robert'); DROP TABLE songs;--".to_owned(),
        album: "100% \\ _wild_ ?1 :name\nsecond line".to_owned(),
    };
    let stored = store.add_song(song.clone(), Genres::new()).await.unwrap();
    assert_eq!(stored, PoolSong::new(song.clone(), Genres::new()));
    assert_eq!(
        store.song(42).await.unwrap(),
        Some(PoolSong::new(song, Genres::new()))
    );
}

pub async fn adding_a_song_again_replaces_only_its_genres(store: &dyn Store) {
    let first = store
        .add_song(
            new_song(7, "First Title", "First Artist"),
            genres([Genre::Pop]),
        )
        .await
        .unwrap();
    store
        .set_preview_failed_on(7, Some(date(2026, 10, 3)))
        .await
        .unwrap();

    // The same track, described differently and tagged differently.
    let again = store
        .add_song(
            new_song(7, "Second Title", "Second Artist"),
            genres([Genre::Rock, Genre::HipHop]),
        )
        .await
        .unwrap();

    let expected = PoolSong {
        genres: genres([Genre::Rock, Genre::HipHop]),
        preview_failed_on: Some(date(2026, 10, 3)),
        ..first
    };
    assert_eq!(again, expected);
    assert_eq!(again.title, "First Title");
    assert_eq!(store.songs().await.unwrap(), vec![expected]);

    // Adding it without tags takes the tags away; the song stays.
    let untagged = store
        .add_song(new_song(7, "Third Title", "Third Artist"), Genres::new())
        .await
        .unwrap();
    assert_eq!(untagged.genres, Genres::new());
    assert_eq!(untagged.title, "First Title");
    assert_eq!(ids(store).await, vec![7]);
}

pub async fn adding_the_same_song_twice_at_once_stores_it_once(store: &dyn Store) {
    let (one, other) = tokio::join!(
        store.add_song(new_song(7, "One", "Someone"), genres([Genre::Pop])),
        store.add_song(new_song(7, "Other", "Someone Else"), genres([Genre::Rock])),
    );
    let (one, other) = (one.unwrap(), other.unwrap());

    let pool = store.songs().await.unwrap();
    assert_eq!(pool.len(), 1);
    let stored = &pool[0];
    // Whichever came first supplied the text, and both callers were told so.
    assert!(
        (stored.title == "One" && stored.artist == "Someone")
            || (stored.title == "Other" && stored.artist == "Someone Else"),
        "{stored:?}"
    );
    assert_eq!(one.title, stored.title);
    assert_eq!(other.title, stored.title);
    // Whichever came last supplied the genres: one whole set, not a mixture.
    assert!(
        stored.genres == genres([Genre::Pop]) || stored.genres == genres([Genre::Rock]),
        "{stored:?}"
    );
}

// --- changing genres ------------------------------------------------------------

pub async fn changing_genres_replaces_the_whole_set(store: &dyn Store) {
    let added = add(store, 7, [Genre::Pop, Genre::Rock]).await;

    let changed = store
        .set_song_genres(7, genres([Genre::HipHop]))
        .await
        .unwrap();
    let expected = PoolSong {
        genres: genres([Genre::HipHop]),
        ..added.clone()
    };
    assert_eq!(changed, Some(expected.clone()));
    assert_eq!(store.song(7).await.unwrap(), Some(expected));

    // Setting what it already has is not an error.
    let same = store
        .set_song_genres(7, genres([Genre::HipHop]))
        .await
        .unwrap();
    assert_eq!(same, changed);

    // No genre at all leaves the song in the General pool only.
    let cleared = store.set_song_genres(7, Genres::new()).await.unwrap();
    let expected = PoolSong {
        genres: Genres::new(),
        ..added
    };
    assert_eq!(cleared, Some(expected.clone()));
    assert_eq!(store.songs().await.unwrap(), vec![expected]);
}

pub async fn changing_genres_touches_no_other_song(store: &dyn Store) {
    let before = add(store, 1, [Genre::Pop]).await;
    add(store, 2, [Genre::Pop]).await;
    let after = add(store, 3, [Genre::Rock, Genre::HipHop]).await;

    store
        .set_song_genres(2, genres([Genre::Rock]))
        .await
        .unwrap();

    assert_eq!(store.song(1).await.unwrap(), Some(before));
    assert_eq!(store.song(3).await.unwrap(), Some(after));
    assert_eq!(
        store.song(2).await.unwrap().unwrap().genres,
        genres([Genre::Rock])
    );
}

pub async fn changing_the_genres_of_an_unknown_song_adds_nothing(store: &dyn Store) {
    let only = add(store, 1, [Genre::Pop]).await;

    let changed = store
        .set_song_genres(2, genres([Genre::Rock]))
        .await
        .unwrap();
    assert_eq!(changed, None);
    assert_eq!(store.song(2).await.unwrap(), None);
    assert_eq!(store.songs().await.unwrap(), vec![only]);

    // The tags were not kept for later: the track, once added, has its own.
    let added = add(store, 2, []).await;
    assert_eq!(added.genres, Genres::new());
}

// --- removing -------------------------------------------------------------------

pub async fn a_removed_song_is_gone_with_its_genres(store: &dyn Store) {
    let kept = add(store, 1, [Genre::Pop, Genre::Rock]).await;
    add(store, 2, [Genre::Pop, Genre::HipHop]).await;
    store
        .set_preview_failed_on(2, Some(date(2026, 10, 3)))
        .await
        .unwrap();

    assert!(store.remove_song(2).await.unwrap());
    assert_eq!(store.song(2).await.unwrap(), None);
    assert_eq!(store.songs().await.unwrap(), vec![kept.clone()]);

    // Added again, it is a new song: no tags and no failed check carried over.
    let again = store
        .add_song(new_song(2, "Back Again", "Someone"), Genres::new())
        .await
        .unwrap();
    assert_eq!(again.title, "Back Again");
    assert_eq!(again.genres, Genres::new());
    assert_eq!(again.preview_failed_on, None);
    assert_eq!(store.songs().await.unwrap(), vec![kept, again]);
}

pub async fn removing_an_unknown_song_changes_nothing(store: &dyn Store) {
    assert!(!store.remove_song(1).await.unwrap());

    let only = add(store, 1, [Genre::Rock]).await;
    assert!(!store.remove_song(2).await.unwrap());
    assert_eq!(store.songs().await.unwrap(), vec![only]);

    // The second removal of the same song finds nothing.
    assert!(store.remove_song(1).await.unwrap());
    assert!(!store.remove_song(1).await.unwrap());
    assert_eq!(store.songs().await.unwrap(), Vec::new());
}

// --- the failed preview check ---------------------------------------------------

pub async fn the_preview_failed_day_is_recorded_replaced_and_cleared(store: &dyn Store) {
    let added = add(store, 1, [Genre::Pop]).await;
    let other = add(store, 2, [Genre::Pop]).await;
    assert_eq!(added.preview_failed_on, None);

    assert!(
        store
            .set_preview_failed_on(1, Some(date(2026, 10, 3)))
            .await
            .unwrap()
    );
    let expected = PoolSong {
        preview_failed_on: Some(date(2026, 10, 3)),
        ..added.clone()
    };
    assert_eq!(store.song(1).await.unwrap(), Some(expected.clone()));
    assert_eq!(store.songs().await.unwrap(), vec![expected, other.clone()]);

    // A later failure replaces the day; the last one is what is kept.
    assert!(
        store
            .set_preview_failed_on(1, Some(date(2027, 2, 28)))
            .await
            .unwrap()
    );
    assert_eq!(
        store.song(1).await.unwrap().unwrap().preview_failed_on,
        Some(date(2027, 2, 28))
    );

    // Cleared, the song is as it was added. Clearing twice is fine.
    assert!(store.set_preview_failed_on(1, None).await.unwrap());
    assert!(store.set_preview_failed_on(1, None).await.unwrap());
    assert_eq!(store.songs().await.unwrap(), vec![added, other]);
}

pub async fn the_preview_failed_day_survives_a_change_of_genres(store: &dyn Store) {
    add(store, 1, [Genre::Pop]).await;
    store
        .set_preview_failed_on(1, Some(date(2026, 12, 31)))
        .await
        .unwrap();

    let changed = store
        .set_song_genres(1, genres([Genre::Rock, Genre::HipHop]))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.preview_failed_on, Some(date(2026, 12, 31)));
    assert_eq!(changed.genres, genres([Genre::Rock, Genre::HipHop]));
    assert_eq!(store.song(1).await.unwrap(), Some(changed));
}

pub async fn the_preview_failed_day_of_an_unknown_song_is_not_recorded(store: &dyn Store) {
    assert!(
        !store
            .set_preview_failed_on(1, Some(date(2026, 10, 3)))
            .await
            .unwrap()
    );
    assert!(!store.set_preview_failed_on(1, None).await.unwrap());
    assert_eq!(store.songs().await.unwrap(), Vec::new());

    // Nothing was kept for later either.
    let added = add(store, 1, []).await;
    assert_eq!(added.preview_failed_on, None);
}
