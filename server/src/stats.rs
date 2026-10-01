//! A player's record in one section, worked out from their stored games.
//!
//! Nothing is counted while games are played. The games are the record, and
//! the numbers are derived from them on every request, so a game that is
//! deleted (Clear my data, and later an admin re-roll or a clock reset) takes
//! its share of the stats with it and no counter can be left out of step.
//!
//! Pure, like [`crate::game`]: no I/O and no clock. The caller passes in
//! today's date.

use std::collections::BTreeMap;

use jiff::civil::Date;
use serde::Serialize;

use crate::game::{GameState, MAX_ATTEMPTS, Status};

/// What a player has done in one section, as of a given day.
///
/// Serialized with camelCase keys, as the `stats` object of the daily state:
///
/// ```json
/// {"played":12,"won":9,"winPercent":75,"currentStreak":3,"bestStreak":5,"guessDistribution":[0,1,2,3,2,1,0]}
/// ```
///
/// None of it says anything about a song, so it may be shown during a game.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Finished games, won or lost. A game that was started and never
    /// finished is not one of them, and neither is a day without a move.
    pub played: u32,
    /// How many of those were won.
    pub won: u32,
    /// `won` as a share of `played` in whole percent, rounded to the nearest
    /// (a half goes up). 0 when nothing has been played.
    pub win_percent: u32,
    /// Days won in a row up to now: the run of consecutive won days that ends
    /// today, or yesterday while today's game is not finished. A lost game
    /// ends it, and so does a day without a win, played or not.
    pub current_streak: u32,
    /// The longest run of consecutive won days there has been. Never less
    /// than `current_streak`.
    pub best_streak: u32,
    /// The wins by the try they came on: the first number is the wins on the
    /// first try (the 0.1 s clip), the last the wins on the seventh (the
    /// whole preview). They add up to `won`.
    pub guess_distribution: [u32; MAX_ATTEMPTS],
}

impl Stats {
    /// The record made by `games`, which are one player's games in one
    /// section, in any order, as of `today`.
    ///
    /// Games dated after `today` are left out altogether. They exist when the
    /// day has gone backwards (the admin's clock, or the machine's), and a
    /// record "as of today" that counted them would show wins on days that
    /// have not happened; they count again once their day comes round. If a
    /// day is given twice, the last game given for it is the one that counts.
    pub fn as_of<'a>(today: Date, games: impl IntoIterator<Item = &'a GameState>) -> Self {
        // The outcome of each day up to today, oldest first: the status and
        // the number of misses.
        let days: BTreeMap<Date, (Status, usize)> = games
            .into_iter()
            .filter(|game| game.day() <= today)
            .map(|game| (game.day(), (game.status(), game.attempts().len())))
            .collect();

        let mut stats = Self::default();
        // The run of consecutive won days that ends on `last_won`.
        let mut run = 0;
        let mut last_won: Option<Date> = None;
        for (&day, &(status, misses)) in &days {
            if status == Status::Playing {
                continue;
            }
            stats.played += 1;
            if status != Status::Won {
                continue;
            }
            stats.won += 1;
            // A win after `misses` misses came on try `misses + 1`. A won
            // game has at most six, so the slot is always there.
            if let Some(wins) = stats.guess_distribution.get_mut(misses) {
                *wins += 1;
            }
            // Only won days are looked at, so anything else in between (a
            // loss, an unfinished game, no game at all) shows up as a gap.
            let follows_on = last_won.and_then(|won| won.tomorrow().ok()) == Some(day);
            run = if follows_on { run + 1 } else { 1 };
            last_won = Some(day);
            stats.best_streak = stats.best_streak.max(run);
        }

        // After the loop `run` is the run ending on the last won day. It is
        // the current streak if that day is today, or if it is yesterday and
        // today is still open: not having finished today's game yet does not
        // break a streak, losing it does.
        let lost_today = matches!(days.get(&today), Some((Status::Lost, _)));
        let alive = match last_won {
            Some(day) if day == today => true,
            Some(day) => !lost_today && today.yesterday().ok() == Some(day),
            None => false,
        };
        if alive {
            stats.current_streak = run;
        }
        stats.win_percent = percent(stats.won, stats.played);
        stats
    }
}

