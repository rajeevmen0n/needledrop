//! The contract of [`Store`], as tests: what every backend must do, whatever it is built on.
//!
//! Each case is a function over `&dyn Store` that starts from an empty store.
//! [`contract_tests!`] turns the whole list into `#[tokio::test]`s for one
//! backend; the in-memory and the SQLite backends both call it, and a new
//! backend is not done until it does too and passes.
//!
//! A new case is a function here plus its name in the macro's list. A case
//! that is not in the list is dead code, which fails `just check`.

use jiff::civil::{Date, date};

use super::{Genre, Genres, NewSong, Pick, PlayerId, PoolSong, Section, Store};
use crate::{
    game::{GameState, Status, TrackMeta},
    random::RandomGame,
    testutil::{lost_game, playing_game, won_game},
};

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
            a_new_store_has_no_games,
            a_saved_game_is_loaded_as_it_was_saved,
            the_text_of_an_attempt_is_stored_exactly_as_given,
            saving_a_game_again_replaces_it,
            games_are_kept_apart_by_player_section_and_day,
            a_players_games_are_listed_by_ascending_day_for_one_section,
            saving_the_same_game_twice_at_once_keeps_one_of_them_whole,
            deleting_a_player_removes_all_their_games_in_every_section,
            deleting_a_player_leaves_everyone_else_alone,
            games_and_the_song_pool_do_not_touch_each_other,
            deleting_a_days_games_in_a_section_leaves_every_other_game,
            a_new_store_has_no_picks,
            a_saved_pick_stands_and_is_found_by_day_and_by_section,
            a_pick_is_not_replaced_by_a_later_one_for_the_same_day_and_section,
            picks_saved_at_once_for_one_day_and_section_agree_on_one,
            a_days_picks_are_listed_general_first_then_the_genres,
            a_sections_pick_history_is_listed_by_ascending_day,
            a_removed_pick_makes_room_for_another,
            picks_games_and_the_song_pool_do_not_touch_each_other,
            the_day_offset_is_zero_until_it_is_set_and_then_what_was_set,
            wiping_deletes_every_game_and_every_pick,
            wiping_leaves_the_song_pool_and_the_day_offset_alone,
            a_new_store_has_no_random_game,
            a_saved_random_game_is_loaded_as_it_was_saved_and_replaced_by_the_next,
            random_games_are_kept_apart_by_player,
            saving_the_same_random_game_twice_at_once_keeps_one_of_them_whole,
            deleting_a_player_removes_their_random_game_and_counts_only_the_daily_games,
            random_games_daily_games_and_the_song_pool_do_not_touch_each_other,
            wiping_deletes_every_random_game,
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

// --- games ----------------------------------------------------------------------

const DAY: Date = date(2026, 10, 1);
const GENERAL: Section = Section::General;
const ROCK: Section = Section::Genre(Genre::Rock);

/// A game with two wrong guesses in it, still being played.
fn game_with_guesses(day: Date) -> GameState {
    let answer = TrackMeta::new("The Song", "", "Someone");
    let mut game = GameState::new(day);
    game.skip().unwrap();
    for (title, artist) in [("Under Pressure", "Queen"), ("Toxic", "Britney Spears")] {
        game.guess(&answer, &TrackMeta::new(title, "", artist))
            .unwrap();
    }
    game
}

pub async fn a_new_store_has_no_games(store: &dyn Store) {
    let player = PlayerId::generate();
    for section in Section::ALL {
        assert_eq!(store.game(&player, section, DAY).await.unwrap(), None);
        assert_eq!(store.games(&player, section).await.unwrap(), Vec::new());
    }
    // Asking did not create anything, and there is nothing to delete.
    assert_eq!(store.delete_player(&player).await.unwrap(), 0);
}

pub async fn a_saved_game_is_loaded_as_it_was_saved(store: &dyn Store) {
    let player = PlayerId::generate();
    // Every state a game can be in, each on a day of its own.
    let games = [
        GameState::new(date(2026, 10, 1)),
        game_with_guesses(date(2026, 10, 2)),
        playing_game(date(2026, 10, 3), 6),
        won_game(date(2026, 10, 4), 0),
        won_game(date(2026, 10, 5), 6),
        lost_game(date(2026, 10, 6)),
    ];
    for game in &games {
        store.save_game(&player, GENERAL, game).await.unwrap();
    }

    for game in &games {
        let loaded = store
            .game(&player, GENERAL, game.day())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&loaded, game);
        // What makes the next move right is all there: the day, the status
        // and how much of the clip is unlocked.
        assert_eq!(loaded.day(), game.day());
        assert_eq!(loaded.status(), game.status());
        assert_eq!(loaded.unlocked_ms(), game.unlocked_ms());
    }
    assert_eq!(store.games(&player, GENERAL).await.unwrap(), games);
    // A day without a game is not one of its neighbours'.
    assert_eq!(
        store
            .game(&player, GENERAL, date(2026, 10, 7))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .game(&player, GENERAL, date(2026, 9, 30))
            .await
            .unwrap(),
        None
    );
}

