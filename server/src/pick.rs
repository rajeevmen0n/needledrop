//! The daily pick as a pure function: which song a section plays on a day.
//!
//! No I/O, no clock and no randomness of its own. [`choose`] is given the
//! pool, the section's pick history, what the other sections play that day
//! and a random number generator, and names a track; fetching those and
//! storing the answer is [`crate::daily`]'s work. That keeps the rules in one
//! place, testable with a seeded generator, and independent of the backend
//! the pool and the history are kept in.
//!
//! The rules, in the order they are applied:
//!
//! 1. A section draws on its own pool: the songs tagged with its genre, or
//!    every song for General.
//! 2. A song that another section plays that day is out, so no song is the
//!    answer twice on one day, in whatever order the picks came to be made.
//! 3. A song whose preview failed the check that same day is out for the day.
//! 4. A re-roll does not draw the song it replaces, if there is another.
//! 5. A section does not play yesterday's song again, if there is another.
//! 6. A section does not repeat a song until its pool is used up: songs it
//!    has not played since then are preferred.
//!
//! Rules 2 and 3 are absolute: when they leave nothing, the section has no
//! song that day. Rules 4 to 6 give way when keeping them would leave nothing
//! to play, the later ones first.
//!
//! [`draw_random`] is the draw of random mode, which is not a section but
//! draws on the pool of one, the one its player chose: that pool under rules
//! 1 to 3, and a player's recent songs last.

use std::collections::BTreeSet;

use jiff::civil::Date;
use rand::{Rng, RngExt};

use crate::store::{Genre, Pick, PoolSong, Section};

/// The order the day's picks are made in: the genres first and General last.
///
/// General draws on every song, so it goes last and takes one the genres left
/// over; a genre that went after it could find its only songs taken. Among
/// the genres the order decides who gets a song that carries several tags.
/// It is not [`Section::ALL`]'s order, which is the order the tabs are shown
/// in.
pub const PICK_ORDER: [Section; 4] = [
    Section::Genre(Genre::Pop),
    Section::Genre(Genre::Rock),
    Section::Genre(Genre::HipHop),
    Section::General,
];

/// Whether `song` is in the pool `section` draws on.
fn plays(section: Section, song: &PoolSong) -> bool {
    section
        .genre()
        .is_none_or(|genre| song.genres.contains(&genre))
}

/// The songs `section` could be given on `day`, by ascending track ID: its
/// pool without what the other sections play that day (`taken`) and without
/// the songs whose preview failed the check that day.
///
/// Empty means the section has no song that day, whatever else is tried.
pub fn candidates(
    section: Section,
    day: Date,
    pool: &[PoolSong],
    taken: &BTreeSet<u64>,
) -> Vec<u64> {
    let candidates: BTreeSet<u64> = pool
        .iter()
        .filter(|song| plays(section, song))
        .filter(|song| song.preview_failed_on != Some(day))
        .map(|song| song.track_id)
        .filter(|track_id| !taken.contains(track_id))
        .collect();
    candidates.into_iter().collect()
}

/// The songs of the section's pool that it has played since the pool was last
/// used up.
///
/// The history is replayed, oldest first, against the pool as it is now: each
/// pick is ticked off, and the moment every song of the pool is ticked the
/// round is over and the list is empty again. A pick of a song that has left
/// the pool since counts for nothing; a song added in the middle of a round
/// is simply one more to play before the round ends.
fn played_this_round(
    section: Section,
    day: Date,
    pool: &[PoolSong],
    history: &[Pick],
) -> BTreeSet<u64> {
    let in_pool: BTreeSet<u64> = pool
        .iter()
        .filter(|song| plays(section, song))
        .map(|song| song.track_id)
        .collect();
    let mut earlier: Vec<&Pick> = history
        .iter()
        .filter(|pick| pick.section == section && pick.day < day)
        .collect();
    earlier.sort_unstable_by_key(|pick| pick.day);

    let mut played = BTreeSet::new();
    for pick in earlier {
        if !in_pool.contains(&pick.track_id) {
            continue;
        }
        played.insert(pick.track_id);
        if played.len() == in_pool.len() {
            played.clear();
        }
    }
    played
}

