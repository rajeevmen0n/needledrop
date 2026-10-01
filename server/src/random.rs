//! Random mode: one song after another, for as long as the player likes.
//!
//! The game is the daily one (the same ladder, the same matching, a skip or
//! a wrong guess costs a try), played against a song drawn from the whole
//! pool instead of the day's pick. When a song is over the player asks for
//! the next, without limit. Nothing here has a day: a random game belongs to
//! a player and to nothing else.
//!
//! **The state** is one [`RandomGame`] per player, kept in the store and
//! replaced whole: the song being played and the tries used on it, the score
//! of the session, and the two things that outlast a session, the longest
//! run and the songs played lately. It is pure: no I/O, no clock.
//!
//! **A session** is what the client says it is. `POST /api/random/start`
//! begins one, abandoning whatever song was there; the page calls it when a
//! browser tab opens random mode for the first time, and otherwise asks for
//! the game that is under way (`GET /api/random`). So a reload goes on where
//! it was, and a new tab starts from scratch.
//!
//! **The round** is what keeps two tabs apart. Every song drawn for a player
//! gets the next number, and a move or a "next song" has to name the round
//! it is for. One that names another round comes from a tab that has not
//! seen a song go by; it is refused and changes nothing.
//!
//! **The rule of the daily routes holds here too:** while a song is being
//! played, nothing that identifies it leaves the server. The answer goes out
//! through one place only ([`RandomView::new`]), and only once the song is
//! won or lost; the round is a counter and says nothing about the track.
//!
//! Drawing and loading the songs is [`crate::daily`]'s work, because a random
//! song must not be one of the day's four and comes from the same Deezer.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::PrivateCookieJar;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::{
    daily::{Answer, Song},
    game::{self, Attempt, GameError, GameState, MAX_ATTEMPTS, Status, TrackMeta},
    player,
    routes::{ApiError, AppState, daily_failed, guessed_track, no_store, store_failed},
    store::PlayerId,
};

/// How many of a player's latest songs are remembered, so that the draw can
/// avoid them. Against a pool of hundreds that is long enough for a song not
/// to come back in one sitting, and short enough to keep the stored record
/// small.
pub const RECENT_SONGS: usize = 50;

// --- the state --------------------------------------------------------------------

/// The next song was asked for while the current one is still being played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the song is not finished")]
pub struct Unfinished;

/// A stored random game that no sequence of sessions, songs and moves can
/// produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("impossible random game: {0}")]
pub struct InvalidRandom(&'static str);

/// One player's random game: the song they are on, the score of their
/// session, and what is remembered between sessions. It is what the store
/// keeps per player; as JSON it reads
///
/// ```json
/// {"round":12,"track_id":3135556,"game":{"day":"2026-10-01","attempts":[{"kind":"skip"}],"status":"playing"},"run":2,"played":3,"won":2,"best_run":5,"recent":[916424,3135556]}
/// ```
///
/// Deserializing checks that the parts agree (see [`InvalidRandom`]), so the
/// methods can rely on it. Treat a state that fails to deserialize like one
/// that was never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "StoredRandom")]
pub struct RandomGame {
    /// Goes up by one with every song drawn for this player, across
    /// sessions; 1 for the first. It names the song a move is for.
    round: u64,
    /// The song being played: a Deezer track ID.
    track_id: u64,
    /// The tries used on it and whether it is won or lost. Its day is the
    /// server's day when the song was drawn; nothing depends on it.
    game: GameState,
    /// Wins in a row in this session. A loss sets it to 0.
    run: u32,
    /// Songs finished in this session, won or lost. A song that was
    /// abandoned for a new session counts for nothing.
    played: u32,
    /// How many of those were won.
    won: u32,
    /// The longest run this player ever had, in any session.
    best_run: u32,
    /// The tracks drawn for this player lately, oldest first and each one
    /// once, the current one last; at most [`RECENT_SONGS`].
    recent: Vec<u64>,
}

/// [`RandomGame`] as read from where it was stored, before its invariants
/// are checked.
#[derive(Deserialize)]
struct StoredRandom {
    round: u64,
    track_id: u64,
    game: GameState,
    run: u32,
    played: u32,
    won: u32,
    best_run: u32,
    recent: Vec<u64>,
}

impl TryFrom<StoredRandom> for RandomGame {
    type Error = InvalidRandom;

    fn try_from(stored: StoredRandom) -> Result<Self, Self::Error> {
        let check =
            |holds: bool, what: &'static str| holds.then_some(()).ok_or(InvalidRandom(what));
        check(stored.round >= 1, "the first song is round 1")?;
        check(
            stored.recent.last() == Some(&stored.track_id),
            "the song being played is the latest of the recent ones",
        )?;
        check(
            stored.recent.len() <= RECENT_SONGS,
            "more recent songs than are kept",
        )?;
        check(stored.won <= stored.played, "more songs won than played")?;
        check(stored.run <= stored.won, "a run longer than the wins")?;
        check(
            stored.run <= stored.best_run,
            "a run longer than the longest",
        )?;
        check(
            u64::from(stored.played) <= stored.round,
            "more songs played than drawn",
        )?;
        check(
            match stored.game.status() {
                Status::Playing => true,
                // The win that ended this song is the latest of the run.
                Status::Won => stored.run >= 1,
                Status::Lost => stored.run == 0 && stored.played >= 1,
            },
            "a score that does not include the song that is over",
        )?;
        Ok(Self {
            round: stored.round,
            track_id: stored.track_id,
            game: stored.game,
            run: stored.run,
            played: stored.played,
            won: stored.won,
            best_run: stored.best_run,
            recent: stored.recent,
        })
    }
}

/// `recent` with `track_id` as its latest song: moved to the end if it was
/// there, and the oldest ones dropped beyond [`RECENT_SONGS`].
fn remembering(recent: &[u64], track_id: u64) -> Vec<u64> {
    let mut recent: Vec<u64> = recent
        .iter()
        .copied()
        .filter(|played| *played != track_id)
        .collect();
    recent.push(track_id);
    let excess = recent.len().saturating_sub(RECENT_SONGS);
    recent.drain(..excess);
    recent
}

impl RandomGame {
    /// A new session on the song `track_id`, drawn on `day`: no tries used,
    /// no run, nothing played. `previous` is the player's game until now, if
    /// they had one, in whatever state: its longest run and its recent songs
    /// are carried over, and the round goes on counting from it.
    pub fn start(previous: Option<&RandomGame>, track_id: u64, day: Date) -> Self {
        Self {
            round: previous.map_or(1, |previous| previous.round.saturating_add(1)),
            track_id,
            game: GameState::new(day),
            run: 0,
            played: 0,
            won: 0,
            best_run: previous.map_or(0, |previous| previous.best_run),
            recent: remembering(
                previous.map_or(&[], |previous| previous.recent.as_slice()),
                track_id,
            ),
        }
    }

    /// The same session on its next song, `track_id`, drawn on `day`. Only a
    /// song that is won or lost can be left behind.
    pub fn next(&self, track_id: u64, day: Date) -> Result<Self, Unfinished> {
        if !self.game.is_finished() {
            return Err(Unfinished);
        }
        Ok(Self {
            round: self.round.saturating_add(1),
            track_id,
            game: GameState::new(day),
            run: self.run,
            played: self.played,
            won: self.won,
            best_run: self.best_run,
            recent: remembering(&self.recent, track_id),
        })
    }

    /// The number of the song being played; see the field.
    pub fn round(&self) -> u64 {
        self.round
    }

    /// The song being played. It identifies the answer.
    pub fn track_id(&self) -> u64 {
        self.track_id
    }

    /// The tries used on the song and how it stands.
    pub fn game(&self) -> &GameState {
        &self.game
    }

    /// Wins in a row in this session, the song just won included.
    pub fn run(&self) -> u32 {
        self.run
    }

    /// Songs finished in this session.
    pub fn played(&self) -> u32 {
        self.played
    }

    /// Songs won in this session.
    pub fn won(&self) -> u32 {
        self.won
    }

    /// The longest run the player ever had; never less than [`run`](Self::run).
    pub fn best_run(&self) -> u32 {
        self.best_run
    }

    /// The tracks drawn for this player lately, oldest first, the one being
    /// played last.
    pub fn recent(&self) -> &[u64] {
        &self.recent
    }

    /// Gives up the current turn; see [`GameState::skip`]. A skip that loses
    /// the song ends the run.
    pub fn skip(&mut self) -> Result<Status, GameError> {
        let status = self.game.skip()?;
        self.settle(status);
        Ok(status)
    }

    /// Plays `guessed` against `answer`, the song of [`track_id`](Self::track_id);
    /// see [`GameState::guess`]. A win adds to the run, a loss ends it.
    pub fn guess(&mut self, answer: &TrackMeta, guessed: &TrackMeta) -> Result<Status, GameError> {
        let status = self.game.guess(answer, guessed)?;
        self.settle(status);
        Ok(status)
    }

    /// Takes the song into the score if the move that led to `status` ended
    /// it. A finished song refuses further moves, so each is counted once.
    fn settle(&mut self, status: Status) {
        match status {
            Status::Playing => {}
            Status::Won => {
                self.played = self.played.saturating_add(1);
                self.won = self.won.saturating_add(1);
                self.run = self.run.saturating_add(1);
                self.best_run = self.best_run.max(self.run);
            }
            Status::Lost => {
                self.played = self.played.saturating_add(1);
                self.run = 0;
            }
        }
    }
}

// --- the routes -------------------------------------------------------------------

/// The routes of random mode, all under `/api/random`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/random", get(show))
        .route("/api/random/start", post(start))
        .route("/api/random/audio", get(audio))
        .route("/api/random/guess", post(guess))
        .route("/api/random/next", post(next))
}