pub async fn the_text_of_an_attempt_is_stored_exactly_as_given(store: &dyn Store) {
    let player = PlayerId::generate();
    let answer = TrackMeta::new("The Song", "", "Someone");
    let mut game = GameState::new(DAY);
    for (title, artist) in [
        ("Señorita \"Live\" — 東京 🎶", "Beyoncé; x=y"),
        ("Robert'); DROP TABLE games;--", "100% \\ _wild_ ?1 :name"),
        ("", "{\"status\":\"won\"}"),
    ] {
        game.guess(&answer, &TrackMeta::new(title, "", artist))
            .unwrap();
    }

    store.save_game(&player, GENERAL, &game).await.unwrap();
    assert_eq!(
        store.game(&player, GENERAL, DAY).await.unwrap(),
        Some(game.clone())
    );
    assert_eq!(store.games(&player, GENERAL).await.unwrap(), vec![game]);
}

pub async fn saving_a_game_again_replaces_it(store: &dyn Store) {
    let player = PlayerId::generate();
    let mut game = GameState::new(DAY);

    // A game as it is played: saved after every move, one stored game.
    for misses in 1..=6 {
        game.skip().unwrap();
        store.save_game(&player, GENERAL, &game).await.unwrap();
        let stored = store.game(&player, GENERAL, DAY).await.unwrap().unwrap();
        assert_eq!(stored.attempts().len(), misses);
        assert_eq!(store.games(&player, GENERAL).await.unwrap().len(), 1);
    }
    game.skip().unwrap();
    store.save_game(&player, GENERAL, &game).await.unwrap();
    assert_eq!(
        store.game(&player, GENERAL, DAY).await.unwrap(),
        Some(lost_game(DAY))
    );

    // The replacement is whole and unconditional: whatever is saved last is
    // the game, even one with fewer attempts than the one before.
    let shorter = playing_game(DAY, 2);
    store.save_game(&player, GENERAL, &shorter).await.unwrap();
    assert_eq!(
        store.games(&player, GENERAL).await.unwrap(),
        vec![shorter.clone()]
    );
    // Saving what is already there changes nothing.
    store.save_game(&player, GENERAL, &shorter).await.unwrap();
    assert_eq!(store.games(&player, GENERAL).await.unwrap(), vec![shorter]);
}

pub async fn games_are_kept_apart_by_player_section_and_day(store: &dyn Store) {
    let one = PlayerId::generate();
    let other = PlayerId::generate();
    let next_day = date(2026, 10, 2);

    // Four games that differ in one part of the key each, all told apart by
    // how many misses they have.
    store
        .save_game(&one, GENERAL, &playing_game(DAY, 1))
        .await
        .unwrap();
    store
        .save_game(&one, GENERAL, &playing_game(next_day, 2))
        .await
        .unwrap();
    store
        .save_game(&one, ROCK, &playing_game(DAY, 3))
        .await
        .unwrap();
    store
        .save_game(&other, GENERAL, &playing_game(DAY, 4))
        .await
        .unwrap();

    let misses = async |player: &PlayerId, section: Section, day: Date| {
        store
            .game(player, section, day)
            .await
            .unwrap()
            .map(|game| game.attempts().len())
    };
    assert_eq!(misses(&one, GENERAL, DAY).await, Some(1));
    assert_eq!(misses(&one, GENERAL, next_day).await, Some(2));
    assert_eq!(misses(&one, ROCK, DAY).await, Some(3));
    assert_eq!(misses(&other, GENERAL, DAY).await, Some(4));
    // The combinations nobody saved are not there.
    assert_eq!(misses(&one, ROCK, next_day).await, None);
    assert_eq!(misses(&other, ROCK, DAY).await, None);
    assert_eq!(misses(&other, GENERAL, next_day).await, None);
    assert_eq!(misses(&one, Section::Genre(Genre::Pop), DAY).await, None);

    // Replacing one of them leaves the other three as they were.
    store
        .save_game(&one, GENERAL, &won_game(DAY, 5))
        .await
        .unwrap();
    let replaced = store.game(&one, GENERAL, DAY).await.unwrap().unwrap();
    assert_eq!(replaced.status(), Status::Won);
    assert_eq!(misses(&one, GENERAL, next_day).await, Some(2));
    assert_eq!(misses(&one, ROCK, DAY).await, Some(3));
    assert_eq!(misses(&other, GENERAL, DAY).await, Some(4));
}