/// The track `section` plays on `day`, or `None` when it has no song that
/// day.
///
/// - `pool` is the whole song pool; the section's part of it is taken here.
/// - `history` is the section's picks on other days, in any order. Picks of
///   other sections, and of `day` or later (the clock can be set back), are
///   ignored.
/// - `taken` is what the other sections play on `day`.
/// - `replaced` is the song a re-roll is replacing, if this is one.
///
/// Which of the songs that are left it is, is up to `rng`. The candidates
/// are put in ascending track ID order first, so the same generator in the
/// same state gives the same answer.
pub fn choose(
    section: Section,
    day: Date,
    pool: &[PoolSong],
    history: &[Pick],
    taken: &BTreeSet<u64>,
    replaced: Option<u64>,
    rng: &mut (impl Rng + ?Sized),
) -> Option<u64> {
    let mut candidates = candidates(section, day, pool, taken);
    if candidates.is_empty() {
        return None;
    }

    // The two songs it would rather not draw, the stronger wish first. Each
    // is dropped only while that leaves another song to play.
    let yesterdays = day.yesterday().ok().and_then(|yesterday| {
        history
            .iter()
            .find(|pick| pick.section == section && pick.day == yesterday)
            .map(|pick| pick.track_id)
    });
    for unwanted in [replaced, yesterdays].into_iter().flatten() {
        if candidates.len() > 1 {
            candidates.retain(|&track_id| track_id != unwanted);
        }
    }

    let played = played_this_round(section, day, pool, history);
    let fresh: Vec<u64> = candidates
        .iter()
        .copied()
        .filter(|track_id| !played.contains(track_id))
        .collect();
    // Nothing fresh among them (the fresh songs are all playing elsewhere
    // today, say): then any of them, rather than no song.
    let from = if fresh.is_empty() {
        &candidates
    } else {
        &fresh
    };
    Some(from[rng.random_range(0..from.len())])
}