/// `part` of `whole` in whole percent, rounded to the nearest with a half
/// going up; 0 of nothing is 0.
fn percent(part: u32, whole: u32) -> u32 {
    if whole == 0 {
        return 0;
    }
    let (part, whole) = (u64::from(part), u64::from(whole));
    // round(100 · part / whole) without floats: add half the divisor first.
    let rounded = (part * 200 + whole) / (whole * 2);
    u32::try_from(rounded).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{lost_game, playing_game, won_game};
    use jiff::{ToSpan, civil::date};
    use serde_json::json;

    const TODAY: Date = date(2026, 10, 20);

    /// The day `days` before [`TODAY`] (after it, when negative).
    fn ago(days: i64) -> Date {
        TODAY.checked_sub(days.days()).unwrap()
    }

    fn stats(games: &[GameState]) -> Stats {
        Stats::as_of(TODAY, games)
    }

    /// (current streak, best streak) of `games`.
    fn streaks(games: &[GameState]) -> (u32, u32) {
        let stats = stats(games);
        (stats.current_streak, stats.best_streak)
    }

    // --- nothing, and today only -------------------------------------------

    #[test]
    fn no_games_are_an_empty_record() {
        let expected = Stats {
            played: 0,
            won: 0,
            win_percent: 0,
            current_streak: 0,
            best_streak: 0,
            guess_distribution: [0; 7],
        };
        assert_eq!(stats(&[]), expected);
        assert_eq!(Stats::default(), expected);
    }

    #[test]
    fn a_game_still_being_played_today_counts_for_nothing_yet() {
        assert_eq!(stats(&[GameState::new(TODAY)]), Stats::default());
        assert_eq!(stats(&[playing_game(TODAY, 6)]), Stats::default());
    }

    #[test]
    fn a_win_today_is_a_streak_of_one() {
        assert_eq!(
            stats(&[won_game(TODAY, 2)]),
            Stats {
                played: 1,
                won: 1,
                win_percent: 100,
                current_streak: 1,
                best_streak: 1,
                guess_distribution: [0, 0, 1, 0, 0, 0, 0],
            }
        );
    }

    #[test]
    fn a_loss_today_is_played_and_nothing_else() {
        assert_eq!(
            stats(&[lost_game(TODAY)]),
            Stats {
                played: 1,
                won: 0,
                win_percent: 0,
                current_streak: 0,
                best_streak: 0,
                guess_distribution: [0; 7],
            }
        );
    }

    // --- streaks -----------------------------------------------------------

    #[test]
    fn consecutive_won_days_ending_today_are_the_current_streak() {
        let games = [
            won_game(ago(3), 0),
            won_game(ago(2), 6),
            won_game(ago(1), 3),
            won_game(TODAY, 1),
        ];
        assert_eq!(streaks(&games), (4, 4));
    }

    #[test]
    fn today_not_played_yet_does_not_break_a_streak_through_yesterday() {
        let through_yesterday = [
            won_game(ago(3), 0),
            won_game(ago(2), 0),
            won_game(ago(1), 0),
        ];
        assert_eq!(streaks(&through_yesterday), (3, 3));

        // Nor does a game today that is still going.
        let mut with_today = through_yesterday.to_vec();
        with_today.push(playing_game(TODAY, 4));
        assert_eq!(streaks(&with_today), (3, 3));
        assert_eq!(stats(&with_today).played, 3);
    }

    #[test]
    fn a_loss_today_ends_the_streak_but_not_the_best() {
        let games = [won_game(ago(2), 0), won_game(ago(1), 0), lost_game(TODAY)];
        assert_eq!(streaks(&games), (0, 2));
    }

    #[test]
    fn a_loss_yesterday_ends_the_streak() {
        let games = [won_game(ago(3), 0), won_game(ago(2), 0), lost_game(ago(1))];
        assert_eq!(streaks(&games), (0, 2));

        // A win today starts again from one.
        let mut with_today = games.to_vec();
        with_today.push(won_game(TODAY, 0));
        assert_eq!(streaks(&with_today), (1, 2));
    }

    #[test]
    fn a_missed_day_ends_the_streak() {
        // Nothing at all yesterday.
        let games = [
            won_game(ago(4), 0),
            won_game(ago(3), 0),
            won_game(ago(2), 0),
        ];
        assert_eq!(streaks(&games), (0, 3));

        // A gap in the middle splits the run in two.
        let games = [
            won_game(ago(5), 0),
            won_game(ago(4), 0),
            won_game(ago(3), 0),
            // ago(2): no game
            won_game(ago(1), 0),
            won_game(TODAY, 0),
        ];
        assert_eq!(streaks(&games), (2, 3));
    }

    #[test]
    fn an_unfinished_game_on_an_earlier_day_is_a_missed_day() {
        let games = [
            won_game(ago(3), 0),
            won_game(ago(2), 0),
            // Started yesterday and never finished.
            playing_game(ago(1), 5),
            won_game(TODAY, 0),
        ];
        let stats = stats(&games);
        assert_eq!((stats.current_streak, stats.best_streak), (1, 2));
        // It was not played to the end, so it is not "played" either.
        assert_eq!(stats.played, 3);
        assert_eq!(stats.won, 3);

        // The same with today still open: the streak is already gone.
        assert_eq!(streaks(&games[..3]), (0, 2));
    }

    #[test]
    fn the_best_streak_is_the_longest_run_wherever_it_was() {
        let games = [
            won_game(ago(30), 0),
            lost_game(ago(29)),
            won_game(ago(20), 0),
            won_game(ago(19), 0),
            won_game(ago(18), 0),
            won_game(ago(17), 0),
            lost_game(ago(16)),
            won_game(ago(15), 0),
            won_game(ago(1), 0),
            won_game(TODAY, 0),
        ];
        assert_eq!(streaks(&games), (2, 4));
    }

    #[test]
    fn a_streak_runs_across_month_year_and_leap_day_boundaries() {
        let new_year = [
            won_game(date(2026, 12, 30), 0),
            won_game(date(2026, 12, 31), 0),
            won_game(date(2027, 1, 1), 0),
        ];
        let stats = Stats::as_of(date(2027, 1, 1), &new_year);
        assert_eq!((stats.current_streak, stats.best_streak), (3, 3));

        let leap = [
            won_game(date(2028, 2, 28), 0),
            won_game(date(2028, 2, 29), 0),
            won_game(date(2028, 3, 1), 0),
        ];
        let stats = Stats::as_of(date(2028, 3, 2), &leap);
        assert_eq!((stats.current_streak, stats.best_streak), (3, 3));

        // In a year without the leap day, the 28th and the 1st are neighbours.
        let no_leap = [
            won_game(date(2027, 2, 28), 0),
            won_game(date(2027, 3, 1), 0),
        ];
        assert_eq!(Stats::as_of(date(2027, 3, 1), &no_leap).current_streak, 2);
    }

    #[test]
    fn a_streak_that_ended_long_ago_is_not_current() {
        let games = [won_game(ago(400), 0), won_game(ago(399), 0)];
        assert_eq!(streaks(&games), (0, 2));
    }

    // --- counts ------------------------------------------------------------

    #[test]
    fn played_counts_finished_games_and_the_distribution_counts_wins_by_try() {
        let games = [
            won_game(ago(9), 0),
            won_game(ago(8), 0),
            won_game(ago(7), 1),
            lost_game(ago(6)),
            won_game(ago(5), 6),
            playing_game(ago(4), 2),
            won_game(ago(3), 3),
            lost_game(ago(2)),
            won_game(ago(1), 3),
            playing_game(TODAY, 1),
        ];
        let stats = stats(&games);
        assert_eq!(
            stats,
            Stats {
                played: 8,
                won: 6,
                win_percent: 75,
                current_streak: 1,
                best_streak: 3,
                guess_distribution: [2, 1, 0, 2, 0, 0, 1],
            }
        );
        assert_eq!(stats.guess_distribution.iter().sum::<u32>(), stats.won);
    }

    #[test]
    fn every_try_has_its_own_place_in_the_distribution() {
        for misses in 0..MAX_ATTEMPTS {
            let stats = stats(&[won_game(TODAY, misses)]);
            let mut expected = [0; MAX_ATTEMPTS];
            expected[misses] = 1;
            assert_eq!(stats.guess_distribution, expected, "{misses} misses");
        }
    }

    #[test]
    fn the_win_rate_is_rounded_to_the_nearest_whole_percent() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(0, 5), 0);
        assert_eq!(percent(5, 5), 100);
        assert_eq!(percent(1, 2), 50);
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(2, 3), 67);
        // Exactly a half goes up.
        assert_eq!(percent(1, 8), 13);
        assert_eq!(percent(1, 200), 1);
        assert_eq!(percent(1, 201), 0);
        assert_eq!(percent(199, 200), 100);
        // No overflow at the far end.
        assert_eq!(percent(u32::MAX, u32::MAX), 100);
        assert_eq!(percent(u32::MAX - 1, u32::MAX), 100);

        let games = [won_game(ago(2), 0), lost_game(ago(1)), won_game(TODAY, 0)];
        assert_eq!(stats(&games).win_percent, 67);
    }

    // --- odd input ---------------------------------------------------------

    #[test]
    fn the_order_of_the_games_does_not_matter() {
        let sorted = [
            won_game(ago(5), 2),
            lost_game(ago(4)),
            won_game(ago(2), 0),
            won_game(ago(1), 1),
            won_game(TODAY, 4),
        ];
        let shuffled = [
            sorted[3].clone(),
            sorted[0].clone(),
            sorted[4].clone(),
            sorted[2].clone(),
            sorted[1].clone(),
        ];
        let mut reversed = sorted.to_vec();
        reversed.reverse();

        let expected = stats(&sorted);
        assert_eq!((expected.current_streak, expected.best_streak), (3, 3));
        assert_eq!(stats(&shuffled), expected);
        assert_eq!(stats(&reversed), expected);
    }

    #[test]
    fn games_dated_after_today_are_left_out() {
        // The day went back to the 20th; these were played "later".
        let future = [
            won_game(ago(-1), 0),
            won_game(ago(-2), 0),
            lost_game(ago(-3)),
        ];
        assert_eq!(stats(&future), Stats::default());

        // They do not extend, break or outdo what happened up to today.
        let mut games = vec![won_game(ago(1), 1), won_game(TODAY, 1)];
        let expected = stats(&games);
        assert_eq!((expected.current_streak, expected.best_streak), (2, 2));
        games.extend(future.iter().cloned());
        games.push(won_game(ago(-4), 0));
        games.push(won_game(ago(-5), 0));
        games.push(won_game(ago(-6), 0));
        assert_eq!(stats(&games), expected);

        // Once their day comes round they count like any other.
        let later = Stats::as_of(ago(-2), &games);
        assert_eq!((later.current_streak, later.best_streak), (4, 4));
        assert_eq!(later.played, 4);
    }

    #[test]
    fn a_day_given_twice_counts_once() {
        let games = [won_game(TODAY, 0), won_game(TODAY, 3)];
        assert_eq!(
            stats(&games),
            Stats {
                played: 1,
                won: 1,
                win_percent: 100,
                current_streak: 1,
                best_streak: 1,
                // The last one given.
                guess_distribution: [0, 0, 0, 1, 0, 0, 0],
            }
        );
    }

    #[test]
    fn the_ends_of_the_calendar_do_not_panic() {
        // There is no day before the first or after the last.
        let first = [won_game(Date::MIN, 0)];
        assert_eq!(Stats::as_of(Date::MIN, &first).current_streak, 1);
        assert_eq!(Stats::as_of(Date::MAX, &first).current_streak, 0);

        let last = [
            won_game(Date::MAX.yesterday().unwrap(), 0),
            won_game(Date::MAX, 0),
        ];
        let stats = Stats::as_of(Date::MAX, &last);
        assert_eq!((stats.current_streak, stats.best_streak), (2, 2));
        assert_eq!(Stats::as_of(Date::MIN, &last), Stats::default());
    }

    // --- the API shape -----------------------------------------------------

    #[test]
    fn stats_serialize_with_camel_case_keys() {
        let games = [
            won_game(ago(3), 0),
            lost_game(ago(2)),
            won_game(ago(1), 2),
            won_game(TODAY, 6),
        ];
        assert_eq!(
            serde_json::to_value(stats(&games)).unwrap(),
            json!({
                "played": 4,
                "won": 3,
                "winPercent": 75,
                "currentStreak": 2,
                "bestStreak": 2,
                "guessDistribution": [1, 0, 1, 0, 0, 0, 1],
            })
        );
        assert_eq!(
            serde_json::to_value(Stats::default()).unwrap(),
            json!({
                "played": 0,
                "won": 0,
                "winPercent": 0,
                "currentStreak": 0,
                "bestStreak": 0,
                "guessDistribution": [0, 0, 0, 0, 0, 0, 0],
            })
        );
    }
}