pub async fn a_players_games_are_listed_by_ascending_day_for_one_section(store: &dyn Store) {
    let player = PlayerId::generate();
    let other = PlayerId::generate();
    // Saved out of order, across a month and a year boundary, finished and
    // unfinished alike.
    let saved = [
        won_game(date(2026, 10, 10), 0),
        lost_game(date(2026, 10, 2)),
        playing_game(date(2027, 1, 1), 2),
        won_game(date(2026, 9, 30), 3),
        lost_game(date(2026, 12, 31)),
        playing_game(date(2026, 10, 9), 5),
    ];
    for game in &saved {
        store.save_game(&player, GENERAL, game).await.unwrap();
    }
    // Games that are not part of the list: another section's and another
    // player's, on days in between.
    store
        .save_game(&player, ROCK, &won_game(date(2026, 10, 5), 0))
        .await
        .unwrap();
    store
        .save_game(&other, GENERAL, &won_game(date(2026, 10, 6), 0))
        .await
        .unwrap();

    let listed = store.games(&player, GENERAL).await.unwrap();
    // Whole games, oldest first.
    let mut expected = saved.to_vec();
    expected.sort_by_key(GameState::day);
    assert_eq!(listed, expected);
    let days: Vec<Date> = listed.iter().map(GameState::day).collect();
    assert_eq!(
        days,
        vec![
            date(2026, 9, 30),
            date(2026, 10, 2),
            date(2026, 10, 9),
            date(2026, 10, 10),
            date(2026, 12, 31),
            date(2027, 1, 1),
        ]
    );
    // The same answer every time.
    assert_eq!(store.games(&player, GENERAL).await.unwrap(), listed);

    assert_eq!(
        store.games(&player, ROCK).await.unwrap(),
        vec![won_game(date(2026, 10, 5), 0)]
    );
    assert_eq!(
        store.games(&other, GENERAL).await.unwrap(),
        vec![won_game(date(2026, 10, 6), 0)]
    );
    assert_eq!(store.games(&other, ROCK).await.unwrap(), Vec::new());
}

pub async fn saving_the_same_game_twice_at_once_keeps_one_of_them_whole(store: &dyn Store) {
    let player = PlayerId::generate();
    let one = game_with_guesses(DAY);
    let other = lost_game(DAY);

    let (first, second) = tokio::join!(
        store.save_game(&player, GENERAL, &one),
        store.save_game(&player, GENERAL, &other),
    );
    first.unwrap();
    second.unwrap();

    // One game for the day, and it is one of the two, not a mixture.
    let stored = store.games(&player, GENERAL).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert!(stored[0] == one || stored[0] == other, "{stored:?}");
    assert_eq!(
        store.game(&player, GENERAL, DAY).await.unwrap().as_ref(),
        Some(&stored[0])
    );
}

pub async fn deleting_a_player_removes_all_their_games_in_every_section(store: &dyn Store) {
    let player = PlayerId::generate();
    let mut saved = 0;
    for section in Section::ALL {
        for day in [date(2026, 10, 1), date(2026, 10, 2), date(2026, 10, 3)] {
            store
                .save_game(&player, section, &won_game(day, 1))
                .await
                .unwrap();
            saved += 1;
        }
    }
    // Saving one again does not make it two.
    store
        .save_game(&player, ROCK, &lost_game(date(2026, 10, 2)))
        .await
        .unwrap();

    assert_eq!(store.delete_player(&player).await.unwrap(), saved);

    for section in Section::ALL {
        assert_eq!(store.games(&player, section).await.unwrap(), Vec::new());
        assert_eq!(
            store
                .game(&player, section, date(2026, 10, 2))
                .await
                .unwrap(),
            None
        );
    }
    // The second time there is nothing left to delete.
    assert_eq!(store.delete_player(&player).await.unwrap(), 0);

    // The ID is not used up: a game saved under it afterwards is a new start.
    let fresh = playing_game(date(2026, 10, 3), 1);
    store.save_game(&player, GENERAL, &fresh).await.unwrap();
    assert_eq!(store.games(&player, GENERAL).await.unwrap(), vec![fresh]);
}

pub async fn deleting_a_player_leaves_everyone_else_alone(store: &dyn Store) {
    let leaving = PlayerId::generate();
    let staying = PlayerId::generate();
    let kept_general = [won_game(date(2026, 10, 1), 2), lost_game(date(2026, 10, 2))];
    let kept_rock = [playing_game(date(2026, 10, 2), 4)];
    for game in &kept_general {
        store.save_game(&staying, GENERAL, game).await.unwrap();
        // The same days, so only the player tells the rows apart.
        store.save_game(&leaving, GENERAL, game).await.unwrap();
    }
    for game in &kept_rock {
        store.save_game(&staying, ROCK, game).await.unwrap();
        store.save_game(&leaving, ROCK, game).await.unwrap();
    }

    assert_eq!(store.delete_player(&leaving).await.unwrap(), 3);

    assert_eq!(store.games(&staying, GENERAL).await.unwrap(), kept_general);
    assert_eq!(store.games(&staying, ROCK).await.unwrap(), kept_rock);
    assert_eq!(store.games(&leaving, GENERAL).await.unwrap(), Vec::new());
    assert_eq!(store.games(&leaving, ROCK).await.unwrap(), Vec::new());
}