/// The body of `GET /api/random` and of every successful `POST` under it.
/// The fields a daily game has too (`ladder`, `attempts`, `status`,
/// `clipSeconds`, `answer`) mean what they mean there.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RandomView<'a> {
    /// Names the song for the moves that follow. A counter, not the track.
    round: u64,
    /// Clip length of each turn, in seconds.
    ladder: [f64; MAX_ATTEMPTS],
    attempts: &'a [Attempt],
    status: Status,
    /// The clip length unlocked right now, in seconds.
    clip_seconds: f64,
    /// `null` until the song is over.
    answer: Option<&'a Answer>,
    /// Wins in a row in this session.
    run: u32,
    /// The longest run the player ever had.
    best_run: u32,
    /// Songs finished in this session.
    played: u32,
    /// Songs won in this session.
    won: u32,
}

impl<'a> RandomView<'a> {
    /// What the player may see of `game`, whose song is `song`. This is the
    /// only place random mode hands out an answer, and only for a song that
    /// is over.
    fn new(game: &'a RandomGame, song: &'a Song) -> Self {
        let state = game.game();
        Self {
            round: game.round(),
            ladder: game::ladder_seconds(),
            attempts: state.attempts(),
            status: state.status(),
            clip_seconds: f64::from(state.unlocked_ms()) / 1000.0,
            answer: state.is_finished().then_some(&song.answer),
            run: game.run(),
            best_run: game.best_run(),
            played: game.played(),
            won: game.won(),
        }
    }
}

/// The state of the random game as a response, which also sets the player
/// cookie again so that it lasts from this visit.
fn game_response(
    jar: PrivateCookieJar,
    headers: &HeaderMap,
    player: &PlayerId,
    game: &RandomGame,
    song: &Song,
) -> Response {
    let view = RandomView::new(game, song);
    let jar = player::remember(jar, player, headers);
    (jar, no_store(), Json(view)).into_response()
}

/// The random game stored for `player`. None is an answer the client acts
/// on: it starts a session.
async fn stored_game(app: &AppState, player: &PlayerId) -> Result<RandomGame, ApiError> {
    app.store()
        .random_game(player)
        .await
        .map_err(store_failed)?
        .ok_or(ApiError::NoGame)
}

/// The song `game` is played against, loaded.
async fn song_of(app: &AppState, game: &RandomGame) -> Result<Arc<Song>, ApiError> {
    app.daily()
        .load_song(game.track_id())
        .await
        .map_err(daily_failed)
}

/// Checks that a request made for the song `round` is about the song `game`
/// is on.
fn same_round(game: &RandomGame, round: u64) -> Result<(), ApiError> {
    if game.round() == round {
        Ok(())
    } else {
        Err(ApiError::Moved)
    }
}

// --- GET /api/random --------------------------------------------------------------

/// The game the player is in. A look: it stores nothing and issues no ID, so
/// a browser the server has not seen, or one that has never started a
/// session, is told there is no game and starts one.
///
/// The song is that of the record the view is built from, so the response is
/// about one song even if another tab moves on in the meantime.
async fn show(
    State(app): State<AppState>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let player = player::known_player(&jar).ok_or(ApiError::NoGame)?;
    let game = stored_game(&app, &player).await?;
    let song = song_of(&app, &game).await?;
    Ok(game_response(jar, &headers, &player, &game, &song))
}

// --- GET /api/random/audio --------------------------------------------------------

/// The clip the player has unlocked of their random song, cut like a daily
/// clip: the leading frames and no tags. No cookie is set, as on the daily
/// audio route, and there is no clip without a game: random mode has no song
/// that everyone shares.
async fn audio(State(app): State<AppState>, jar: PrivateCookieJar) -> Result<Response, ApiError> {
    let player = player::known_player(&jar).ok_or(ApiError::NoGame)?;
    let game = stored_game(&app, &player).await?;
    let song = song_of(&app, &game).await?;
    let clip = song.mp3.prefix(game.game().unlocked_ms());
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static("audio/mpeg")),
        (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        (header::CONTENT_LENGTH, HeaderValue::from(clip.len())),
    ];
    Ok((headers, clip).into_response())
}

// --- POST /api/random/start -------------------------------------------------------

/// A new session: a song is drawn, and the run and the totals start at zero.
/// Whatever song the player was on is abandoned, finished or not; the
/// longest run and the recent songs stay. The body is not looked at.
///
/// This is the one route of random mode that works without a cookie: it is
/// the first thing a new player's page sends.
async fn start(
    State(app): State<AppState>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let today = app.today().await?;
    let known = player::known_player(&jar);
    let recent = match &known {
        Some(player) => app
            .store()
            .random_game(player)
            .await
            .map_err(store_failed)?
            .map(|game| game.recent().to_vec())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    // The slow part, before the player's turn is taken: the draw loads the
    // song, which can be a download.
    let (track_id, song) = app
        .daily()
        .random_song(today, &recent)
        .await
        .map_err(daily_failed)?;
    let player = known.unwrap_or_else(PlayerId::generate);

    let game = {
        let _turn = app.move_locks().lock(&player).await;
        // Read again under the lock: a move in another tab may have ended a
        // song since, and its run must not be lost to this save.
        let previous = app
            .store()
            .random_game(&player)
            .await
            .map_err(store_failed)?;
        let game = RandomGame::start(previous.as_ref(), track_id, today);
        app.store()
            .save_random_game(&player, &game)
            .await
            .map_err(store_failed)?;
        game
    };
    Ok(game_response(jar, &headers, &player, &game, &song))
}

// --- POST /api/random/guess -------------------------------------------------------

/// The body of a move: `{ "round": 3, "trackId": 123 }` or
/// `{ "round": 3, "skip": true }`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveRequest {
    round: Option<u64>,
    track_id: Option<u64>,
    skip: Option<bool>,
}

/// What a valid [`MoveRequest`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    Skip,
    Guess(u64),
}

impl MoveRequest {
    /// The round and the move, or `None` when the body names no round, or
    /// both or neither of a track and a skip.
    fn into_move(self) -> Option<(u64, Move)> {
        let chosen = match (self.track_id, self.skip.unwrap_or(false)) {
            (Some(track_id), false) => Move::Guess(track_id),
            (None, true) => Move::Skip,
            _ => return None,
        };
        Some((self.round?, chosen))
    }
}

async fn guess(
    State(app): State<AppState>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
    body: Result<Json<MoveRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (round, chosen) = body
        .ok()
        .and_then(|Json(request)| request.into_move())
        .ok_or(ApiError::BadRequest(
            "Send {\"round\": <number>} with either \"trackId\": <number> or \"skip\": true, as JSON.",
        ))?;

    let player = player::known_player(&jar).ok_or(ApiError::NoGame)?;
    // A first look, before anything is looked up: a move for another song or
    // on a finished one costs Deezer nothing. The checks that count are made
    // under the lock below.
    let planned = stored_game(&app, &player).await?;
    same_round(&planned, round)?;
    if planned.game().is_finished() {
        return Err(ApiError::SongOver);
    }

    // Everything slow happens here, before the player's turn is taken: the
    // song (from the disk, after a restart) and what the guessed track is.
    let song = song_of(&app, &planned).await?;
    let guessed = match chosen {
        Move::Skip => None,
        Move::Guess(track_id) => Some(guessed_track(&app, track_id).await?),
    };

    // The move itself: read the game, apply the move, store the result, as
    // one critical section per player (see `MoveLocks`). Moves sent at once
    // are made one after the other, each on the game the one before it left.
    let game = {
        let _turn = app.move_locks().lock(&player).await;
        let mut game = stored_game(&app, &player).await?;
        // Another tab may have moved on to the next song, or started over,
        // since the look above. The song fetched there is the one of that
        // round, so the round still being the same is what makes it the
        // song this move is judged against.
        same_round(&game, round)?;
        let moved = match &guessed {
            None => game.skip(),
            Some(guessed) => game.guess(&song.meta, guessed),
        };
        moved.map_err(|GameError::Finished| ApiError::SongOver)?;
        app.store()
            .save_random_game(&player, &game)
            .await
            .map_err(store_failed)?;
        game
    };
    Ok(game_response(jar, &headers, &player, &game, &song))
}