/// The track random mode gives a player next, or `None` when there is nothing
/// to give.
///
/// Random mode is not a section and has no day of its own, but its player
/// chooses which section's pool it draws on: `section` is General for every
/// song, or a genre for the songs tagged with it. And it lives next to the
/// daily games, so two of their rules hold here too and are as absolute: a
/// song that any section plays on `day` (`taken`) is out, so that playing
/// random never gives away a daily game, and so is a song whose preview
/// failed the check that day. What is left of the section's pool are the
/// candidates, the very ones the section itself could be given that day
/// ([`candidates`]).
///
/// `recent` is what this player was given lately, oldest first, the song
/// being replaced last, from whichever pool each of them was drawn. One of
/// the candidates that is not among them is drawn by `rng`. When every
/// candidate is recent (a small pool, or a small genre), it is the one
/// played longest ago, which `rng` has no say in: the player goes round the
/// pool in the same order, and the song just played comes back only when
/// there is no other. A recent song of another genre is no candidate, so it
/// neither comes back nor keeps a song of this genre from coming back.
pub fn draw_random(
    section: Section,
    day: Date,
    pool: &[PoolSong],
    taken: &BTreeSet<u64>,
    recent: &[u64],
    rng: &mut (impl Rng + ?Sized),
) -> Option<u64> {
    let candidates = candidates(section, day, pool, taken);
    let fresh: Vec<u64> = candidates
        .iter()
        .copied()
        .filter(|track_id| !recent.contains(track_id))
        .collect();
    if !fresh.is_empty() {
        return Some(fresh[rng.random_range(0..fresh.len())]);
    }
    // Every one of them is in `recent`: the one whose last turn is furthest
    // back. Positions are distinct, so there is one answer.
    candidates
        .into_iter()
        .min_by_key(|track_id| recent.iter().rposition(|played| played == track_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Genres, NewSong, SEED_SONGS};
    use jiff::civil::date;
    use rand::{SeedableRng, rngs::StdRng};
    use std::collections::BTreeMap;

    const POP: Section = Section::Genre(Genre::Pop);
    const ROCK: Section = Section::Genre(Genre::Rock);
    const HIP_HOP: Section = Section::Genre(Genre::HipHop);
    const DAY: Date = date(2026, 10, 10);

    fn song<const N: usize>(track_id: u64, genres: [Genre; N]) -> PoolSong {
        PoolSong::new(
            NewSong {
                track_id,
                title: format!("Song {track_id}"),
                title_short: format!("Song {track_id}"),
                artist: "Someone".to_owned(),
                album: "LP".to_owned(),
            },
            Genres::from(genres),
        )
    }

    /// The six seed songs, as the pool holds them.
    fn seed_pool() -> Vec<PoolSong> {
        SEED_SONGS
            .iter()
            .map(|seed| {
                let mut song = song(seed.track_id, []);
                song.genres = seed.genres.iter().copied().collect();
                song
            })
            .collect()
    }

    fn rng(seed: u64) -> StdRng {
        StdRng::seed_from_u64(seed)
    }

    fn pick(day: Date, section: Section, track_id: u64) -> Pick {
        Pick {
            day,
            section,
            track_id,
        }
    }

    fn days_before(day: Date, days: i64) -> Date {
        day.checked_sub(jiff::Span::new().days(days)).unwrap()
    }

    /// The set of answers `choose` gives over many generators.
    fn outcomes(
        section: Section,
        pool: &[PoolSong],
        history: &[Pick],
        taken: &[u64],
        replaced: Option<u64>,
    ) -> BTreeSet<Option<u64>> {
        let taken: BTreeSet<u64> = taken.iter().copied().collect();
        (0..200)
            .map(|seed| {
                choose(
                    section,
                    DAY,
                    pool,
                    history,
                    &taken,
                    replaced,
                    &mut rng(seed),
                )
            })
            .collect()
    }

    fn only(track_id: u64) -> BTreeSet<Option<u64>> {
        BTreeSet::from([Some(track_id)])
    }

    fn any_of<const N: usize>(track_ids: [u64; N]) -> BTreeSet<Option<u64>> {
        track_ids.into_iter().map(Some).collect()
    }

    /// Plays `days` days from `first`, all four sections in [`PICK_ORDER`],
    /// and returns each day's picks by section.
    fn simulate(
        pool: &[PoolSong],
        first: Date,
        days: usize,
        seed: u64,
    ) -> Vec<BTreeMap<Section, u64>> {
        let mut rng = rng(seed);
        let mut history: Vec<Pick> = Vec::new();
        let mut played = Vec::new();
        let mut day = first;
        for _ in 0..days {
            let mut today = BTreeMap::new();
            for section in PICK_ORDER {
                let taken: BTreeSet<u64> = today.values().copied().collect();
                if let Some(track_id) = choose(section, day, pool, &history, &taken, None, &mut rng)
                {
                    today.insert(section, track_id);
                    history.push(pick(day, section, track_id));
                }
            }
            played.push(today);
            day = day.tomorrow().unwrap();
        }
        played
    }

    // --- the order and the pools ----------------------------------------------

    #[test]
    fn the_picks_are_made_genres_first_and_general_last() {
        assert_eq!(PICK_ORDER, [POP, ROCK, HIP_HOP, Section::General]);
        // Every section is in it once.
        let all: BTreeSet<Section> = PICK_ORDER.into_iter().collect();
        assert_eq!(all, Section::ALL.into_iter().collect());
    }

    #[test]
    fn a_genre_section_draws_only_on_songs_tagged_with_it() {
        let pool = [
            song(1, [Genre::Pop]),
            song(2, [Genre::Rock]),
            song(3, [Genre::Pop, Genre::HipHop]),
            song(4, []),
        ];
        assert_eq!(outcomes(POP, &pool, &[], &[], None), any_of([1, 3]));
        assert_eq!(outcomes(ROCK, &pool, &[], &[], None), only(2));
        assert_eq!(outcomes(HIP_HOP, &pool, &[], &[], None), only(3));
    }

    #[test]
    fn general_draws_on_every_song_tagged_or_not() {
        let pool = [
            song(1, [Genre::Pop]),
            song(2, [Genre::Rock, Genre::HipHop]),
            song(3, []),
        ];
        assert_eq!(
            outcomes(Section::General, &pool, &[], &[], None),
            any_of([1, 2, 3])
        );
    }

    #[test]
    fn a_section_without_songs_has_no_song() {
        let pool = [song(1, [Genre::Pop]), song(2, [])];
        assert_eq!(
            outcomes(ROCK, &pool, &[], &[], None),
            BTreeSet::from([None])
        );
        for section in Section::ALL {
            assert_eq!(
                outcomes(section, &[], &[], &[], None),
                BTreeSet::from([None]),
                "{section}"
            );
        }
    }

    // --- what is out for the day ------------------------------------------------

    #[test]
    fn a_song_another_section_plays_today_is_never_drawn() {
        let pool = [
            song(1, [Genre::Pop, Genre::Rock]),
            song(2, [Genre::Rock]),
            song(3, []),
        ];
        // Pop took the song both genres share; General is left with one.
        assert_eq!(outcomes(ROCK, &pool, &[], &[1], None), only(2));
        assert_eq!(
            outcomes(Section::General, &pool, &[], &[1, 2], None),
            only(3)
        );
        // It holds in any order: Pop drawing after General took its song.
        assert_eq!(
            outcomes(POP, &pool, &[], &[1], None),
            BTreeSet::from([None])
        );
    }

    #[test]
    fn a_section_whose_songs_are_all_taken_has_no_song() {
        let pool = [song(1, [Genre::Pop, Genre::Rock])];
        assert_eq!(
            outcomes(ROCK, &pool, &[], &[1], None),
            BTreeSet::from([None])
        );
        assert_eq!(
            outcomes(Section::General, &pool, &[], &[1], None),
            BTreeSet::from([None])
        );
        // Not even a re-roll or the lack of anything else brings it back.
        assert_eq!(
            outcomes(ROCK, &pool, &[], &[1], Some(1)),
            BTreeSet::from([None])
        );
    }

    #[test]
    fn a_song_whose_preview_failed_today_is_out_for_today_only() {
        let mut failed_today = song(1, [Genre::Pop]);
        failed_today.preview_failed_on = Some(DAY);
        let mut failed_yesterday = song(2, [Genre::Pop]);
        failed_yesterday.preview_failed_on = Some(DAY.yesterday().unwrap());
        let pool = [
            failed_today.clone(),
            failed_yesterday,
            song(3, [Genre::Pop]),
        ];

        assert_eq!(outcomes(POP, &pool, &[], &[], None), any_of([2, 3]));
        assert_eq!(
            outcomes(Section::General, &pool, &[], &[], None),
            any_of([2, 3])
        );
        // When it is the only song, the section has none today.
        assert_eq!(
            outcomes(POP, &[failed_today], &[], &[], None),
            BTreeSet::from([None])
        );
        assert_eq!(candidates(POP, DAY, &pool, &BTreeSet::new()), vec![2, 3]);
        // The day after, it is tried again.
        let tomorrow = DAY.tomorrow().unwrap();
        assert_eq!(
            candidates(POP, tomorrow, &pool, &BTreeSet::new()),
            vec![1, 2, 3]
        );
    }

    // --- no repeats ---------------------------------------------------------------

    #[test]
    fn a_song_is_not_repeated_until_the_pool_is_used_up() {
        let pool = [
            song(1, [Genre::Pop]),
            song(2, [Genre::Pop]),
            song(3, [Genre::Pop]),
            song(4, [Genre::Pop]),
        ];
        let history = [
            pick(days_before(DAY, 3), POP, 1),
            pick(days_before(DAY, 2), POP, 4),
        ];
        assert_eq!(outcomes(POP, &pool, &history, &[], None), any_of([2, 3]));

        // One left in the round: that one, whatever the generator says.
        let history = [
            pick(days_before(DAY, 5), POP, 1),
            pick(days_before(DAY, 4), POP, 4),
            pick(days_before(DAY, 2), POP, 2),
        ];
        assert_eq!(outcomes(POP, &pool, &history, &[], None), only(3));
    }

    #[test]
    fn a_used_up_pool_starts_again_but_not_with_yesterdays_song() {
        let pool = [
            song(1, [Genre::Pop]),
            song(2, [Genre::Pop]),
            song(3, [Genre::Pop]),
        ];
        // All three played; 2 was yesterday's.
        let history = [
            pick(days_before(DAY, 3), POP, 3),
            pick(days_before(DAY, 2), POP, 1),
            pick(days_before(DAY, 1), POP, 2),
        ];
        assert_eq!(outcomes(POP, &pool, &history, &[], None), any_of([1, 3]));

        // The round that follows counts from there: once 1 has been played
        // again, the other two are the fresh ones.
        let mut history = history.to_vec();
        history.push(pick(DAY, POP, 1));
        let next = DAY.tomorrow().unwrap();
        let drawn: BTreeSet<Option<u64>> = (0..200)
            .map(|seed| {
                choose(
                    POP,
                    next,
                    &pool,
                    &history,
                    &BTreeSet::new(),
                    None,
                    &mut rng(seed),
                )
            })
            .collect();
        assert_eq!(drawn, any_of([2, 3]));
    }

    #[test]
    fn yesterdays_song_is_played_again_only_when_there_is_no_other() {
        let yesterday = DAY.yesterday().unwrap();
        let one = [song(1, [Genre::Rock])];
        let history = [pick(yesterday, ROCK, 1)];
        // A pool of one plays its song every day.
        assert_eq!(outcomes(ROCK, &one, &history, &[], None), only(1));

        // With a second song that is free, never.
        let two = [song(1, [Genre::Rock]), song(2, [Genre::Rock])];
        assert_eq!(outcomes(ROCK, &two, &history, &[], None), only(2));
        // With a second song that another section plays today, it has to.
        assert_eq!(outcomes(ROCK, &two, &history, &[2], None), only(1));
        // A pick from the day before yesterday is no reason to avoid a song
        // once the round is over.
        let history = [
            pick(days_before(DAY, 3), ROCK, 2),
            pick(days_before(DAY, 2), ROCK, 1),
        ];
        assert_eq!(outcomes(ROCK, &two, &history, &[], None), any_of([1, 2]));
    }

    #[test]
    fn when_every_fresh_song_is_taken_a_played_one_is_drawn_rather_than_none() {
        let pool = [song(1, []), song(2, []), song(3, [])];
        // 1 was played this round; 2 and 3, the fresh ones, play elsewhere today.
        let history = [pick(days_before(DAY, 2), Section::General, 1)];
        assert_eq!(
            outcomes(Section::General, &pool, &history, &[2, 3], None),
            only(1)
        );
    }

    #[test]
    fn the_history_counts_only_this_section_earlier_days_and_songs_still_in_the_pool() {
        let pool = [
            song(1, [Genre::Pop]),
            song(2, [Genre::Pop]),
            song(3, [Genre::Pop]),
        ];
        let history = [
            // Another section's pick of the same song.
            pick(days_before(DAY, 2), Section::General, 1),
            // A pick of today and of a later day: the clock was set back.
            pick(DAY, POP, 2),
            pick(DAY.tomorrow().unwrap(), POP, 3),
            // A song that has left the pool since.
            pick(days_before(DAY, 3), POP, 99),
        ];
        assert_eq!(outcomes(POP, &pool, &history, &[], None), any_of([1, 2, 3]));

        // A removed song does not end a round early either: 1 and 2 are
        // played, 3 is the one that is left.
        let history = [
            pick(days_before(DAY, 4), POP, 1),
            pick(days_before(DAY, 3), POP, 99),
            pick(days_before(DAY, 2), POP, 2),
        ];
        assert_eq!(outcomes(POP, &pool, &history, &[], None), only(3));
    }

    #[test]
    fn a_song_added_in_the_middle_of_a_round_is_played_before_the_round_ends() {
        // 1 and 2 were the whole pool and both were played; 2 the day before
        // yesterday, so nothing but the round keeps it out. Then 3 was added.
        let pool = [song(1, []), song(2, []), song(3, [])];
        let history = [
            pick(days_before(DAY, 3), Section::General, 1),
            pick(days_before(DAY, 2), Section::General, 2),
        ];
        assert_eq!(
            outcomes(Section::General, &pool, &history, &[], None),
            only(3)
        );
    }

    // --- re-rolls -----------------------------------------------------------------

    #[test]
    fn a_reroll_draws_another_song_when_there_is_one() {
        let pool = [
            song(1, [Genre::Pop]),
            song(2, [Genre::Pop]),
            song(3, [Genre::Pop]),
        ];
        assert_eq!(outcomes(POP, &pool, &[], &[], Some(2)), any_of([1, 3]));

        // The only song is drawn again.
        let one = [song(1, [Genre::Pop])];
        assert_eq!(outcomes(POP, &one, &[], &[], Some(1)), only(1));
        // So is the only one that no other section plays today.
        assert_eq!(outcomes(POP, &pool, &[], &[1, 3], Some(2)), only(2));
        // A replaced song that is not a candidate changes nothing.
        assert_eq!(outcomes(POP, &pool, &[], &[], Some(99)), any_of([1, 2, 3]));
    }

    #[test]
    fn a_reroll_would_rather_repeat_yesterday_than_draw_the_same_song_again() {
        // The seed's situation: two songs, yesterday's and today's.
        let pool = [song(1, [Genre::Pop]), song(2, [Genre::Pop])];
        let history = [pick(DAY.yesterday().unwrap(), POP, 1)];
        assert_eq!(outcomes(POP, &pool, &history, &[], Some(2)), only(1));

        // With a third song, it is neither.
        let pool = [
            song(1, [Genre::Pop]),
            song(2, [Genre::Pop]),
            song(3, [Genre::Pop]),
        ];
        assert_eq!(outcomes(POP, &pool, &history, &[], Some(2)), only(3));
    }

    // --- the draw -------------------------------------------------------------------

    #[test]
    fn the_same_generator_gives_the_same_song_whatever_order_the_pool_is_in() {
        let mut pool: Vec<PoolSong> = (1..=40).map(|id| song(id, [Genre::Pop])).collect();
        let taken = BTreeSet::new();
        let first = choose(POP, DAY, &pool, &[], &taken, None, &mut rng(7));
        pool.reverse();
        let again = choose(POP, DAY, &pool, &[], &taken, None, &mut rng(7));
        assert_eq!(first, again);
        assert!(first.is_some());
    }

    #[test]
    fn every_candidate_can_be_drawn() {
        let pool: Vec<PoolSong> = (1..=5).map(|id| song(id, [])).collect();
        assert_eq!(
            outcomes(Section::General, &pool, &[], &[], None),
            any_of([1, 2, 3, 4, 5])
        );
    }

    // --- the seed pool, day after day -----------------------------------------------

    #[test]
    fn with_the_seed_each_genre_alternates_its_two_songs() {
        let pool = seed_pool();
        for seed in 0..20 {
            let days = simulate(&pool, date(2026, 10, 1), 30, seed);
            for genre in Genre::ALL {
                let section = Section::Genre(genre);
                let own: BTreeSet<u64> = pool
                    .iter()
                    .filter(|song| song.genres.contains(&genre))
                    .map(|song| song.track_id)
                    .collect();
                let played: Vec<u64> = days.iter().map(|day| day[&section]).collect();
                assert!(played.iter().all(|track_id| own.contains(track_id)));
                // Never the same song two days running: with two songs that
                // is strict alternation.
                assert!(
                    played.windows(2).all(|pair| pair[0] != pair[1]),
                    "seed {seed}, {section}: {played:?}"
                );
            }
        }
    }

    #[test]
    fn with_the_seed_general_gets_one_of_the_three_the_genres_did_not_take() {
        let pool = seed_pool();
        let everything: BTreeSet<u64> = pool.iter().map(|song| song.track_id).collect();
        let mut general_played = BTreeSet::new();
        for seed in 0..20 {
            for day in simulate(&pool, date(2026, 10, 1), 30, seed) {
                // Four sections, four different songs.
                assert_eq!(day.len(), 4);
                let distinct: BTreeSet<u64> = day.values().copied().collect();
                assert_eq!(distinct.len(), 4, "seed {seed}: {day:?}");

                let genres: BTreeSet<u64> = Genre::ALL
                    .into_iter()
                    .map(|genre| day[&Section::Genre(genre)])
                    .collect();
                let left: BTreeSet<u64> = everything.difference(&genres).copied().collect();
                assert_eq!(left.len(), 3);
                assert!(
                    left.contains(&day[&Section::General]),
                    "seed {seed}: {day:?}"
                );
                general_played.insert(day[&Section::General]);
            }
        }
        // Over time General plays every song of the pool.
        assert_eq!(general_played, everything);
    }

    #[test]
    fn with_the_seed_general_plays_all_six_songs_before_it_repeats_one() {
        let pool = seed_pool();
        for seed in 0..20 {
            let days = simulate(&pool, date(2026, 10, 1), 24, seed);
            let general: Vec<u64> = days.iter().map(|day| day[&Section::General]).collect();
            // The genres alternate, so General's three candidates alternate
            // too, and six days use up its pool exactly: every round of six
            // is the six songs.
            for round in general.chunks(6) {
                let distinct: BTreeSet<u64> = round.iter().copied().collect();
                assert_eq!(distinct.len(), 6, "seed {seed}: {general:?}");
            }
        }
    }

    #[test]
    fn a_song_with_two_tags_goes_to_the_genre_that_picks_first() {
        // One song both genres want and nothing else for either: Pop is
        // first in the order, so Rock has no song, on every day.
        let pool = [song(1, [Genre::Pop, Genre::Rock]), song(2, [])];
        for day in simulate(&pool, date(2026, 10, 1), 5, 3) {
            assert_eq!(day.get(&POP), Some(&1));
            assert_eq!(day.get(&ROCK), None);
            assert_eq!(day.get(&HIP_HOP), None);
            assert_eq!(day.get(&Section::General), Some(&2));
        }
    }

    // --- the random draw ----------------------------------------------------------

    /// The set of answers `draw_random` gives over many generators, drawing
    /// on the whole pool.
    fn random_outcomes(pool: &[PoolSong], taken: &[u64], recent: &[u64]) -> BTreeSet<Option<u64>> {
        random_outcomes_in(Section::General, pool, taken, recent)
    }

    /// The same, drawing on the pool of `section`.
    fn random_outcomes_in(
        section: Section,
        pool: &[PoolSong],
        taken: &[u64],
        recent: &[u64],
    ) -> BTreeSet<Option<u64>> {
        let taken: BTreeSet<u64> = taken.iter().copied().collect();
        (0..200)
            .map(|seed| draw_random(section, DAY, pool, &taken, recent, &mut rng(seed)))
            .collect()
    }

    #[test]
    fn the_random_draw_from_general_is_from_the_whole_pool_whatever_the_tags() {
        let pool = [
            song(1, []),
            song(2, [Genre::Pop]),
            song(3, [Genre::Rock, Genre::HipHop]),
            song(4, [Genre::HipHop]),
        ];
        assert_eq!(random_outcomes(&pool, &[], &[]), any_of([1, 2, 3, 4]));
    }

    #[test]
    fn the_random_draw_never_gives_a_song_a_section_plays_that_day() {
        let pool = [
            song(1, []),
            song(2, [Genre::Pop]),
            song(3, [Genre::Rock]),
            song(4, []),
            song(5, []),
        ];
        assert_eq!(random_outcomes(&pool, &[1, 2, 3], &[]), any_of([4, 5]));
        // Not even when it is all there is, and whatever was played lately.
        assert_eq!(
            random_outcomes(&pool, &[1, 2, 3, 4, 5], &[]),
            BTreeSet::from([None])
        );
        assert_eq!(random_outcomes(&pool, &[1, 2, 3, 4], &[5, 5, 5]), only(5));
    }

    #[test]
    fn the_random_draw_leaves_out_a_song_whose_preview_failed_that_day() {
        let mut pool = vec![song(1, []), song(2, []), song(3, [])];
        pool[0].preview_failed_on = Some(DAY);
        // A failure on another day is history.
        pool[1].preview_failed_on = Some(days_before(DAY, 1));
        assert_eq!(random_outcomes(&pool, &[], &[]), any_of([2, 3]));

        pool[1].preview_failed_on = Some(DAY);
        pool[2].preview_failed_on = Some(DAY);
        assert_eq!(random_outcomes(&pool, &[], &[]), BTreeSet::from([None]));
    }

    #[test]
    fn the_random_draw_of_an_empty_pool_is_nothing() {
        assert_eq!(random_outcomes(&[], &[], &[9]), BTreeSet::from([None]));
    }

    #[test]
    fn the_random_draw_avoids_the_recent_songs_while_another_is_left() {
        let pool: Vec<PoolSong> = (1..=6).map(|id| song(id, [])).collect();
        assert_eq!(random_outcomes(&pool, &[], &[3, 1, 5]), any_of([2, 4, 6]));
        assert_eq!(random_outcomes(&pool, &[], &[3, 1, 5, 2, 4]), only(6));
        // Songs that have left the pool since, and a song listed twice,
        // change nothing.
        assert_eq!(
            random_outcomes(&pool, &[], &[77, 3, 1, 3, 5, 88]),
            any_of([2, 4, 6])
        );
        // What a section plays is out before the recent ones are looked at.
        assert_eq!(random_outcomes(&pool, &[6], &[3, 1, 5, 2]), only(4));
    }

    #[test]
    fn when_every_candidate_is_recent_the_one_played_longest_ago_comes_back() {
        let pool: Vec<PoolSong> = (1..=4).map(|id| song(id, [])).collect();
        // Oldest first: 2 was played longest ago.
        assert_eq!(random_outcomes(&pool, &[], &[2, 4, 1, 3]), only(2));
        // A song played twice counts by its last turn.
        assert_eq!(random_outcomes(&pool, &[], &[2, 4, 1, 3, 2]), only(4));
        // The oldest may be playing in a section today: then the next oldest.
        assert_eq!(random_outcomes(&pool, &[2], &[2, 4, 1, 3]), only(4));
        // The song just played comes back only when there is no other.
        assert_eq!(random_outcomes(&pool[..1], &[], &[1]), only(1));
        assert_eq!(random_outcomes(&pool[..2], &[], &[2, 1]), only(2));
    }

    #[test]
    fn a_player_goes_round_a_small_pool_without_a_repeat_inside_a_round() {
        let pool: Vec<PoolSong> = (1..=5).map(|id| song(id, [])).collect();
        let taken = BTreeSet::from([5]);
        for seed in 0..20 {
            let mut rng = rng(seed);
            let mut recent: Vec<u64> = Vec::new();
            for _ in 0..12 {
                let drawn =
                    draw_random(Section::General, DAY, &pool, &taken, &recent, &mut rng).unwrap();
                recent.push(drawn);
            }
            // Four candidates: every four draws in a row are the four songs,
            // and once the first round is over the order repeats.
            for round in recent.chunks(4) {
                let distinct: BTreeSet<u64> = round.iter().copied().collect();
                assert_eq!(distinct, BTreeSet::from([1, 2, 3, 4]), "seed {seed}");
            }
            assert_eq!(recent[..4], recent[4..8], "seed {seed}");
        }
    }

    // --- the random draw from one genre ---------------------------------------------

    /// A pool with every kind of song: untagged, of one genre, of several.
    fn mixed_pool() -> Vec<PoolSong> {
        vec![
            song(1, []),
            song(2, [Genre::Pop]),
            song(3, [Genre::Pop]),
            song(4, [Genre::Rock]),
            song(5, [Genre::Pop, Genre::Rock]),
            song(6, [Genre::Pop, Genre::HipHop]),
            song(7, []),
        ]
    }

    #[test]
    fn the_random_draw_from_a_genre_stays_inside_it() {
        let pool = mixed_pool();
        assert_eq!(
            random_outcomes_in(POP, &pool, &[], &[]),
            any_of([2, 3, 5, 6])
        );
        // A song with several tags is in the pool of each of them.
        assert_eq!(random_outcomes_in(ROCK, &pool, &[], &[]), any_of([4, 5]));
        assert_eq!(random_outcomes_in(HIP_HOP, &pool, &[], &[]), only(6));
        // And General is still all of it.
        assert_eq!(
            random_outcomes_in(Section::General, &pool, &[], &[]),
            any_of([1, 2, 3, 4, 5, 6, 7])
        );
    }

    #[test]
    fn the_random_draw_from_a_genre_never_gives_a_song_a_section_plays_that_day() {
        let pool = mixed_pool();
        // Whichever section it is that plays it: say Pop its own 2, General
        // the 3, and Rock the 5 that Pop could have had too.
        assert_eq!(random_outcomes_in(POP, &pool, &[2, 3, 5], &[]), only(6));
        assert_eq!(random_outcomes_in(ROCK, &pool, &[2, 3, 5], &[]), only(4));
        // Not when it is all the genre has, whatever was played lately...
        assert_eq!(
            random_outcomes_in(HIP_HOP, &pool, &[6], &[]),
            BTreeSet::from([None])
        );
        assert_eq!(
            random_outcomes_in(POP, &pool, &[2, 3, 5, 6], &[1, 4, 7]),
            BTreeSet::from([None])
        );
        // ...and the one that is left comes back rather than one of them.
        assert_eq!(random_outcomes_in(POP, &pool, &[2, 3, 5], &[6, 6]), only(6));
    }

    #[test]
    fn the_random_draw_from_a_genre_leaves_out_a_song_whose_preview_failed_that_day() {
        let mut pool = mixed_pool();
        pool[1].preview_failed_on = Some(DAY);
        // A failure on another day is history.
        pool[2].preview_failed_on = Some(days_before(DAY, 1));
        assert_eq!(random_outcomes_in(POP, &pool, &[], &[]), any_of([3, 5, 6]));
        // The only song of a genre, failed today: nothing to draw there,
        // while the other pools go on without it.
        pool[5].preview_failed_on = Some(DAY);
        assert_eq!(
            random_outcomes_in(HIP_HOP, &pool, &[], &[]),
            BTreeSet::from([None])
        );
        assert_eq!(random_outcomes_in(POP, &pool, &[], &[]), any_of([3, 5]));
    }

    #[test]
    fn the_random_draw_from_a_genre_without_songs_is_nothing() {
        // Nothing is tagged hip-hop, however much else there is and whatever
        // the player was given lately.
        let pool = [song(1, []), song(2, [Genre::Pop]), song(3, [Genre::Rock])];
        assert_eq!(
            random_outcomes_in(HIP_HOP, &pool, &[], &[]),
            BTreeSet::from([None])
        );
        assert_eq!(
            random_outcomes_in(HIP_HOP, &pool, &[], &[1, 2, 3]),
            BTreeSet::from([None])
        );
        for section in Section::ALL {
            assert_eq!(
                random_outcomes_in(section, &[], &[], &[9]),
                BTreeSet::from([None]),
                "{section}"
            );
        }
    }

    #[test]
    fn the_recent_songs_are_avoided_inside_the_genre() {
        let pool = mixed_pool();
        // Pop has 2, 3, 5 and 6. Recent songs of the other pools (1, 4, 7)
        // are no candidates and keep nothing out.
        assert_eq!(
            random_outcomes_in(POP, &pool, &[], &[1, 3, 4, 7]),
            any_of([2, 5, 6])
        );
        assert_eq!(
            random_outcomes_in(POP, &pool, &[], &[2, 1, 6, 4, 3]),
            only(5)
        );
        // Every pop song is recent: the one played longest ago comes back,
        // and it is a pop song, although 1 and 4 were played before it.
        assert_eq!(
            random_outcomes_in(POP, &pool, &[], &[1, 4, 6, 2, 7, 5, 3]),
            only(6)
        );
        // What a section plays today is out before the recent ones are
        // looked at.
        assert_eq!(
            random_outcomes_in(POP, &pool, &[6], &[1, 4, 6, 2, 7, 5, 3]),
            only(2)
        );
        // The song just played comes back only when the genre has no other.
        assert_eq!(random_outcomes_in(ROCK, &pool, &[], &[5, 4]), only(5));
        assert_eq!(random_outcomes_in(HIP_HOP, &pool, &[], &[6]), only(6));
    }

    #[test]
    fn a_player_who_changes_genre_goes_round_the_new_one_from_its_least_recent_song() {
        let pool = mixed_pool();
        let taken = BTreeSet::new();
        for seed in 0..20 {
            let mut rng = rng(seed);
            // A while in the whole pool: all seven songs, each once.
            let mut recent: Vec<u64> = Vec::new();
            for _ in 0..7 {
                let drawn =
                    draw_random(Section::General, DAY, &pool, &taken, &recent, &mut rng).unwrap();
                recent.push(drawn);
            }
            let earlier_rock = *recent.iter().find(|id| [4, 5].contains(id)).unwrap();
            let later_rock = 4 + 5 - earlier_rock;

            // Then rock only. Both rock songs are recent, so the one played
            // longer ago is the first to come back, then the other, in turns.
            let mut rock = Vec::new();
            for _ in 0..4 {
                let drawn = draw_random(ROCK, DAY, &pool, &taken, &recent, &mut rng).unwrap();
                // What the game does with a song it is given: it becomes
                // the latest of the recent ones, once.
                recent.retain(|played| *played != drawn);
                recent.push(drawn);
                rock.push(drawn);
            }
            assert_eq!(
                rock,
                [earlier_rock, later_rock, earlier_rock, later_rock],
                "seed {seed}"
            );
        }
    }
}