pub async fn games_and_the_song_pool_do_not_touch_each_other(store: &dyn Store) {
    let player = PlayerId::generate();
    let song = add(store, 7, [Genre::Rock]).await;
    let game = won_game(DAY, 3);
    store.save_game(&player, ROCK, &game).await.unwrap();

    // A song leaving the pool takes no game with it: a game records what the
    // player did, not which track it was about.
    assert!(store.remove_song(7).await.unwrap());
    assert_eq!(store.games(&player, ROCK).await.unwrap(), vec![game]);

    // And clearing a player's data takes no song with it.
    let song = store
        .add_song(
            new_song(7, &song.title, &song.artist),
            genres([Genre::Rock]),
        )
        .await
        .unwrap();
    assert_eq!(store.delete_player(&player).await.unwrap(), 1);
    assert_eq!(store.songs().await.unwrap(), vec![song]);
}

// --- deleting a day's games in a section ----------------------------------------

pub async fn deleting_a_days_games_in_a_section_leaves_every_other_game(store: &dyn Store) {
    let one = PlayerId::generate();
    let other = PlayerId::generate();
    let next_day = date(2026, 10, 2);
    // The games that go: both players', finished or not.
    store
        .save_game(&one, ROCK, &playing_game(DAY, 3))
        .await
        .unwrap();
    store
        .save_game(&other, ROCK, &won_game(DAY, 0))
        .await
        .unwrap();
    // The games that stay: another section on that day, that section on
    // another day.
    let general = lost_game(DAY);
    let later = won_game(next_day, 2);
    store.save_game(&one, GENERAL, &general).await.unwrap();
    store.save_game(&one, ROCK, &later).await.unwrap();
    store.save_game(&other, ROCK, &later).await.unwrap();

    assert_eq!(store.delete_games(ROCK, DAY).await.unwrap(), 2);

    assert_eq!(store.game(&one, ROCK, DAY).await.unwrap(), None);
    assert_eq!(store.game(&other, ROCK, DAY).await.unwrap(), None);
    assert_eq!(store.games(&one, ROCK).await.unwrap(), vec![later.clone()]);
    assert_eq!(store.games(&other, ROCK).await.unwrap(), vec![later]);
    assert_eq!(store.games(&one, GENERAL).await.unwrap(), vec![general]);

    // Nothing is left to delete there, and a day nobody played has nothing.
    assert_eq!(store.delete_games(ROCK, DAY).await.unwrap(), 0);
    assert_eq!(
        store
            .delete_games(Section::Genre(Genre::Pop), DAY)
            .await
            .unwrap(),
        0
    );
    // A game saved there afterwards is a new start.
    let fresh = playing_game(DAY, 1);
    store.save_game(&one, ROCK, &fresh).await.unwrap();
    assert_eq!(store.game(&one, ROCK, DAY).await.unwrap(), Some(fresh));
}

// --- picks ----------------------------------------------------------------------

const POP: Section = Section::Genre(Genre::Pop);

fn pick(day: Date, section: Section, track_id: u64) -> Pick {
    Pick {
        day,
        section,
        track_id,
    }
}

pub async fn a_new_store_has_no_picks(store: &dyn Store) {
    assert_eq!(store.picks_on(DAY).await.unwrap(), Vec::new());
    for section in Section::ALL {
        assert_eq!(store.pick_history(section).await.unwrap(), Vec::new());
        assert!(!store.remove_pick(DAY, section).await.unwrap());
    }
    // Asking and removing created nothing.
    assert_eq!(store.picks_on(DAY).await.unwrap(), Vec::new());
}

pub async fn a_saved_pick_stands_and_is_found_by_day_and_by_section(store: &dyn Store) {
    // A track ID that does not fit in 32 bits, as Deezer has them.
    let saved = pick(DAY, ROCK, 4_091_937_401);
    assert_eq!(store.save_pick(saved).await.unwrap(), saved);

    assert_eq!(store.picks_on(DAY).await.unwrap(), vec![saved]);
    assert_eq!(store.pick_history(ROCK).await.unwrap(), vec![saved]);
    // Neither another day nor another section has it.
    assert_eq!(store.picks_on(date(2026, 10, 2)).await.unwrap(), Vec::new());
    assert_eq!(store.picks_on(date(2026, 9, 30)).await.unwrap(), Vec::new());
    assert_eq!(store.pick_history(GENERAL).await.unwrap(), Vec::new());
    assert_eq!(store.pick_history(POP).await.unwrap(), Vec::new());
}