// --- POST /api/random/next --------------------------------------------------------

/// The body of a "next song": `{ "round": 3 }`, the song being left behind.
#[derive(Debug, Deserialize)]
struct NextRequest {
    round: Option<u64>,
}

/// The next song of the session, once the current one is won or lost. The
/// run and the totals go on.
async fn next(
    State(app): State<AppState>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
    body: Result<Json<NextRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let round = body
        .ok()
        .and_then(|Json(request)| request.round)
        .ok_or(ApiError::BadRequest("Send {\"round\": <number>} as JSON."))?;

    let player = player::known_player(&jar).ok_or(ApiError::NoGame)?;
    let today = app.today().await?;
    let planned = stored_game(&app, &player).await?;
    same_round(&planned, round)?;
    if !planned.game().is_finished() {
        return Err(ApiError::Unfinished);
    }

    // The slow part, before the player's turn is taken.
    let (track_id, song) = app
        .daily()
        .random_song(today, planned.recent())
        .await
        .map_err(daily_failed)?;

    let game = {
        let _turn = app.move_locks().lock(&player).await;
        let current = stored_game(&app, &player).await?;
        // Two requests for the song after the same one (a double tap, two
        // tabs): the first moves on, and the second finds another round.
        same_round(&current, round)?;
        let game = current
            .next(track_id, today)
            .map_err(|Unfinished| ApiError::Unfinished)?;
        app.store()
            .save_random_game(&player, &game)
            .await
            .map_err(store_failed)?;
        game
    };
    Ok(game_response(jar, &headers, &player, &game, &song))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        game::{FULL_CLIP_MS, LADDER_MS},
        mp3::Mp3,
        store::{Genre, MemoryStore, NewSong, Pick, Section},
        testutil::{
            BrokenStore, Harness, ID3_MARKER, MockTrack, PREVIEW_FRAMES, Player, Preview, Reply,
            TODAY, Unhurried, synthetic_mp3,
        },
    };
    use axum::http::StatusCode;
    use jiff::civil::date;
    use serde_json::{Value, json};

    // --- the state ------------------------------------------------------------------

    const DAY: Date = date(2026, 10, 1);

    fn the_song() -> TrackMeta {
        TrackMeta::new("The Song", "", "Someone")
    }

    fn another_song() -> TrackMeta {
        TrackMeta::new("Under Pressure", "", "Queen")
    }

    /// Wins the song `game` is on.
    fn win(game: &mut RandomGame) {
        assert_eq!(game.guess(&the_song(), &the_song()), Ok(Status::Won));
    }

    /// Loses the song `game` is on, with as many skips as it takes.
    fn lose(game: &mut RandomGame) {
        while game.skip().unwrap() == Status::Playing {}
        assert_eq!(game.game().status(), Status::Lost);
    }

    #[test]
    fn a_first_session_starts_at_round_one_with_nothing_on_record() {
        let game = RandomGame::start(None, 7, DAY);
        assert_eq!(game.round(), 1);
        assert_eq!(game.track_id(), 7);
        assert_eq!(game.game(), &GameState::new(DAY));
        assert_eq!((game.run(), game.played(), game.won()), (0, 0, 0));
        assert_eq!(game.best_run(), 0);
        assert_eq!(game.recent(), [7]);
    }

    #[test]
    fn a_win_adds_to_the_run_and_a_loss_ends_it_but_not_the_best() {
        let mut game = RandomGame::start(None, 1, DAY);
        // Misses on the way change nothing of the score.
        assert_eq!(game.skip(), Ok(Status::Playing));
        assert_eq!(
            game.guess(&the_song(), &another_song()),
            Ok(Status::Playing)
        );
        assert_eq!((game.run(), game.played(), game.won()), (0, 0, 0));
        assert_eq!(game.game().attempts().len(), 2);

        win(&mut game);
        assert_eq!((game.run(), game.played(), game.won()), (1, 1, 1));
        assert_eq!(game.best_run(), 1);

        let mut game = game.next(2, DAY).unwrap();
        assert_eq!(game.round(), 2);
        assert_eq!(game.game(), &GameState::new(DAY));
        // The score goes on with the session.
        assert_eq!((game.run(), game.played(), game.won()), (1, 1, 1));
        win(&mut game);
        let mut game = game.next(3, DAY).unwrap();
        win(&mut game);
        assert_eq!((game.run(), game.played(), game.won()), (3, 3, 3));
        assert_eq!(game.best_run(), 3);

        let mut game = game.next(4, DAY).unwrap();
        lose(&mut game);
        assert_eq!((game.run(), game.played(), game.won()), (0, 4, 3));
        assert_eq!(game.best_run(), 3);

        // A shorter run afterwards does not replace the longest.
        let mut game = game.next(5, DAY).unwrap();
        win(&mut game);
        assert_eq!((game.run(), game.played(), game.won()), (1, 5, 4));
        assert_eq!(game.best_run(), 3);
        assert_eq!(game.round(), 5);
        assert_eq!(game.recent(), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_finished_song_takes_no_more_moves_and_is_counted_once() {
        let mut won = RandomGame::start(None, 1, DAY);
        win(&mut won);
        let before = won.clone();
        assert_eq!(won.skip(), Err(GameError::Finished));
        assert_eq!(
            won.guess(&the_song(), &the_song()),
            Err(GameError::Finished)
        );
        assert_eq!(won, before);

        let mut lost = RandomGame::start(None, 1, DAY);
        lose(&mut lost);
        let before = lost.clone();
        assert_eq!(lost.skip(), Err(GameError::Finished));
        assert_eq!(
            lost.guess(&the_song(), &the_song()),
            Err(GameError::Finished)
        );
        assert_eq!(lost, before);
        assert_eq!((lost.run(), lost.played(), lost.won()), (0, 1, 0));
    }

    #[test]
    fn the_next_song_needs_the_current_one_to_be_over() {
        let mut game = RandomGame::start(None, 1, DAY);
        assert_eq!(game.next(2, DAY), Err(Unfinished));
        game.skip().unwrap();
        assert_eq!(game.next(2, DAY), Err(Unfinished));
        lose(&mut game);
        assert!(game.next(2, DAY).is_ok());
    }

    #[test]
    fn a_new_session_zeroes_the_score_and_keeps_the_best_run_the_round_and_the_recent_songs() {
        let mut game = RandomGame::start(None, 1, DAY);
        win(&mut game);
        let mut game = game.next(2, DAY).unwrap();
        win(&mut game);
        let mut game = game.next(3, DAY).unwrap();
        game.skip().unwrap();

        // In the middle of a song: it is abandoned and counts for nothing.
        let later = date(2026, 10, 9);
        let fresh = RandomGame::start(Some(&game), 4, later);
        assert_eq!(fresh.round(), 4);
        assert_eq!(fresh.track_id(), 4);
        assert_eq!(fresh.game(), &GameState::new(later));
        assert_eq!((fresh.run(), fresh.played(), fresh.won()), (0, 0, 0));
        assert_eq!(fresh.best_run(), 2);
        assert_eq!(fresh.recent(), [1, 2, 3, 4]);

        // And after a finished one just the same.
        let mut over = game.clone();
        lose(&mut over);
        let fresh = RandomGame::start(Some(&over), 4, later);
        assert_eq!((fresh.run(), fresh.played(), fresh.won()), (0, 0, 0));
        assert_eq!(fresh.best_run(), 2);
        assert_eq!(fresh.round(), 4);
    }

    #[test]
    fn only_so_many_recent_songs_are_kept_each_once_and_the_current_one_last() {
        let mut game = RandomGame::start(None, 1, DAY);
        for track_id in 2..=RECENT_SONGS as u64 + 10 {
            lose(&mut game);
            game = game.next(track_id, DAY).unwrap();
        }
        let expected: Vec<u64> = (11..=RECENT_SONGS as u64 + 10).collect();
        assert_eq!(game.recent(), expected);
        assert_eq!(game.recent().len(), RECENT_SONGS);

        // A song that comes back moves to the end instead of being there twice.
        lose(&mut game);
        let game = game.next(20, DAY).unwrap();
        assert_eq!(game.recent().len(), RECENT_SONGS);
        assert_eq!(game.recent().last(), Some(&20));
        assert_eq!(
            game.recent().iter().filter(|played| **played == 20).count(),
            1
        );
        // The same in a new session, the same song included.
        let again = RandomGame::start(Some(&game), 20, DAY);
        assert_eq!(again.recent(), game.recent());
        let other = RandomGame::start(Some(&game), 12, DAY);
        assert_eq!(other.recent().len(), RECENT_SONGS);
        assert_eq!(other.recent().last(), Some(&12));
    }

    #[test]
    fn a_random_game_reads_back_from_its_json_as_it_was() {
        let mut game = RandomGame::start(None, 916_424, DAY);
        win(&mut game);
        let mut game = game.next(3_135_556, DAY).unwrap();
        game.skip().unwrap();
        game.guess(&the_song(), &another_song()).unwrap();

        let json = serde_json::to_value(&game).unwrap();
        assert_eq!(
            json,
            json!({
                "round": 2,
                "track_id": 3_135_556,
                "game": {
                    "day": "2026-10-01",
                    "attempts": [
                        { "kind": "skip" },
                        { "kind": "wrong", "title": "Under Pressure", "artist": "Queen" },
                    ],
                    "status": "playing",
                },
                "run": 1,
                "played": 1,
                "won": 1,
                "best_run": 1,
                "recent": [916_424, 3_135_556],
            })
        );
        assert_eq!(serde_json::from_value::<RandomGame>(json).unwrap(), game);

        // Every state a session passes through reads back too.
        let mut game = RandomGame::start(None, 1, DAY);
        for track_id in 2..12 {
            if track_id % 3 == 0 {
                lose(&mut game);
            } else {
                win(&mut game);
            }
            let text = serde_json::to_string(&game).unwrap();
            assert_eq!(serde_json::from_str::<RandomGame>(&text).unwrap(), game);
            game = if track_id % 5 == 0 {
                RandomGame::start(Some(&game), track_id, DAY)
            } else {
                game.next(track_id, DAY).unwrap()
            };
            let text = serde_json::to_string(&game).unwrap();
            assert_eq!(serde_json::from_str::<RandomGame>(&text).unwrap(), game);
        }
    }

    #[test]
    fn a_stored_random_game_that_cannot_be_is_refused() {
        let good = json!({
            "round": 5,
            "track_id": 7,
            "game": { "day": "2026-10-01", "attempts": [], "status": "playing" },
            "run": 1,
            "played": 3,
            "won": 2,
            "best_run": 4,
            "recent": [9, 8, 7],
        });
        assert!(serde_json::from_value::<RandomGame>(good.clone()).is_ok());

        let with = |field: &str, value: Value| {
            let mut state = good.clone();
            state[field] = value;
            state
        };
        let won = json!({ "day": "2026-10-01", "attempts": [], "status": "won" });
        let lost = json!({
            "day": "2026-10-01",
            "attempts": vec![json!({ "kind": "skip" }); 7],
            "status": "lost",
        });
        // The good state with a finished song is still good...
        assert!(serde_json::from_value::<RandomGame>(with("game", won.clone())).is_ok());
        let mut after_a_loss = with("game", lost.clone());
        after_a_loss["run"] = json!(0);
        assert!(serde_json::from_value::<RandomGame>(after_a_loss).is_ok());

        // ...and these are not states at all.
        let impossible = [
            ("round 0", with("round", json!(0))),
            (
                "another song than the latest",
                with("recent", json!([7, 8])),
            ),
            ("no recent song", with("recent", json!([]))),
            (
                "too many recent songs",
                with(
                    "recent",
                    json!(
                        (0..=RECENT_SONGS as u64)
                            .rev()
                            .map(|n| n + 7)
                            .collect::<Vec<_>>()
                    ),
                ),
            ),
            ("more won than played", with("won", json!(4))),
            ("a run longer than the wins", with("run", json!(3))),
            ("a run longer than the best", with("best_run", json!(0))),
            ("more played than drawn", with("round", json!(2))),
            ("a lost song with a run", with("game", lost)),
            ("a won song without a run", {
                let mut state = with("game", won);
                state["run"] = json!(0);
                state
            }),
            (
                "a daily game that cannot be",
                with(
                    "game",
                    json!({ "day": "2026-10-01", "attempts": [], "status": "lost" }),
                ),
            ),
            ("a negative count", with("played", json!(-1))),
            ("a missing field", {
                let mut state = good.clone();
                state.as_object_mut().unwrap().remove("best_run");
                state
            }),
        ];
        for (what, state) in impossible {
            assert!(
                serde_json::from_value::<RandomGame>(state.clone()).is_err(),
                "{what}: {state}"
            );
        }
    }

    // --- the routes -----------------------------------------------------------------

    const ALPHA: u64 = 700_000_001;
    const BETA: u64 = 700_000_002;
    const GAMMA: u64 = 700_000_003;
    const DELTA: u64 = 700_000_004;
    const EPSILON: u64 = 700_000_005;
    /// A track Deezer knows but will not play: `readable: false`, no preview.
    const WITHDRAWN: u64 = 700_000_009;
    /// Tracks nobody plays here, for wrong guesses.
    const QUEEN: u64 = 10;
    const FILLER: u64 = 100;
    const UNKNOWN_ID: u64 = 555;

    const POP: Section = Section::Genre(Genre::Pop);
    const ROCK: Section = Section::Genre(Genre::Rock);
    const HIP_HOP: Section = Section::Genre(Genre::HipHop);

    /// The songs the pools are made of: ID, title, artist, album. The IDs
    /// are long so that they cannot turn up by chance in a `Content-Length`
    /// or in the cookie's ciphertext, and no word is in two of them.
    const SONGS: [(u64, &str, &str, &str); 5] = [
        (ALPHA, "Amber Lanterns", "The Alphas", "First Light"),
        (BETA, "Brass Meridian", "The Betas", "Second Wind"),
        (GAMMA, "Cobalt Harbour", "The Gammas", "Third Rail"),
        (DELTA, "Dune Parade", "The Deltas", "Fourth Wall"),
        (EPSILON, "Ember Atlas", "The Epsilons", "Fifth Season"),
    ];

    fn tracks() -> Vec<MockTrack> {
        let mut tracks: Vec<MockTrack> = SONGS
            .iter()
            .map(|(id, title, artist, album)| MockTrack::new(*id, title, artist).album(album))
            .collect();
        tracks.push(MockTrack::new(WITHDRAWN, "Vanished Hit", "The Ghosts").preview(Preview::None));
        tracks.push(MockTrack::new(QUEEN, "Under Pressure", "Queen").album("Hot Space"));
        for n in 0..4 {
            tracks.push(MockTrack::new(
                FILLER + n,
                &format!("Filler Song {n}"),
                "Padding",
            ));
        }
        tracks
    }

    /// Everything that identifies one of [`SONGS`]: none of it may be seen
    /// while that song is being played.
    fn secrets_of(track_id: u64) -> Vec<String> {
        let (id, title, artist, album) = SONGS
            .iter()
            .find(|song| song.0 == track_id)
            .unwrap_or_else(|| panic!("{track_id} is none of the songs"));
        vec![
            id.to_string(),
            (*title).to_owned(),
            (*artist).to_owned(),
            (*album).to_owned(),
            format!("cover{id}"),
        ]
    }

    /// The anti-leak rule, as a check on one response: headers, cookie and
    /// body together say nothing of the song `track_id`.
    fn assert_hides(reply: &Reply, track_id: u64) {
        let secrets = secrets_of(track_id);
        let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
        reply.assert_lacks(&secrets);
    }

    /// A server whose pool is `pool`, untagged, and where General plays the
    /// first of them today: random mode draws from the rest.
    async fn start(pool: &[u64]) -> Harness {
        over(pool, Arc::new(MemoryStore::new())).await
    }

    /// The same over a store of the test's choosing.
    async fn over(pool: &[u64], store: Arc<dyn crate::store::Store>) -> Harness {
        let pool: Vec<(u64, &[Genre])> = pool.iter().map(|id| (*id, &[] as &[Genre])).collect();
        let harness = Harness::over(tracks(), &pool, store).await;
        if let Some((general, _)) = pool.first() {
            let pick = Pick {
                day: TODAY,
                section: Section::General,
                track_id: *general,
            };
            harness.store.save_pick(pick).await.unwrap();
        }
        harness
    }

    /// The random game the store has for the browser `player`.
    async fn stored(harness: &Harness, player: &Player) -> RandomGame {
        let id = harness
            .player_id(player)
            .expect("the browser has no cookie");
        harness
            .store
            .random_game(&id)
            .await
            .unwrap()
            .expect("the player has no random game")
    }

    /// A browser that has started a session, and the song it is on. The
    /// start's response is checked like any playing-state response.
    async fn started(harness: &Harness) -> (Player, u64) {
        let mut player = harness.player();
        let reply = player.random_start().await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        let track_id = stored(harness, &player).await.track_id();
        assert_hides(&reply, track_id);
        (player, track_id)
    }

    /// Wins the song `round` (the track `track_id`) and moves on to the
    /// next. Returns the song the player is on then.
    async fn win_and_go_on(
        harness: &Harness,
        player: &mut Player,
        round: u64,
        track_id: u64,
    ) -> u64 {
        let reply = player.random_guess(round, track_id).await;
        assert_eq!(reply.json()["status"], "won", "{}", reply.visible());
        let reply = player.random_next(round).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json()["round"], round + 1);
        let next = stored(harness, player).await.track_id();
        assert_hides(&reply, next);
        next
    }

    fn reference_mp3() -> Mp3 {
        Mp3::parse(synthetic_mp3(PREVIEW_FRAMES)).unwrap()
    }

    // --- POST /api/random/start ---------------------------------------------------

    #[tokio::test]
    async fn starting_without_a_cookie_issues_one_draws_a_song_and_shows_a_fresh_game() {
        let harness = start(&[ALPHA, BETA]).await;
        let mut player = harness.player();

        let reply = player.random_start().await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        reply.assert_no_store();
        assert_eq!(
            reply.json(),
            json!({
                "round": 1,
                "ladder": [0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0],
                "attempts": [],
                "status": "playing",
                "clipSeconds": 0.1,
                "answer": null,
                "run": 0,
                "bestRun": 0,
                "played": 0,
                "won": 0,
            })
        );

        // The browser is a player now, and the game is in the store under
        // that ID: General plays the one song, so this is the other.
        let id = harness.player_id(&player).expect("a cookie was set");
        let game = harness.store.random_game(&id).await.unwrap().unwrap();
        assert_eq!(game.track_id(), BETA);
        assert_eq!(game.round(), 1);
        assert_eq!(game.game(), &GameState::new(TODAY));
        assert_hides(&reply, BETA);
        // The cookie holds the ID and nothing else.
        assert_eq!(harness.cookie_plaintext(&player), Some(id.to_string()));
        // No daily game came of it.
        assert_eq!(
            harness.store.games(&id, Section::General).await.unwrap(),
            Vec::new()
        );
    }

    #[tokio::test]
    async fn the_body_and_the_content_type_of_a_start_are_not_looked_at() {
        let harness = start(&[ALPHA, BETA]).await;
        let mut player = harness.player();
        for (content_type, body) in [
            ("application/json", r#"{"round": 9, "skip": true}"#),
            ("text/plain", "anything at all"),
            ("application/json", "{not json"),
        ] {
            let reply = player
                .post_to("/api/random/start", content_type, body)
                .await;
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
            assert_eq!(reply.json()["status"], "playing");
            assert_hides(&reply, BETA);
        }
        // Each was a new session over the one before, for the same player.
        assert_eq!(stored(&harness, &player).await.round(), 3);
    }

    #[tokio::test]
    async fn a_new_session_zeroes_the_score_and_keeps_the_best_run() {
        let harness = start(&[ALPHA, BETA, GAMMA, DELTA]).await;
        let (mut player, first) = started(&harness).await;
        let second = win_and_go_on(&harness, &mut player, 1, first).await;
        let reply = player.random_guess(2, second).await;
        let body = reply.json();
        assert_eq!(body["run"], 2);
        assert_eq!(body["bestRun"], 2);

        // A new session: the round goes on counting, the score does not.
        let reply = player.random_start().await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        let body = reply.json();
        assert_eq!(body["round"], 3);
        assert_eq!(body["status"], "playing");
        assert_eq!(body["attempts"], json!([]));
        assert_eq!(body["answer"], Value::Null);
        assert_eq!(body["run"], 0);
        assert_eq!(body["played"], 0);
        assert_eq!(body["won"], 0);
        assert_eq!(body["bestRun"], 2);
        // The songs of the session before are still the recent ones: the
        // third of the three is drawn.
        let third = stored(&harness, &player).await.track_id();
        assert!(third != first && third != second);
        assert_hides(&reply, third);

        // Starting over in the middle of a song abandons it: it is neither
        // played nor lost, and the song after it is the one played longest
        // ago.
        assert_eq!(player.random_skip(3).await.status, StatusCode::OK);
        let reply = player.random_start().await;
        let body = reply.json();
        assert_eq!(body["round"], 4);
        assert_eq!(body["attempts"], json!([]));
        assert_eq!(body["clipSeconds"], 0.1);
        assert_eq!(body["played"], 0);
        assert_eq!(body["bestRun"], 2);
        assert_eq!(stored(&harness, &player).await.track_id(), first);
        assert_hides(&reply, first);
    }

    #[tokio::test]
    async fn todays_daily_songs_are_never_drawn_and_their_picks_are_made_first() {
        // One song for each genre and two for General to choose from.
        let pool: [(u64, &[Genre]); 5] = [
            (ALPHA, &[Genre::Pop]),
            (BETA, &[Genre::Rock]),
            (GAMMA, &[Genre::HipHop]),
            (DELTA, &[]),
            (EPSILON, &[]),
        ];
        let harness = Harness::with_pool(tracks(), &pool).await;
        // Nobody has asked for a daily game: nothing is picked yet.
        assert_eq!(harness.pick(TODAY, Section::General).await, None);

        let (mut player, first) = started(&harness).await;

        assert_eq!(harness.pick(TODAY, POP).await, Some(ALPHA));
        assert_eq!(harness.pick(TODAY, ROCK).await, Some(BETA));
        assert_eq!(harness.pick(TODAY, HIP_HOP).await, Some(GAMMA));
        let general = harness.pick(TODAY, Section::General).await.unwrap();
        // The one song no section plays today, however often it is drawn.
        assert_eq!(first, DELTA + EPSILON - general);
        for round in 1..=6 {
            let next = win_and_go_on(&harness, &mut player, round, first).await;
            assert_eq!(next, first);
        }
        for _ in 0..4 {
            assert_eq!(player.random_start().await.status, StatusCode::OK);
            assert_eq!(stored(&harness, &player).await.track_id(), first);
        }
    }

    #[tokio::test]
    async fn with_nothing_to_draw_there_is_no_song_and_no_game() {
        // One song, and General plays it.
        let harness = start(&[ALPHA]).await;
        let mut player = harness.player();
        let reply = player.random_start().await;
        reply.assert_error(StatusCode::NOT_FOUND, "no_song");
        assert_hides(&reply, ALPHA);
        // No game, and no cookie for a game that is not there.
        assert!(reply.headers.get(header::SET_COOKIE).is_none());
        assert!(player.cookie.is_none());

        // An empty pool.
        let harness = start(&[]).await;
        harness
            .player()
            .random_start()
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_song");
    }

    #[tokio::test]
    async fn a_drawn_track_without_a_preview_is_marked_and_another_is_drawn() {
        let harness = start(&[ALPHA, BETA]).await;
        let (mut player, first) = started(&harness).await;
        assert_eq!(first, BETA);

        // A withdrawn track joins the pool. It is the one song that is not
        // recent, so the next session draws it first, finds it has no
        // preview, marks it and falls back on the song it knows.
        let withdrawn = NewSong {
            track_id: WITHDRAWN,
            title: "Vanished Hit".to_owned(),
            title_short: "Vanished Hit".to_owned(),
            artist: "The Ghosts".to_owned(),
            album: "LP".to_owned(),
        };
        harness
            .store
            .add_song(withdrawn, Default::default())
            .await
            .unwrap();
        let reply = player.random_start().await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json()["round"], 2);
        assert_eq!(stored(&harness, &player).await.track_id(), BETA);
        assert_hides(&reply, BETA);
        let marked = harness.store.song(WITHDRAWN).await.unwrap().unwrap();
        assert_eq!(marked.preview_failed_on, Some(TODAY));
        assert_eq!(
            harness
                .store
                .song(BETA)
                .await
                .unwrap()
                .unwrap()
                .preview_failed_on,
            None
        );

        // Where it is all there is, there is no song: marked, it is out of
        // the draw for the day.
        harness.store.remove_song(BETA).await.unwrap();
        let mut newcomer = harness.player();
        newcomer
            .random_start()
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_song");
    }

    // --- GET /api/random ------------------------------------------------------------

    #[tokio::test]
    async fn without_a_game_there_is_none_to_look_at_listen_to_or_move_in() {
        let harness = start(&[ALPHA, BETA]).await;

        // A browser the server has never seen: no game, and no cookie either.
        let mut stranger = harness.player();
        for reply in [
            stranger.random().await,
            stranger.random_audio().await,
            stranger.random_skip(1).await,
            stranger.random_guess(1, BETA).await,
            stranger.random_next(1).await,
        ] {
            reply.assert_error(StatusCode::NOT_FOUND, "no_game");
            assert!(reply.headers.get(header::SET_COOKIE).is_none());
        }
        assert!(stranger.cookie.is_none());

        // A player of the daily games who has never opened random mode.
        let mut player = harness.player();
        assert_eq!(player.get("/api/today").await.status, StatusCode::OK);
        let id = harness.player_id(&player).unwrap();
        for reply in [
            player.random().await,
            player.random_audio().await,
            player.random_skip(1).await,
            player.random_guess(1, BETA).await,
            player.random_next(1).await,
        ] {
            reply.assert_error(StatusCode::NOT_FOUND, "no_game");
        }
        // Nothing came of asking.
        assert_eq!(harness.store.random_game(&id).await.unwrap(), None);
        assert_eq!(harness.player_id(&player), Some(id));
    }

    #[tokio::test]
    async fn a_look_shows_the_game_refreshes_the_cookie_and_stores_nothing() {
        let harness = start(&[ALPHA, BETA]).await;
        let (mut player, track_id) = started(&harness).await;
        let id = harness.player_id(&player).unwrap();
        let reply = player.random_skip(1).await;
        let after_the_skip = reply.json();
        let before = stored(&harness, &player).await;

        for _ in 0..3 {
            let reply = player.random().await;
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
            reply.assert_no_store();
            assert_hides(&reply, track_id);
            // What the move answered, and the same every time.
            assert_eq!(reply.json(), after_the_skip);
            let cookie = reply.header(header::SET_COOKIE);
            assert!(cookie.contains("Max-Age=34560000"), "{cookie}");
            assert_eq!(harness.player_id(&player), Some(id.clone()));
        }
        assert_eq!(stored(&harness, &player).await, before);

        // Another browser sees none of it.
        harness
            .player()
            .random()
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_game");
    }

    #[tokio::test]
    async fn the_game_survives_a_restart_and_needs_no_deezer_for_it() {
        let mut harness = start(&[ALPHA, BETA]).await;
        let (mut player, track_id) = started(&harness).await;
        let id = harness.player_id(&player).unwrap();
        let before = player.random_skip(1).await.json();
        let clip = player.random_audio().await.body;

        harness.restart();
        harness.deezer.set_failing(true);
        let hits = harness.deezer.api_hits();
        let mut player = harness.player_with_id(&id);
        let reply = player.random().await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json(), before);
        assert_hides(&reply, track_id);
        assert_eq!(player.random_audio().await.body, clip);
        // A skip needs nothing looked up.
        let reply = player.random_skip(1).await;
        assert_eq!(reply.json()["attempts"].as_array().unwrap().len(), 2);
        assert_eq!(harness.deezer.api_hits(), hits);
    }

    // --- GET /api/random/audio ------------------------------------------------------

    #[tokio::test]
    async fn the_clip_is_the_unlocked_prefix_and_never_sets_a_cookie() {
        let harness = start(&[ALPHA, BETA]).await;
        let (mut player, track_id) = started(&harness).await;
        let id = harness.player_id(&player);
        let reference = reference_mp3();

        let clip = |reply: &Reply, unlocked_ms: u32| {
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
            assert_eq!(reply.header(header::CONTENT_TYPE), "audio/mpeg");
            reply.assert_no_store();
            assert!(reply.headers.get(header::SET_COOKIE).is_none());
            assert!(reply.headers.get(header::ACCEPT_RANGES).is_none());
            assert_eq!(reply.content_length(), reply.body.len());
            // Frames only: the tag, which could carry the title, is not sent.
            assert!(
                !reply
                    .body
                    .windows(ID3_MARKER.len())
                    .any(|window| window == ID3_MARKER)
            );
            assert_eq!(reply.body, reference.prefix(unlocked_ms));
        };

        // A fresh song: the shortest clip, whatever the query says.
        let first = player.random_audio().await;
        clip(&first, LADDER_MS[0]);
        assert!(first.body.len() < 5_000);
        assert_hides(&first, track_id);
        clip(
            &player.get("/api/random/audio?t=1-6-won").await,
            LADDER_MS[0],
        );

        // Each miss unlocks the next step, and no more.
        for (misses, unlocked) in LADDER_MS.iter().enumerate().skip(1).take(3) {
            let reply = player.random_skip(1).await;
            assert_eq!(reply.json()["attempts"].as_array().unwrap().len(), misses);
            clip(&player.random_audio().await, *unlocked);
        }

        // Once it is over, all of it.
        assert_eq!(
            player.random_guess(1, track_id).await.json()["status"],
            "won"
        );
        let whole = player.random_audio().await;
        clip(&whole, FULL_CLIP_MS);
        assert_eq!(whole.body, reference.prefix(u32::MAX));

        // The next song starts at the shortest clip again.
        assert_eq!(player.random_next(1).await.status, StatusCode::OK);
        clip(&player.random_audio().await, LADDER_MS[0]);
        // Listening never changed who the browser is.
        assert_eq!(harness.player_id(&player), id);
    }

    // --- POST /api/random/guess -----------------------------------------------------

    #[tokio::test]
    async fn a_wrong_guess_and_a_skip_each_cost_a_try_and_unlock_a_longer_clip() {
        let harness = start(&[ALPHA, BETA]).await;
        let (mut player, track_id) = started(&harness).await;

        let reply = player.random_guess(1, QUEEN).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        reply.assert_no_store();
        assert_hides(&reply, track_id);
        let body = reply.json();
        assert_eq!(body["round"], 1);
        assert_eq!(body["status"], "playing");
        assert_eq!(
            body["attempts"],
            json!([{ "kind": "wrong", "title": "Under Pressure", "artist": "Queen" }])
        );
        assert_eq!(body["clipSeconds"], 0.3);
        assert_eq!(body["answer"], Value::Null);

        let reply = player.random_skip(1).await;
        assert_hides(&reply, track_id);
        let body = reply.json();
        assert_eq!(
            body["attempts"],
            json!([
                { "kind": "wrong", "title": "Under Pressure", "artist": "Queen" },
                { "kind": "skip" },
            ])
        );
        assert_eq!(body["clipSeconds"], 1.0);
        // Misses are no part of the score.
        assert_eq!(body["run"], 0);
        assert_eq!(body["played"], 0);
        assert_eq!(body["won"], 0);
        assert_eq!(body["bestRun"], 0);

        // Another song of the pool is a wrong guess like any other, and is
        // named in the tries: it is not this song's secret.
        let reply = player.random_guess(1, ALPHA).await;
        assert_hides(&reply, track_id);
        let body = reply.json();
        assert_eq!(body["attempts"][2]["title"], "Amber Lanterns");
        assert_eq!(body["clipSeconds"], 3.0);
        assert_eq!(stored(&harness, &player).await.game().attempts().len(), 3);
    }

    #[tokio::test]
    async fn a_win_reveals_the_song_and_counts_and_the_next_song_goes_on_with_the_run() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let (mut player, first) = started(&harness).await;
        assert_eq!(player.random_skip(1).await.status, StatusCode::OK);

        let reply = player.random_guess(1, first).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        let body = reply.json();
        assert_eq!(body["round"], 1);
        assert_eq!(body["status"], "won");
        // The winning guess is no attempt.
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
        assert_eq!(body["clipSeconds"], 30.0);
        let (id, title, artist, album) = SONGS.iter().find(|song| song.0 == first).unwrap();
        assert_eq!(
            body["answer"],
            json!({
                "title": title,
                "artist": artist,
                "album": album,
                "cover": format!("https://cdn-images.dzcdn.net/images/cover/cover{id}/500x500-000000-80-0-0.jpg"),
                "link": format!("https://www.deezer.com/track/{id}"),
            })
        );
        assert_eq!(body["run"], 1);
        assert_eq!(body["bestRun"], 1);
        assert_eq!(body["played"], 1);
        assert_eq!(body["won"], 1);
        // A look shows the same finished song.
        assert_eq!(player.random().await.json(), body);

        // The next song: a fresh game, the other song, and the score as it was.
        let reply = player.random_next(1).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        reply.assert_no_store();
        let second = stored(&harness, &player).await.track_id();
        assert_eq!(second, BETA + GAMMA - first);
        assert_hides(&reply, second);
        assert_eq!(
            reply.json(),
            json!({
                "round": 2,
                "ladder": [0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0],
                "attempts": [],
                "status": "playing",
                "clipSeconds": 0.1,
                "answer": null,
                "run": 1,
                "bestRun": 1,
                "played": 1,
                "won": 1,
            })
        );
    }

    #[tokio::test]
    async fn a_loss_reveals_the_song_ends_the_run_and_keeps_the_best() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let (mut player, first) = started(&harness).await;
        let second = win_and_go_on(&harness, &mut player, 1, first).await;

        // Six misses: still playing, still hidden.
        for miss in 1..MAX_ATTEMPTS {
            let reply = if miss % 2 == 0 {
                player.random_guess(2, QUEEN).await
            } else {
                player.random_skip(2).await
            };
            let body = reply.json();
            assert_eq!(body["status"], "playing");
            assert_eq!(body["attempts"].as_array().unwrap().len(), miss);
            assert_eq!(body["run"], 1);
            assert_hides(&reply, second);
        }
        // The seventh loses, and shows what it was.
        let reply = player.random_skip(2).await;
        let body = reply.json();
        assert_eq!(body["status"], "lost");
        assert_eq!(body["attempts"].as_array().unwrap().len(), MAX_ATTEMPTS);
        assert_eq!(body["clipSeconds"], 30.0);
        let title = SONGS.iter().find(|song| song.0 == second).unwrap().1;
        assert_eq!(body["answer"]["title"], title);
        assert_eq!(body["run"], 0);
        assert_eq!(body["bestRun"], 1);
        assert_eq!(body["played"], 2);
        assert_eq!(body["won"], 1);

        // It is over: no move changes it.
        let before = stored(&harness, &player).await;
        player
            .random_skip(2)
            .await
            .assert_error(StatusCode::CONFLICT, "finished");
        player
            .random_guess(2, second)
            .await
            .assert_error(StatusCode::CONFLICT, "finished");
        assert_eq!(stored(&harness, &player).await, before);

        // And the session goes on.
        let reply = player.random_next(2).await;
        let body = reply.json();
        assert_eq!(body["round"], 3);
        assert_eq!(body["status"], "playing");
        assert_eq!(body["run"], 0);
        assert_eq!(body["bestRun"], 1);
        assert_eq!(body["played"], 2);
    }

    #[tokio::test]
    async fn another_release_of_the_song_wins_and_a_song_that_left_the_pool_is_played_to_its_end() {
        let mut tracks = tracks();
        const BETA_REMASTER: u64 = 700_000_102;
        tracks.push(
            MockTrack::new(
                BETA_REMASTER,
                "Brass Meridian (Remastered 2011)",
                "The Betas",
            )
            .title_short("Brass Meridian")
            .album("Second Wind (Deluxe)"),
        );
        let pool: [(u64, &[Genre]); 2] = [(ALPHA, &[]), (BETA, &[])];
        let harness = Harness::with_pool(tracks, &pool).await;
        let general = Pick {
            day: TODAY,
            section: Section::General,
            track_id: ALPHA,
        };
        harness.store.save_pick(general).await.unwrap();
        let (mut player, track_id) = started(&harness).await;
        assert_eq!(track_id, BETA);

        // The admin removes the song while it is being played.
        assert!(harness.store.remove_song(BETA).await.unwrap());
        let reply = player.random_skip(1).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_hides(&reply, BETA);

        let reply = player.random_guess(1, BETA_REMASTER).await;
        let body = reply.json();
        assert_eq!(body["status"], "won");
        // The answer is the song that was played, not the release guessed.
        assert_eq!(body["answer"]["title"], "Brass Meridian");
        assert_eq!(body["answer"]["album"], "Second Wind");
    }

    #[tokio::test]
    async fn a_move_that_is_not_understood_is_refused_and_uses_no_try() {
        let harness = start(&[ALPHA, BETA]).await;
        let (mut player, track_id) = started(&harness).await;
        let before = stored(&harness, &player).await;

        let bodies = [
            json!({ "skip": true }),
            json!({ "trackId": QUEEN }),
            json!({ "round": 1 }),
            json!({ "round": 1, "skip": false }),
            json!({ "round": 1, "trackId": QUEEN, "skip": true }),
            json!({ "round": "1", "skip": true }),
            json!({ "round": 1.5, "skip": true }),
            json!({ "round": -1, "skip": true }),
            json!({ "round": null, "skip": true }),
            json!({ "round": 1, "trackId": "10" }),
            json!([1, true]),
        ];
        for body in bodies {
            let reply = player.random_move(body.clone()).await;
            reply.assert_error(StatusCode::BAD_REQUEST, "bad_request");
            assert_hides(&reply, track_id);
        }
        for (content_type, body) in [
            ("application/json", "{not json"),
            ("application/json", ""),
            ("text/plain", r#"{"round": 1, "skip": true}"#),
        ] {
            player
                .post_to("/api/random/guess", content_type, body)
                .await
                .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        }
        assert_eq!(stored(&harness, &player).await, before);

        // The request is read before anyone is asked who is playing.
        harness
            .post("/api/random/guess", Some(json!({ "skip": true })))
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
    }

    #[tokio::test]
    async fn an_unknown_track_is_refused_and_uses_no_try() {
        let harness = start(&[ALPHA, BETA]).await;
        let (mut player, track_id) = started(&harness).await;
        let before = stored(&harness, &player).await;

        let reply = player.random_guess(1, UNKNOWN_ID).await;
        reply.assert_error(StatusCode::NOT_FOUND, "unknown_track");
        assert_hides(&reply, track_id);
        assert_eq!(stored(&harness, &player).await, before);
    }

    #[tokio::test]
    async fn a_deezer_failure_uses_no_try_and_moves_on_to_no_song() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let (mut player, first) = started(&harness).await;
        harness.deezer.set_failing(true);
        let before = stored(&harness, &player).await;

        // A guess whose track has to be looked up.
        let reply = player.random_guess(1, QUEEN).await;
        reply.assert_error(StatusCode::BAD_GATEWAY, "upstream");
        assert_hides(&reply, first);
        assert_eq!(stored(&harness, &player).await, before);
        // A skip needs nothing from Deezer, and neither does the right
        // answer: the song being played was looked up when it was loaded.
        assert_eq!(player.random_skip(1).await.status, StatusCode::OK);
        assert_eq!(player.random_guess(1, first).await.json()["status"], "won");

        // The next song is the one that is not recent, and nothing of it is
        // here yet: without Deezer the player stays where they are.
        let before = stored(&harness, &player).await;
        let reply = player.random_next(1).await;
        reply.assert_error(StatusCode::BAD_GATEWAY, "upstream");
        assert_hides(&reply, BETA + GAMMA - first);
        assert_eq!(stored(&harness, &player).await, before);
        // No verdict on the song came of it.
        for track_id in [BETA, GAMMA] {
            let song = harness.store.song(track_id).await.unwrap().unwrap();
            assert_eq!(song.preview_failed_on, None);
        }

        harness.deezer.set_failing(false);
        let reply = player.random_next(1).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json()["round"], 2);
    }

    // --- POST /api/random/next ------------------------------------------------------

    #[tokio::test]
    async fn the_next_song_is_refused_while_the_current_one_is_being_played() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let (mut player, track_id) = started(&harness).await;
        assert_eq!(player.random_skip(1).await.status, StatusCode::OK);
        let before = stored(&harness, &player).await;

        let reply = player.random_next(1).await;
        reply.assert_error(StatusCode::CONFLICT, "unfinished");
        assert_hides(&reply, track_id);
        assert_eq!(stored(&harness, &player).await, before);

        // A body that names no round is not a request for a next song.
        for body in [json!({}), json!({ "round": "1" }), json!({ "round": null })] {
            player
                .post_to("/api/random/next", "application/json", body.to_string())
                .await
                .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        }
        player
            .post_to("/api/random/next", "text/plain", r#"{"round": 1}"#)
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        harness
            .post("/api/random/next", None)
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        assert_eq!(stored(&harness, &player).await, before);
    }

    #[tokio::test]
    async fn a_move_or_a_next_for_another_round_is_refused_and_changes_nothing() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let (mut player, first) = started(&harness).await;
        let before = stored(&harness, &player).await;

        // Rounds that are not this song's: one to come, one that never was.
        for round in [2, 0, 99] {
            let reply = player.random_skip(round).await;
            reply.assert_error(StatusCode::CONFLICT, "changed");
            assert_hides(&reply, first);
            // Even the right answer does not count for another round.
            player
                .random_guess(round, first)
                .await
                .assert_error(StatusCode::CONFLICT, "changed");
            player
                .random_next(round)
                .await
                .assert_error(StatusCode::CONFLICT, "changed");
        }
        assert_eq!(stored(&harness, &player).await, before);

        // The page moves on in one tab...
        let second = win_and_go_on(&harness, &mut player, 1, first).await;
        let before = stored(&harness, &player).await;
        // ...and another tab still shows the first song, finished. Its "next
        // song" would skip the second, and its moves are for a song that is
        // gone: both are told to look again, not that the song is over.
        for reply in [
            player.random_next(1).await,
            player.random_skip(1).await,
            player.random_guess(1, second).await,
            player.random_guess(1, first).await,
        ] {
            reply.assert_error(StatusCode::CONFLICT, "changed");
            assert_hides(&reply, second);
        }
        assert_eq!(stored(&harness, &player).await, before);

        // A new session in one tab is the same to the other.
        assert_eq!(player.random_start().await.json()["round"], 3);
        player
            .random_skip(2)
            .await
            .assert_error(StatusCode::CONFLICT, "changed");
        assert_eq!(player.random_skip(3).await.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn recent_songs_are_not_drawn_again_until_only_they_are_left() {
        let harness = start(&[ALPHA, BETA, GAMMA, DELTA]).await;
        let (mut player, first) = started(&harness).await;
        let mut songs = vec![first];
        for round in 1..=6 {
            let track_id = *songs.last().unwrap();
            songs.push(win_and_go_on(&harness, &mut player, round, track_id).await);
        }

        // Three songs to draw from (General plays the fourth): the first
        // three rounds are the three of them, and from then on the one
        // played longest ago comes back, so the order repeats.
        let mut round: Vec<u64> = songs[..3].to_vec();
        round.sort_unstable();
        assert_eq!(round, [BETA, GAMMA, DELTA]);
        assert_eq!(songs[3..6], songs[..3]);
        assert_eq!(songs[6], songs[0]);
        assert_eq!(
            stored(&harness, &player).await.recent(),
            [songs[1], songs[2], songs[0]]
        );
    }

    // --- moves at once --------------------------------------------------------------

    /// Sends every request in `moves` to `/api/random/guess` at the same
    /// moment, each from its own task and all as the player `id`.
    async fn all_at_once(harness: &Harness, id: &PlayerId, moves: Vec<Value>) -> Vec<Reply> {
        let requests: Vec<_> = moves
            .into_iter()
            .map(|body| {
                let mut player = harness.player_with_id(id);
                tokio::spawn(async move { player.random_move(body).await })
            })
            .collect();
        let mut replies = Vec::new();
        for request in requests {
            replies.push(request.await.unwrap());
        }
        replies
    }

    #[tokio::test]
    async fn moves_sent_at_once_each_cost_a_try() {
        let store = Arc::new(Unhurried(MemoryStore::new()));
        let harness = over(&[ALPHA, BETA], store).await;
        let (player, track_id) = started(&harness).await;
        let id = harness.player_id(&player).unwrap();

        // Three wrong guesses and two skips, all read before any is written
        // if nothing keeps them apart: five tries for the price of one.
        let moves = vec![
            json!({ "round": 1, "trackId": QUEEN }),
            json!({ "round": 1, "skip": true }),
            json!({ "round": 1, "trackId": FILLER }),
            json!({ "round": 1, "trackId": FILLER + 1 }),
            json!({ "round": 1, "skip": true }),
        ];
        let replies = all_at_once(&harness, &id, moves).await;

        let mut counts: Vec<usize> = replies
            .iter()
            .map(|reply| {
                assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
                assert_hides(reply, track_id);
                reply.json()["attempts"].as_array().unwrap().len()
            })
            .collect();
        counts.sort_unstable();
        assert_eq!(counts, vec![1, 2, 3, 4, 5]);
        assert_eq!(stored(&harness, &player).await.game().attempts().len(), 5);
    }

    #[tokio::test]
    async fn more_moves_at_once_than_tries_left_lose_the_song_once() {
        let store = Arc::new(Unhurried(MemoryStore::new()));
        let harness = over(&[ALPHA, BETA], store).await;
        let (player, _) = started(&harness).await;
        let id = harness.player_id(&player).unwrap();

        let skips = vec![json!({ "round": 1, "skip": true }); 10];
        let replies = all_at_once(&harness, &id, skips).await;

        let made = replies
            .iter()
            .filter(|reply| reply.status == StatusCode::OK)
            .count();
        assert_eq!(made, MAX_ATTEMPTS);
        for reply in replies
            .iter()
            .filter(|reply| reply.status != StatusCode::OK)
        {
            reply.assert_error(StatusCode::CONFLICT, "finished");
        }
        // One song was played and lost, however many requests said so.
        let game = stored(&harness, &player).await;
        assert_eq!(game.game().status(), Status::Lost);
        assert_eq!((game.run(), game.played(), game.won()), (0, 1, 0));
    }

    #[tokio::test]
    async fn next_songs_asked_for_at_once_move_on_by_one() {
        let store = Arc::new(Unhurried(MemoryStore::new()));
        let harness = over(&[ALPHA, BETA, GAMMA], store).await;
        let (mut player, first) = started(&harness).await;
        let id = harness.player_id(&player).unwrap();
        assert_eq!(player.random_guess(1, first).await.json()["status"], "won");

        let requests: Vec<_> = (0..5)
            .map(|_| {
                let mut player = harness.player_with_id(&id);
                tokio::spawn(async move { player.random_next(1).await })
            })
            .collect();
        let mut moved_on = 0;
        for request in requests {
            let reply = request.await.unwrap();
            if reply.status == StatusCode::OK {
                assert_eq!(reply.json()["round"], 2);
                moved_on += 1;
            } else {
                reply.assert_error(StatusCode::CONFLICT, "changed");
            }
        }
        // A double tap is one song further, not two.
        assert_eq!(moved_on, 1);
        let game = stored(&harness, &player).await;
        assert_eq!(game.round(), 2);
        assert_eq!((game.run(), game.played(), game.won()), (1, 1, 1));
    }

    // --- the store and the rest of the server -----------------------------------------

    #[tokio::test]
    async fn a_store_that_fails_is_an_internal_error_that_says_nothing() {
        let harness = Harness::over(tracks(), &[], Arc::new(BrokenStore)).await;
        let id = PlayerId::generate();
        let mut player = harness.player_with_id(&id);
        for reply in [
            player.random().await,
            player.random_audio().await,
            player.random_start().await,
            player.random_skip(1).await,
            player.random_guess(1, QUEEN).await,
            player.random_next(1).await,
            // A new player's first request, too.
            harness.player().random_start().await,
        ] {
            reply.assert_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
            reply.assert_lacks(&["fire", "needledrop.db", "pretending"]);
        }
    }

    #[tokio::test]
    async fn clear_my_data_removes_the_random_game_and_the_best_run_with_it() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let (mut player, first) = started(&harness).await;
        win_and_go_on(&harness, &mut player, 1, first).await;
        let old = harness.player_id(&player).unwrap();

        assert_eq!(player.clear().await.status, StatusCode::NO_CONTENT);

        assert_eq!(harness.store.random_game(&old).await.unwrap(), None);
        // The browser is a new player, who has no game...
        let new = harness.player_id(&player).unwrap();
        assert_ne!(new, old);
        player
            .random()
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_game");
        // ...and whose first session starts from nothing.
        let body = player.random_start().await.json();
        assert_eq!(body["round"], 1);
        assert_eq!(body["bestRun"], 0);
        assert_eq!(harness.player_id(&player), Some(new));
        // A copy of the old cookie leads to no game either.
        harness
            .player_with_id(&old)
            .random()
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_game");
    }

    #[tokio::test]
    async fn a_reset_removes_every_random_game_and_a_reroll_none() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let (mut one, first) = started(&harness).await;
        let (mut other, _) = started(&harness).await;
        let before = one.random_skip(1).await.json();

        // A re-roll is about the day's songs and the games played on them.
        let reply = harness.post("/api/admin/reroll", None).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        let reply = one.random().await;
        assert_eq!(reply.json(), before);
        assert_hides(&reply, first);
        assert_eq!(other.random().await.status, StatusCode::OK);

        // A reset is about everything the players did.
        let reply = harness.post("/api/admin/reset", None).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        for player in [&mut one, &mut other] {
            player
                .random()
                .await
                .assert_error(StatusCode::NOT_FOUND, "no_game");
            player
                .random_skip(1)
                .await
                .assert_error(StatusCode::NOT_FOUND, "no_game");
        }
        // The cookies still name the same players, who start again.
        let body = one.random_start().await.json();
        assert_eq!(body["round"], 1);
        assert_eq!(body["bestRun"], 0);
    }
}