pub async fn a_pick_is_not_replaced_by_a_later_one_for_the_same_day_and_section(store: &dyn Store) {
    let first = pick(DAY, POP, 100);
    assert_eq!(store.save_pick(first).await.unwrap(), first);

    // The second caller is told which pick stands, and it is not its own.
    assert_eq!(store.save_pick(pick(DAY, POP, 200)).await.unwrap(), first);
    assert_eq!(store.save_pick(pick(DAY, POP, 300)).await.unwrap(), first);
    // Saving the pick that stands is not an error either.
    assert_eq!(store.save_pick(first).await.unwrap(), first);
    assert_eq!(store.picks_on(DAY).await.unwrap(), vec![first]);
    assert_eq!(store.pick_history(POP).await.unwrap(), vec![first]);

    // The rule is per day and per section: the same section on another day
    // and another section on the same day take their own.
    let next_day = pick(date(2026, 10, 2), POP, 200);
    let rock = pick(DAY, ROCK, 200);
    assert_eq!(store.save_pick(next_day).await.unwrap(), next_day);
    assert_eq!(store.save_pick(rock).await.unwrap(), rock);
    assert_eq!(store.picks_on(DAY).await.unwrap(), vec![first, rock]);
}

pub async fn picks_saved_at_once_for_one_day_and_section_agree_on_one(store: &dyn Store) {
    // Two requests at midnight, each with the song it drew.
    let (one, other, third) = tokio::join!(
        store.save_pick(pick(DAY, GENERAL, 100)),
        store.save_pick(pick(DAY, GENERAL, 200)),
        store.save_pick(pick(DAY, GENERAL, 300)),
    );
    let stands = one.unwrap();
    assert_eq!(other.unwrap(), stands);
    assert_eq!(third.unwrap(), stands);
    assert!([100, 200, 300].contains(&stands.track_id), "{stands:?}");
    assert_eq!(store.picks_on(DAY).await.unwrap(), vec![stands]);
}

pub async fn a_days_picks_are_listed_general_first_then_the_genres(store: &dyn Store) {
    let next_day = date(2026, 10, 2);
    // Saved in the order they are made, which is not the order they are
    // listed in, with another day's in between.
    store.save_pick(pick(DAY, POP, 1)).await.unwrap();
    store.save_pick(pick(next_day, POP, 9)).await.unwrap();
    store.save_pick(pick(DAY, ROCK, 2)).await.unwrap();
    store
        .save_pick(pick(DAY, Section::Genre(Genre::HipHop), 3))
        .await
        .unwrap();
    store.save_pick(pick(DAY, GENERAL, 4)).await.unwrap();

    let listed = store.picks_on(DAY).await.unwrap();
    let sections: Vec<Section> = listed.iter().map(|pick| pick.section).collect();
    assert_eq!(sections, Section::ALL);
    let tracks: Vec<u64> = listed.iter().map(|pick| pick.track_id).collect();
    assert_eq!(tracks, vec![4, 1, 2, 3]);
    assert!(listed.iter().all(|pick| pick.day == DAY));
    // The same answer every time.
    assert_eq!(store.picks_on(DAY).await.unwrap(), listed);
    assert_eq!(
        store.picks_on(next_day).await.unwrap(),
        vec![pick(next_day, POP, 9)]
    );
}

pub async fn a_sections_pick_history_is_listed_by_ascending_day(store: &dyn Store) {
    // Saved out of order, across a month and a year boundary, with the same
    // song on two days.
    let saved = [
        pick(date(2026, 10, 10), POP, 5),
        pick(date(2026, 10, 2), POP, 7),
        pick(date(2027, 1, 1), POP, 5),
        pick(date(2026, 9, 30), POP, 1),
        pick(date(2026, 12, 31), POP, 3),
    ];
    for pick in saved {
        store.save_pick(pick).await.unwrap();
    }
    // Other sections' picks on days in between are not part of it.
    store
        .save_pick(pick(date(2026, 10, 5), ROCK, 7))
        .await
        .unwrap();
    store
        .save_pick(pick(date(2026, 10, 2), GENERAL, 8))
        .await
        .unwrap();

    let history = store.pick_history(POP).await.unwrap();
    let mut expected = saved.to_vec();
    expected.sort_by_key(|pick| pick.day);
    assert_eq!(history, expected);
    let days: Vec<Date> = history.iter().map(|pick| pick.day).collect();
    assert_eq!(
        days,
        vec![
            date(2026, 9, 30),
            date(2026, 10, 2),
            date(2026, 10, 10),
            date(2026, 12, 31),
            date(2027, 1, 1),
        ]
    );
    assert_eq!(store.pick_history(POP).await.unwrap(), history);
    assert_eq!(
        store.pick_history(ROCK).await.unwrap(),
        vec![pick(date(2026, 10, 5), ROCK, 7)]
    );
}

pub async fn a_removed_pick_makes_room_for_another(store: &dyn Store) {
    let next_day = date(2026, 10, 2);
    let old = pick(DAY, POP, 100);
    let rock = pick(DAY, ROCK, 300);
    let tomorrow = pick(next_day, POP, 100);
    for pick in [old, rock, tomorrow] {
        store.save_pick(pick).await.unwrap();
    }

    assert!(store.remove_pick(DAY, POP).await.unwrap());
    // Only that day's pick of that section went.
    assert_eq!(store.picks_on(DAY).await.unwrap(), vec![rock]);
    assert_eq!(store.pick_history(POP).await.unwrap(), vec![tomorrow]);
    // The second removal finds nothing.
    assert!(!store.remove_pick(DAY, POP).await.unwrap());

    // Now another pick stands there: a re-roll.
    let new = pick(DAY, POP, 200);
    assert_eq!(store.save_pick(new).await.unwrap(), new);
    assert_eq!(store.picks_on(DAY).await.unwrap(), vec![new, rock]);
    assert_eq!(store.pick_history(POP).await.unwrap(), vec![new, tomorrow]);
}

pub async fn picks_games_and_the_song_pool_do_not_touch_each_other(store: &dyn Store) {
    let player = PlayerId::generate();
    add(store, 7, [Genre::Rock]).await;
    let picked = pick(DAY, ROCK, 7);
    store.save_pick(picked).await.unwrap();
    // A pick of a track the pool has never had is a pick all the same.
    let stranger = pick(DAY, GENERAL, 999);
    assert_eq!(store.save_pick(stranger).await.unwrap(), stranger);
    let game = won_game(DAY, 2);
    store.save_game(&player, ROCK, &game).await.unwrap();

    // A song leaving the pool takes neither its pick nor the games with it:
    // the day was played, and the history is what prevents repeats.
    assert!(store.remove_song(7).await.unwrap());
    assert_eq!(store.picks_on(DAY).await.unwrap(), vec![stranger, picked]);
    assert_eq!(
        store.games(&player, ROCK).await.unwrap(),
        vec![game.clone()]
    );

    // Removing a pick deletes no game, and deleting the games no pick: the
    // caller does both, in the order it wants.
    assert!(store.remove_pick(DAY, ROCK).await.unwrap());
    assert_eq!(store.games(&player, ROCK).await.unwrap(), vec![game]);
    assert_eq!(store.delete_games(ROCK, DAY).await.unwrap(), 1);
    assert_eq!(store.picks_on(DAY).await.unwrap(), vec![stranger]);
    // And clearing a player's data touches no pick.
    store.delete_player(&player).await.unwrap();
    assert_eq!(store.picks_on(DAY).await.unwrap(), vec![stranger]);
}

// --- the day offset -------------------------------------------------------------

pub async fn the_day_offset_is_zero_until_it_is_set_and_then_what_was_set(store: &dyn Store) {
    assert_eq!(store.day_offset().await.unwrap(), 0);

    for days in [1, 2, 400, 0, -3, -36_500, 7] {
        store.set_day_offset(days).await.unwrap();
        assert_eq!(store.day_offset().await.unwrap(), days);
        // The same answer every time.
        assert_eq!(store.day_offset().await.unwrap(), days);
    }
    // Setting what is already there is fine.
    store.set_day_offset(7).await.unwrap();
    assert_eq!(store.day_offset().await.unwrap(), 7);

    // It is a setting of its own: no pick, game or song came of it.
    assert_eq!(store.picks_on(DAY).await.unwrap(), Vec::new());
    assert_eq!(store.songs().await.unwrap(), Vec::new());
}

// --- wiping ---------------------------------------------------------------------

pub async fn wiping_deletes_every_game_and_every_pick(store: &dyn Store) {
    let one = PlayerId::generate();
    let other = PlayerId::generate();
    let days = [date(2026, 10, 1), date(2026, 10, 2), date(2026, 11, 30)];
    for day in days {
        for section in Section::ALL {
            store.save_pick(pick(day, section, 100)).await.unwrap();
            store
                .save_game(&one, section, &won_game(day, 1))
                .await
                .unwrap();
        }
        store
            .save_game(&other, GENERAL, &playing_game(day, 2))
            .await
            .unwrap();
    }

    store.wipe_games_and_picks().await.unwrap();

    for day in days {
        assert_eq!(store.picks_on(day).await.unwrap(), Vec::new());
    }
    for section in Section::ALL {
        assert_eq!(store.pick_history(section).await.unwrap(), Vec::new());
        assert_eq!(store.games(&one, section).await.unwrap(), Vec::new());
    }
    assert_eq!(store.games(&other, GENERAL).await.unwrap(), Vec::new());
    // There is nothing left for a player to clear.
    assert_eq!(store.delete_player(&one).await.unwrap(), 0);

    // Wiping what is already empty is fine, and the store works on: a pick
    // for a wiped day is a first pick, a game a first game.
    store.wipe_games_and_picks().await.unwrap();
    let again = pick(days[0], GENERAL, 200);
    assert_eq!(store.save_pick(again).await.unwrap(), again);
    let fresh = playing_game(days[0], 1);
    store.save_game(&one, GENERAL, &fresh).await.unwrap();
    assert_eq!(store.games(&one, GENERAL).await.unwrap(), vec![fresh]);
}

pub async fn wiping_leaves_the_song_pool_and_the_day_offset_alone(store: &dyn Store) {
    add(store, 1, [Genre::Pop, Genre::Rock]).await;
    add(store, 2, []).await;
    store
        .set_preview_failed_on(2, Some(date(2026, 10, 3)))
        .await
        .unwrap();
    store.set_day_offset(12).await.unwrap();
    store.save_pick(pick(DAY, POP, 1)).await.unwrap();
    let pool = store.songs().await.unwrap();

    store.wipe_games_and_picks().await.unwrap();

    // Every song, its genre tags and its failed-preview day included.
    assert_eq!(store.songs().await.unwrap(), pool);
    assert_eq!(pool.len(), 2);
    assert_eq!(pool[1].preview_failed_on, Some(date(2026, 10, 3)));
    // Where the day goes after a reset is the caller's decision.
    assert_eq!(store.day_offset().await.unwrap(), 12);
}

// --- random games ---------------------------------------------------------------

/// When the sample random games were last played, in seconds since the Unix
/// epoch. A store keeps it like the rest of the state and decides nothing by it.
const NOW: i64 = 1_790_000_000;

/// A random game in its `songs`th song of a first session, every earlier one
/// won on the first try and the current one skipped once. The tracks are
/// 1000, 1001, …
fn random_game(songs: u64) -> RandomGame {
    let song = TrackMeta::new("The Song", "", "Someone");
    let mut game = RandomGame::start(None, 1000, DAY, NOW);
    for track_id in 1001..1000 + songs {
        game.guess(&song, &song, NOW).unwrap();
        game = game.next(track_id, DAY, NOW).unwrap();
    }
    game.skip(NOW).unwrap();
    game
}

pub async fn a_new_store_has_no_random_game(store: &dyn Store) {
    let player = PlayerId::generate();
    assert_eq!(store.random_game(&player).await.unwrap(), None);
    // And there is nothing of the kind to delete.
    assert_eq!(store.delete_player(&player).await.unwrap(), 0);
    assert_eq!(store.random_game(&player).await.unwrap(), None);
}

pub async fn a_saved_random_game_is_loaded_as_it_was_saved_and_replaced_by_the_next(
    store: &dyn Store,
) {
    let player = PlayerId::generate();
    let mut game = random_game(3);
    store.save_random_game(&player, &game).await.unwrap();
    let loaded = store.random_game(&player).await.unwrap().unwrap();
    assert_eq!(loaded, game);
    // All of it: the song, the tries, the score and what outlasts a session.
    assert_eq!(loaded.round(), 3);
    assert_eq!(loaded.track_id(), 1002);
    assert_eq!(loaded.game().attempts().len(), 1);
    assert_eq!((loaded.run(), loaded.played(), loaded.won()), (2, 2, 2));
    assert_eq!(loaded.best_run(), 2);
    assert_eq!(loaded.recent(), [1000, 1001, 1002]);
    assert_eq!(loaded.active_at(), NOW);

    // A player has one random game: a save replaces it, whole.
    let wrong = TrackMeta::new("Ünder \"Pressure\" \\ 圧力", "", "Queen & 椎名林檎");
    game.guess(&TrackMeta::new("The Song", "", "Someone"), &wrong, NOW + 60)
        .unwrap();
    store.save_random_game(&player, &game).await.unwrap();
    assert_eq!(store.random_game(&player).await.unwrap(), Some(game));

    // A new session over it is a replacement like any other.
    let again = RandomGame::start(
        store.random_game(&player).await.unwrap().as_ref(),
        7,
        DAY,
        NOW + 3_600,
    );
    store.save_random_game(&player, &again).await.unwrap();
    let loaded = store.random_game(&player).await.unwrap().unwrap();
    assert_eq!(loaded, again);
    assert_eq!(loaded.round(), 4);
    assert_eq!(loaded.best_run(), 2);
    assert_eq!((loaded.run(), loaded.played(), loaded.won()), (0, 0, 0));
}

pub async fn random_games_are_kept_apart_by_player(store: &dyn Store) {
    let one = PlayerId::generate();
    let other = PlayerId::generate();
    let stranger = PlayerId::generate();
    let (first, second) = (random_game(2), random_game(5));
    store.save_random_game(&one, &first).await.unwrap();
    store.save_random_game(&other, &second).await.unwrap();

    assert_eq!(store.random_game(&one).await.unwrap(), Some(first.clone()));
    assert_eq!(store.random_game(&other).await.unwrap(), Some(second));
    assert_eq!(store.random_game(&stranger).await.unwrap(), None);

    // Replacing one player's game leaves the other's alone.
    let replaced = random_game(9);
    store.save_random_game(&other, &replaced).await.unwrap();
    assert_eq!(store.random_game(&one).await.unwrap(), Some(first));
    assert_eq!(store.random_game(&other).await.unwrap(), Some(replaced));
}

pub async fn saving_the_same_random_game_twice_at_once_keeps_one_of_them_whole(store: &dyn Store) {
    let player = PlayerId::generate();
    let (one, other) = (random_game(2), random_game(6));
    let (first, second) = tokio::join!(
        store.save_random_game(&player, &one),
        store.save_random_game(&player, &other)
    );
    first.unwrap();
    second.unwrap();

    // It is one of the two, not a mixture.
    let stored = store.random_game(&player).await.unwrap().unwrap();
    assert!(stored == one || stored == other, "{stored:?}");
}

pub async fn deleting_a_player_removes_their_random_game_and_counts_only_the_daily_games(
    store: &dyn Store,
) {
    let leaving = PlayerId::generate();
    let staying = PlayerId::generate();
    let kept = random_game(4);
    store
        .save_random_game(&leaving, &random_game(3))
        .await
        .unwrap();
    store.save_random_game(&staying, &kept).await.unwrap();
    store
        .save_game(&leaving, GENERAL, &won_game(DAY, 1))
        .await
        .unwrap();
    store
        .save_game(&leaving, ROCK, &lost_game(DAY))
        .await
        .unwrap();

    // The number says how many daily games went; the random game went too.
    assert_eq!(store.delete_player(&leaving).await.unwrap(), 2);
    assert_eq!(store.random_game(&leaving).await.unwrap(), None);
    assert_eq!(store.games(&leaving, GENERAL).await.unwrap(), Vec::new());
    assert_eq!(store.random_game(&staying).await.unwrap(), Some(kept));

    // A player with a random game and nothing else: it goes, and the count
    // is of the daily games, which they never had.
    assert_eq!(store.delete_player(&staying).await.unwrap(), 0);
    assert_eq!(store.random_game(&staying).await.unwrap(), None);

    // The ID is not used up: a game saved under it afterwards is a new start.
    let fresh = random_game(1);
    store.save_random_game(&leaving, &fresh).await.unwrap();
    assert_eq!(store.random_game(&leaving).await.unwrap(), Some(fresh));
}

pub async fn random_games_daily_games_and_the_song_pool_do_not_touch_each_other(store: &dyn Store) {
    let player = PlayerId::generate();
    add(store, 1000, [Genre::Rock]).await;
    add(store, 1001, []).await;
    let random = random_game(2);
    let daily = playing_game(DAY, 2);
    store.save_random_game(&player, &random).await.unwrap();
    store.save_game(&player, ROCK, &daily).await.unwrap();
    store.save_pick(pick(DAY, ROCK, 1000)).await.unwrap();

    // The song being played leaves the pool, and takes no game with it.
    assert!(store.remove_song(1001).await.unwrap());
    assert_eq!(
        store.random_game(&player).await.unwrap(),
        Some(random.clone())
    );

    // A re-roll's two deletions are about a section's day and nothing else.
    assert_eq!(store.delete_games(ROCK, DAY).await.unwrap(), 1);
    assert!(store.remove_pick(DAY, ROCK).await.unwrap());
    assert_eq!(store.random_game(&player).await.unwrap(), Some(random));

    // And saving a random game made no daily game, pick or song.
    assert_eq!(store.games(&player, ROCK).await.unwrap(), Vec::new());
    assert_eq!(store.games(&player, GENERAL).await.unwrap(), Vec::new());
    assert_eq!(store.picks_on(DAY).await.unwrap(), Vec::new());
    assert_eq!(ids(store).await, vec![1000]);
}

pub async fn wiping_deletes_every_random_game(store: &dyn Store) {
    let one = PlayerId::generate();
    let other = PlayerId::generate();
    store.save_random_game(&one, &random_game(2)).await.unwrap();
    store
        .save_random_game(&other, &random_game(7))
        .await
        .unwrap();

    store.wipe_games_and_picks().await.unwrap();

    assert_eq!(store.random_game(&one).await.unwrap(), None);
    assert_eq!(store.random_game(&other).await.unwrap(), None);
    // The store works on: a game saved after the wipe is a first game.
    let fresh = random_game(1);
    store.save_random_game(&one, &fresh).await.unwrap();
    assert_eq!(store.random_game(&one).await.unwrap(), Some(fresh));
}
