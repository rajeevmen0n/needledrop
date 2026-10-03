//! Random mode: one song after another, for as long as the player likes.
//!
//! The game is the daily one (the same ladder, the same matching, a skip or
//! a wrong guess costs a try), played against a song drawn from the song
//! pool instead of the day's pick. When a song is over the player asks for
//! the next, without limit. Nothing here has a day: a random game belongs to
//! a player and to nothing else.
//!
//! **The state** is one [`RandomGame`] per player, kept in the store and
//! replaced whole: the song being played and the tries used on it, the score
//! of the session, when the session was last played, the pool its songs are
//! drawn from, and the two things that outlast a session, the longest run
//! and the songs played lately. It is pure: no I/O and no clock; whoever
//! plays says what time it is.
//!
//! **The pool** is the player's choice of what to be given: every song
//! (`general`, which it is until they choose), or only the songs of one
//! genre. It is a [`Section`], because those are the four pools the song
//! database has. The choice is about the songs to come. It travels on
//! `POST /api/random/start`, and changing it in the middle of a song draws
//! nothing: the song being played stays, with its tries, and the session,
//! its run and its score go on across genres. So a change of genre is no
//! way to give up a song, which the owner did not want random mode to have.
//! Nor is it playing: it does not keep a session alive.
//!
//! **A session** is the player's, in whatever browser tab, and it ends by
//! itself: one that has not been played for [`SESSION_IDLE`] is over. Playing
//! is a start, a move or a next song; looking and listening do not count,
//! so a page left open does not keep a session alive. To every route but one
//! a session that is over is no game at all (404 `no_game`), and nothing is
//! written: the record stays where it is until `POST /api/random/start`
//! begins the next session from it. That route answers "the session to play
//! in": the one under way if there is one, a new one otherwise. So a second
//! tab joins the session, a reload goes on where it was, and half an hour
//! away starts from scratch.
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
//! The pool a session draws from is in every view, and it is not a part of
//! that secret: it is what the player asked for, it is about the next song,
//! and the song on screen may well have been drawn from another.
//!
//! Drawing and loading the songs is [`crate::daily`]'s work, because a random
//! song must not be one of the day's four and comes from the same Deezer.

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Bytes,
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
    store::{PlayerId, Section},
};

/// How many of a player's latest songs are remembered, so that the draw can
/// avoid them. Against a pool of hundreds that is long enough for a song not
/// to come back in one sitting, and short enough to keep the stored record
/// small.
pub const RECENT_SONGS: usize = 50;

/// How long a session lasts without being played. Long enough for a break
/// and for the longest clip listened to several times over; short enough
/// that someone who comes back later starts from scratch, as the owner asked
/// ("short lived if inactive").
pub const SESSION_IDLE: Duration = Duration::from_secs(30 * 60);

/// [`SESSION_IDLE`] in seconds, the unit the state keeps time in. Half an
/// hour fits an `i64` with room to spare.
const SESSION_IDLE_SECONDS: i64 = SESSION_IDLE.as_secs() as i64;

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
/// {"round":12,"track_id":3135556,"game":{"day":"2026-10-01","attempts":[{"kind":"skip"}],"status":"playing"},"run":2,"played":3,"won":2,"best_run":5,"recent":[916424,3135556],"active_at":1790899200,"pool":"rock"}
/// ```
///
/// Times are seconds since the Unix epoch, and they are the caller's: every
/// method that plays takes `now`. The pool is a section's slug.
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
    /// When the session was last played: its start, a move or a next song.
    /// The session is over [`SESSION_IDLE`] after this.
    active_at: i64,
    /// The pool the session's next songs are drawn from: General for every
    /// song, a genre for the songs tagged with it. It says nothing about the
    /// song being played, which was drawn from whatever the pool was then.
    pool: Section,
}

/// The pool of a player who has not chosen one: every song.
fn whole_pool() -> Section {
    Section::General
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
    /// Missing in a record stored before sessions ended by themselves. Such
    /// a session reads as last played in 1970, so as long over; what
    /// outlasts a session is carried over from it all the same.
    #[serde(default)]
    active_at: i64,
    /// Missing in a record stored before the pool could be chosen, when
    /// every song was drawn from all of them. A slug that is none of the
    /// four is no state at all, like any other field that cannot be read.
    #[serde(default = "whole_pool")]
    pool: Section,
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
            active_at: stored.active_at,
            pool: stored.pool,
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
    /// A new session, begun at `now` on the song `track_id`, drawn on `day`:
    /// no tries used, no run, nothing played. `previous` is the player's game
    /// until now, if they had one, in whatever state: its longest run and
    /// its recent songs are carried over, and the round goes on counting
    /// from it.
    ///
    /// `pool` is what the session draws from, and what `track_id` was drawn
    /// from. It is the caller's to decide, because the draw comes before the
    /// session: the player's choice if they made one, else the pool of
    /// `previous` ([`pool`](Self::pool)), which so carries over like the
    /// longest run, else all of it.
    pub fn start(
        previous: Option<&RandomGame>,
        track_id: u64,
        pool: Section,
        day: Date,
        now: i64,
    ) -> Self {
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
            active_at: now,
            pool,
        }
    }

    /// The same session on its next song, `track_id`, drawn on `day` at
    /// `now`. Only a song that is won or lost can be left behind. The pool
    /// stays what it is: a session draws from it until the player chooses
    /// another.
    pub fn next(&self, track_id: u64, day: Date, now: i64) -> Result<Self, Unfinished> {
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
            active_at: now,
            pool: self.pool,
        })
    }

    /// Changes the pool the songs to come are drawn from, and nothing else.
    ///
    /// The song being played stays, with its tries: the player asked for
    /// other songs from here on, not for another song now, and a change of
    /// genre that replaced the song would be the give-up shortcut random
    /// mode does not have. The round, the score and the recent songs are
    /// the session's, whatever it draws from. And choosing is not playing:
    /// it takes no `now`, and the session ends when it would have ended.
    pub fn draw_from(&mut self, pool: Section) {
        self.pool = pool;
    }

    /// Whether the session has ended by `now`: it was last played
    /// [`SESSION_IDLE`] ago or longer. What is left of it then is what a new
    /// session carries over.
    ///
    /// A `now` before the last play (the machine's clock was set back) ends
    /// nothing: the session is as alive as it was when it was played.
    pub fn is_over(&self, now: i64) -> bool {
        now.saturating_sub(self.active_at) >= SESSION_IDLE_SECONDS
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

    /// When the session was last played, in seconds since the Unix epoch.
    pub fn active_at(&self) -> i64 {
        self.active_at
    }

    /// The pool the next songs are drawn from; see the field.
    pub fn pool(&self) -> Section {
        self.pool
    }

    /// Gives up the current turn at `now`; see [`GameState::skip`]. A skip
    /// that loses the song ends the run.
    pub fn skip(&mut self, now: i64) -> Result<Status, GameError> {
        let status = self.game.skip()?;
        self.settle(status, now);
        Ok(status)
    }

    /// Plays `guessed` against `answer`, the song of [`track_id`](Self::track_id),
    /// at `now`; see [`GameState::guess`]. A win adds to the run, a loss ends
    /// it.
    pub fn guess(
        &mut self,
        answer: &TrackMeta,
        guessed: &TrackMeta,
        now: i64,
    ) -> Result<Status, GameError> {
        let status = self.game.guess(answer, guessed)?;
        self.settle(status, now);
        Ok(status)
    }

    /// Notes a move made at `now`, which keeps the session going, and takes
    /// the song into the score if the move that led to `status` ended it. A
    /// finished song refuses further moves, so each is counted once and a
    /// refused move does not count as playing.
    fn settle(&mut self, status: Status, now: i64) {
        self.active_at = now;
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
    /// The pool the session's next songs are drawn from, as a section's
    /// slug. The player's own choice, and not necessarily where the song
    /// being played came from, so it gives nothing of that song away.
    pool: Section,
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
            pool: game.pool(),
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

/// The record the store has for `player`, whether its session is still
/// going or long over. `None`: they never played random mode, or their data
/// was cleared.
async fn stored_game(app: &AppState, player: &PlayerId) -> Result<Option<RandomGame>, ApiError> {
    app.store().random_game(player).await.map_err(store_failed)
}

/// The session `player` is in at `now`. A session that is over is no game,
/// exactly like no record at all, and that is an answer the client acts on:
/// it starts a session.
async fn live_game(app: &AppState, player: &PlayerId, now: i64) -> Result<RandomGame, ApiError> {
    stored_game(app, player)
        .await?
        .filter(|game| !game.is_over(now))
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

/// The session the player is in. A look: it stores nothing, issues no ID and
/// does not count as playing, so a browser the server has not seen, one that
/// has never started a session and one whose session has ended are all told
/// there is no game, and start one.
///
/// The song is that of the record the view is built from, so the response is
/// about one song even if another tab moves on in the meantime.
async fn show(
    State(app): State<AppState>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let player = player::known_player(&jar).ok_or(ApiError::NoGame)?;
    let game = live_game(&app, &player, app.now()).await?;
    let song = song_of(&app, &game).await?;
    Ok(game_response(jar, &headers, &player, &game, &song))
}

// --- GET /api/random/audio --------------------------------------------------------

/// The clip the player has unlocked of their random song, cut like a daily
/// clip: the leading frames and no tags. No cookie is set, as on the daily
/// audio route, and there is no clip without a session: random mode has no
/// song that everyone shares. Listening does not count as playing.
async fn audio(State(app): State<AppState>, jar: PrivateCookieJar) -> Result<Response, ApiError> {
    let player = player::known_player(&jar).ok_or(ApiError::NoGame)?;
    let game = live_game(&app, &player, app.now()).await?;
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

/// The pool a start asks to draw from: `Some(Some(pool))` for
/// `{ "pool": "rock" }`, and `Some(None)`, no choice, for a body that is
/// empty, `{}` or `{ "pool": null }`. `None` when the body is anything else:
/// not a JSON object, or a `pool` that is none of the four slugs. Other
/// fields are not looked at.
///
/// The content type is not looked at either: "the session to play in" is a
/// request without a body, and a client that sends none has no reason to
/// label it. It is read like the body of a re-roll, which names a section or
/// none in the same way.
fn chosen_pool(body: &[u8]) -> Option<Option<Section>> {
    if body.trim_ascii().is_empty() {
        return Some(None);
    }
    let body: serde_json::Value = serde_json::from_slice(body).ok()?;
    match body.as_object()?.get("pool") {
        None | Some(serde_json::Value::Null) => Some(None),
        Some(pool) => Section::deserialize(pool).ok().map(Some),
    }
}

/// Makes `choice` the pool `game` draws from and stores the game, when it is
/// a choice and not the pool the game has already. To be called with the
/// player's turn taken, on the game as it was read under the lock.
///
/// Nothing else of the game changes ([`RandomGame::draw_from`]): no song is
/// drawn, and the write does not count as playing.
async fn choose_pool(
    app: &AppState,
    player: &PlayerId,
    game: &mut RandomGame,
    choice: Option<Section>,
) -> Result<(), ApiError> {
    let Some(pool) = choice.filter(|pool| *pool != game.pool()) else {
        return Ok(());
    };
    game.draw_from(pool);
    app.store()
        .save_random_game(player, game)
        .await
        .map_err(store_failed)
}

/// The session to play in, and the pool it draws from.
///
/// The body may name a pool: `{ "pool": "rock" }`, one of the four section
/// slugs ([`chosen_pool`]). This is where the choice travels because the
/// deployment forwards a fixed list of routes, and because choosing what to
/// be given is part of saying where one wants to play.
///
/// A player whose session is still going is given it as it stands: nothing
/// is drawn, so a second browser tab joins the session instead of ending
/// it. Without a choice, or with the pool the session has, nothing is
/// written either. Another pool is stored, and that is all: the song being
/// played, its tries, the score and the time the session was last played
/// stay, and the next song is the first to be drawn from the new pool.
///
/// Otherwise a new session begins: a song is drawn, and the run and the
/// totals start at zero; the longest run and the recent songs carry over
/// from the session that ended, and so does its pool, unless the request
/// chooses one. A player without a record draws from all of it.
///
/// This is the one route of random mode that works without a cookie: it is
/// the first thing a new player's page sends.
async fn start(
    State(app): State<AppState>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    // Read before anyone is asked who is playing, like every body.
    let choice = chosen_pool(&body).ok_or(ApiError::BadRequest(
        "Send {\"pool\": <\"general\", \"pop\", \"rock\" or \"hip-hop\">} as JSON, or no body to keep the pool as it is.",
    ))?;

    let now = app.now();
    let today = app.today().await?;
    let known = player::known_player(&jar);
    let previous = match &known {
        Some(player) => stored_game(&app, player).await?,
        None => None,
    };
    // A first look, before anything is drawn: joining a session costs
    // Deezer nothing.
    if let (Some(player), Some(game)) = (&known, &previous)
        && !game.is_over(now)
    {
        // The song first, as everything slow: a song that cannot be loaded
        // is an error before anything is written.
        let song = song_of(&app, game).await?;
        if choice.is_none_or(|pool| pool == game.pool()) {
            return Ok(game_response(jar, &headers, player, game, &song));
        }

        // Another pool for the songs to come: a write, so it takes the
        // player's turn and is made on the game as it stands then. A move
        // made in another tab since the look above must not be undone by
        // storing what was read there.
        let changed = {
            let _turn = app.move_locks().lock(player).await;
            match stored_game(&app, player).await? {
                Some(mut current) if !current.is_over(now) => {
                    choose_pool(&app, player, &mut current, choice).await?;
                    Some(current)
                }
                _ => None,
            }
        };
        if let Some(changed) = changed {
            // Another tab may have moved on to the next song in between;
            // then it is that song the view is about.
            let song = if changed.round() == game.round() {
                song
            } else {
                song_of(&app, &changed).await?
            };
            return Ok(game_response(jar, &headers, player, &changed, &song));
        }
        // The session is gone since the look above (Clear my data, a
        // reset): what is left to do is to begin one, as below.
    }

    // The pool of the new session: the one asked for, else the one the
    // player drew from last, else all of it. It is settled here, before the
    // draw, and the session that begins keeps it whatever the record says
    // by then: its first song was drawn from this pool.
    let pool = choice
        .or(previous.as_ref().map(RandomGame::pool))
        .unwrap_or_else(whole_pool);
    // The slow part, before the player's turn is taken: the draw loads the
    // song, which can be a download.
    let recent = previous.map_or_else(Vec::new, |game| game.recent().to_vec());
    let (track_id, song) = app
        .daily()
        .random_song(today, pool, &recent)
        .await
        .map_err(daily_failed)?;
    let player = known.unwrap_or_else(PlayerId::generate);

    let started = {
        let _turn = app.move_locks().lock(&player).await;
        // Read again under the lock. Two tabs that both found the session
        // over each drew a song; the first to get here begins the session
        // and the second must join it, or each tab would be in a session
        // the other has ended. A move in another tab may also have ended a
        // song since the look above, and its run must not be lost.
        let current = stored_game(&app, &player).await?;
        match current {
            Some(mut game) if !game.is_over(now) => {
                // Joining it with a choice of one's own is what choosing in
                // a session that is going always is: the pool of the songs
                // to come, and the other tab's song stays.
                choose_pool(&app, &player, &mut game, choice).await?;
                Err(game)
            }
            previous => {
                let game = RandomGame::start(previous.as_ref(), track_id, pool, today, now);
                app.store()
                    .save_random_game(&player, &game)
                    .await
                    .map_err(store_failed)?;
                Ok(game)
            }
        }
    };
    match started {
        Ok(game) => Ok(game_response(jar, &headers, &player, &game, &song)),
        // The other tab's session. Its song is loaded outside the lock,
        // like every song; the one drawn here is not played.
        Err(game) => {
            let song = song_of(&app, &game).await?;
            Ok(game_response(jar, &headers, &player, &game, &song))
        }
    }
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
    // The time of the move: read once, so that the look here and the check
    // under the lock agree on whether the session is still going.
    let now = app.now();
    // A first look, before anything is looked up: a move in a session that
    // is over, for another song or on a finished one costs Deezer nothing.
    // The checks that count are made under the lock below.
    let planned = live_game(&app, &player, now).await?;
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
        let mut game = live_game(&app, &player, now).await?;
        // Another tab may have moved on to the next song since the look
        // above. The song fetched there is the one of that round, so the
        // round still being the same is what makes it the song this move is
        // judged against.
        same_round(&game, round)?;
        let moved = match &guessed {
            None => game.skip(now),
            Some(guessed) => game.guess(&song.meta, guessed, now),
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
/// run and the totals go on, and so does the session: asking for a song is
/// playing.
///
/// It is drawn from the pool the session has. When that pool has nothing to
/// give (404 `no_song`), nothing is changed, and the player can choose
/// another with `POST /api/random/start` and ask again.
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
    let now = app.now();
    let today = app.today().await?;
    let planned = live_game(&app, &player, now).await?;
    same_round(&planned, round)?;
    if !planned.game().is_finished() {
        return Err(ApiError::Unfinished);
    }

    // The slow part, before the player's turn is taken.
    let (track_id, song) = app
        .daily()
        .random_song(today, planned.pool(), planned.recent())
        .await
        .map_err(daily_failed)?;

    let game = {
        let _turn = app.move_locks().lock(&player).await;
        let current = live_game(&app, &player, now).await?;
        // Two requests for the song after the same one (a double tap, two
        // tabs): the first moves on, and the second finds another round.
        same_round(&current, round)?;
        // The pool may have been changed in another tab since the song was
        // drawn. The song is played all the same, and the session keeps the
        // pool it has now: the change is about the songs after this one.
        let game = current
            .next(track_id, today, now)
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
    /// A moment for the sessions of the pure tests, in seconds since the
    /// Unix epoch. Nothing reads a clock: time is whatever a test passes in.
    const NOW: i64 = 1_790_000_000;
    /// [`SESSION_IDLE`] in seconds.
    const IDLE: i64 = 30 * 60;

    fn the_song() -> TrackMeta {
        TrackMeta::new("The Song", "", "Someone")
    }

    fn another_song() -> TrackMeta {
        TrackMeta::new("Under Pressure", "", "Queen")
    }

    /// Wins the song `game` is on.
    fn win(game: &mut RandomGame) {
        assert_eq!(game.guess(&the_song(), &the_song(), NOW), Ok(Status::Won));
    }

    /// Loses the song `game` is on, with as many skips as it takes.
    fn lose(game: &mut RandomGame) {
        while game.skip(NOW).unwrap() == Status::Playing {}
        assert_eq!(game.game().status(), Status::Lost);
    }

    #[test]
    fn a_first_session_starts_at_round_one_with_nothing_on_record() {
        let game = RandomGame::start(None, 7, Section::General, DAY, NOW);
        assert_eq!(game.round(), 1);
        assert_eq!(game.track_id(), 7);
        assert_eq!(game.game(), &GameState::new(DAY));
        assert_eq!((game.run(), game.played(), game.won()), (0, 0, 0));
        assert_eq!(game.best_run(), 0);
        assert_eq!(game.recent(), [7]);
    }

    #[test]
    fn a_win_adds_to_the_run_and_a_loss_ends_it_but_not_the_best() {
        let mut game = RandomGame::start(None, 1, Section::General, DAY, NOW);
        // Misses on the way change nothing of the score.
        assert_eq!(game.skip(NOW), Ok(Status::Playing));
        assert_eq!(
            game.guess(&the_song(), &another_song(), NOW),
            Ok(Status::Playing)
        );
        assert_eq!((game.run(), game.played(), game.won()), (0, 0, 0));
        assert_eq!(game.game().attempts().len(), 2);

        win(&mut game);
        assert_eq!((game.run(), game.played(), game.won()), (1, 1, 1));
        assert_eq!(game.best_run(), 1);

        let mut game = game.next(2, DAY, NOW).unwrap();
        assert_eq!(game.round(), 2);
        assert_eq!(game.game(), &GameState::new(DAY));
        // The score goes on with the session.
        assert_eq!((game.run(), game.played(), game.won()), (1, 1, 1));
        win(&mut game);
        let mut game = game.next(3, DAY, NOW).unwrap();
        win(&mut game);
        assert_eq!((game.run(), game.played(), game.won()), (3, 3, 3));
        assert_eq!(game.best_run(), 3);

        let mut game = game.next(4, DAY, NOW).unwrap();
        lose(&mut game);
        assert_eq!((game.run(), game.played(), game.won()), (0, 4, 3));
        assert_eq!(game.best_run(), 3);

        // A shorter run afterwards does not replace the longest.
        let mut game = game.next(5, DAY, NOW).unwrap();
        win(&mut game);
        assert_eq!((game.run(), game.played(), game.won()), (1, 5, 4));
        assert_eq!(game.best_run(), 3);
        assert_eq!(game.round(), 5);
        assert_eq!(game.recent(), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_finished_song_takes_no_more_moves_and_is_counted_once() {
        let mut won = RandomGame::start(None, 1, Section::General, DAY, NOW);
        win(&mut won);
        let before = won.clone();
        assert_eq!(won.skip(NOW), Err(GameError::Finished));
        assert_eq!(
            won.guess(&the_song(), &the_song(), NOW),
            Err(GameError::Finished)
        );
        assert_eq!(won, before);

        let mut lost = RandomGame::start(None, 1, Section::General, DAY, NOW);
        lose(&mut lost);
        let before = lost.clone();
        assert_eq!(lost.skip(NOW), Err(GameError::Finished));
        assert_eq!(
            lost.guess(&the_song(), &the_song(), NOW),
            Err(GameError::Finished)
        );
        assert_eq!(lost, before);
        assert_eq!((lost.run(), lost.played(), lost.won()), (0, 1, 0));
    }

    #[test]
    fn the_next_song_needs_the_current_one_to_be_over() {
        let mut game = RandomGame::start(None, 1, Section::General, DAY, NOW);
        assert_eq!(game.next(2, DAY, NOW), Err(Unfinished));
        game.skip(NOW).unwrap();
        assert_eq!(game.next(2, DAY, NOW), Err(Unfinished));
        lose(&mut game);
        assert!(game.next(2, DAY, NOW).is_ok());
    }

    #[test]
    fn a_new_session_zeroes_the_score_and_keeps_the_best_run_the_round_and_the_recent_songs() {
        let mut game = RandomGame::start(None, 1, Section::General, DAY, NOW);
        win(&mut game);
        let mut game = game.next(2, DAY, NOW).unwrap();
        win(&mut game);
        let mut game = game.next(3, DAY, NOW).unwrap();
        game.skip(NOW).unwrap();

        // In the middle of a song: it is abandoned and counts for nothing.
        let later = date(2026, 10, 9);
        let fresh = RandomGame::start(Some(&game), 4, Section::General, later, NOW);
        assert_eq!(fresh.round(), 4);
        assert_eq!(fresh.track_id(), 4);
        assert_eq!(fresh.game(), &GameState::new(later));
        assert_eq!((fresh.run(), fresh.played(), fresh.won()), (0, 0, 0));
        assert_eq!(fresh.best_run(), 2);
        assert_eq!(fresh.recent(), [1, 2, 3, 4]);

        // And after a finished one just the same.
        let mut over = game.clone();
        lose(&mut over);
        let fresh = RandomGame::start(Some(&over), 4, Section::General, later, NOW);
        assert_eq!((fresh.run(), fresh.played(), fresh.won()), (0, 0, 0));
        assert_eq!(fresh.best_run(), 2);
        assert_eq!(fresh.round(), 4);
    }

    #[test]
    fn only_so_many_recent_songs_are_kept_each_once_and_the_current_one_last() {
        let mut game = RandomGame::start(None, 1, Section::General, DAY, NOW);
        for track_id in 2..=RECENT_SONGS as u64 + 10 {
            lose(&mut game);
            game = game.next(track_id, DAY, NOW).unwrap();
        }
        let expected: Vec<u64> = (11..=RECENT_SONGS as u64 + 10).collect();
        assert_eq!(game.recent(), expected);
        assert_eq!(game.recent().len(), RECENT_SONGS);

        // A song that comes back moves to the end instead of being there twice.
        lose(&mut game);
        let game = game.next(20, DAY, NOW).unwrap();
        assert_eq!(game.recent().len(), RECENT_SONGS);
        assert_eq!(game.recent().last(), Some(&20));
        assert_eq!(
            game.recent().iter().filter(|played| **played == 20).count(),
            1
        );
        // The same in a new session, the same song included.
        let again = RandomGame::start(Some(&game), 20, Section::General, DAY, NOW);
        assert_eq!(again.recent(), game.recent());
        let other = RandomGame::start(Some(&game), 12, Section::General, DAY, NOW);
        assert_eq!(other.recent().len(), RECENT_SONGS);
        assert_eq!(other.recent().last(), Some(&12));
    }

    #[test]
    fn a_random_game_reads_back_from_its_json_as_it_was() {
        let mut game = RandomGame::start(None, 916_424, Section::General, DAY, NOW);
        win(&mut game);
        let mut game = game.next(3_135_556, DAY, NOW).unwrap();
        game.skip(NOW).unwrap();
        game.guess(&the_song(), &another_song(), NOW).unwrap();

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
                "active_at": NOW,
                "pool": "general",
            })
        );
        assert_eq!(serde_json::from_value::<RandomGame>(json).unwrap(), game);

        // Every state a session passes through reads back too.
        let mut game = RandomGame::start(None, 1, Section::General, DAY, NOW);
        for track_id in 2..12 {
            if track_id % 3 == 0 {
                lose(&mut game);
            } else {
                win(&mut game);
            }
            let text = serde_json::to_string(&game).unwrap();
            assert_eq!(serde_json::from_str::<RandomGame>(&text).unwrap(), game);
            game = if track_id % 5 == 0 {
                RandomGame::start(Some(&game), track_id, Section::General, DAY, NOW)
            } else {
                game.next(track_id, DAY, NOW).unwrap()
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
            ("a pool that is no section", with("pool", json!("jazz"))),
            ("a pool that is no slug", with("pool", json!(2))),
            ("a pool that is nothing", with("pool", Value::Null)),
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

    // --- how long a session lasts -----------------------------------------------------

    #[test]
    fn a_session_is_over_half_an_hour_after_it_was_last_played() {
        let game = RandomGame::start(None, 1, Section::General, DAY, NOW);
        assert_eq!(game.active_at(), NOW);
        assert!(!game.is_over(NOW));
        assert!(!game.is_over(NOW + 29 * 60));
        assert!(!game.is_over(NOW + IDLE - 1));
        // Half an hour to the second, and from then on.
        assert!(game.is_over(NOW + IDLE));
        assert!(game.is_over(NOW + IDLE + 1));
        assert!(game.is_over(NOW + 86_400));
        assert!(game.is_over(i64::MAX));
        assert_eq!(SESSION_IDLE, Duration::from_secs(30 * 60));
    }

    #[test]
    fn every_way_of_playing_keeps_the_session_going_and_a_refused_move_does_not() {
        let mut game = RandomGame::start(None, 1, Section::General, DAY, NOW);

        // A skip, a wrong guess, a win and the next song are all playing.
        game.skip(NOW + 100).unwrap();
        assert_eq!(game.active_at(), NOW + 100);
        game.guess(&the_song(), &another_song(), NOW + 200).unwrap();
        assert_eq!(game.active_at(), NOW + 200);
        game.guess(&the_song(), &the_song(), NOW + 300).unwrap();
        assert_eq!(game.active_at(), NOW + 300);
        assert!(!game.is_over(NOW + 300 + IDLE - 1));
        assert!(game.is_over(NOW + 300 + IDLE));

        // A move the finished song refuses is not playing, and changes nothing.
        let before = game.clone();
        assert_eq!(game.skip(NOW + 400), Err(GameError::Finished));
        assert_eq!(
            game.guess(&the_song(), &the_song(), NOW + 400),
            Err(GameError::Finished)
        );
        assert_eq!(game, before);
        // Neither is a next song that is refused.
        let unfinished = RandomGame::start(None, 1, Section::General, DAY, NOW);
        assert_eq!(unfinished.next(2, DAY, NOW + 400), Err(Unfinished));
        assert_eq!(unfinished.active_at(), NOW);

        let game = game.next(2, DAY, NOW + 500).unwrap();
        assert_eq!(game.active_at(), NOW + 500);
        // The session that follows is last played when it begins.
        let fresh = RandomGame::start(Some(&game), 3, Section::General, DAY, NOW + 9_000);
        assert_eq!(fresh.active_at(), NOW + 9_000);
        assert!(!fresh.is_over(NOW + 9_000 + IDLE - 1));
    }

    #[test]
    fn a_clock_that_went_back_ends_no_session() {
        let mut game = RandomGame::start(None, 1, Section::General, DAY, NOW);
        // The machine's clock is corrected to before the session began.
        assert!(!game.is_over(NOW - 1));
        assert!(!game.is_over(NOW - 86_400));
        assert!(!game.is_over(0));
        assert!(!game.is_over(i64::MIN));

        // Playing by the corrected clock goes on from what it says.
        game.skip(NOW - 3_600).unwrap();
        assert_eq!(game.active_at(), NOW - 3_600);
        assert!(!game.is_over(NOW - 3_600 + IDLE - 1));
        assert!(game.is_over(NOW - 3_600 + IDLE));
    }

    #[test]
    fn a_record_from_before_sessions_ended_is_a_session_long_over_that_is_carried_over() {
        // What was stored before the state knew when it was last played.
        let old = json!({
            "round": 5,
            "track_id": 7,
            "game": { "day": "2026-10-01", "attempts": [{ "kind": "skip" }], "status": "playing" },
            "run": 1,
            "played": 3,
            "won": 2,
            "best_run": 4,
            "recent": [9, 8, 7],
        });
        let old: RandomGame = serde_json::from_value(old).unwrap();
        assert_eq!(old.active_at(), 0);
        assert!(old.is_over(NOW));
        assert!(old.is_over(IDLE));

        // Everything that outlasts a session is still there for the next.
        let fresh = RandomGame::start(Some(&old), 11, Section::General, DAY, NOW);
        assert_eq!(fresh.round(), 6);
        assert_eq!(fresh.best_run(), 4);
        assert_eq!(fresh.recent(), [9, 8, 7, 11]);
        assert_eq!((fresh.run(), fresh.played(), fresh.won()), (0, 0, 0));
        assert!(!fresh.is_over(NOW));

        // Written again, it says when; and a time before 1970 reads too.
        let json = serde_json::to_value(&fresh).unwrap();
        assert_eq!(json["active_at"], NOW);
        let mut odd = json.clone();
        odd["active_at"] = json!(-5);
        assert_eq!(
            serde_json::from_value::<RandomGame>(odd)
                .unwrap()
                .active_at(),
            -5
        );
        // Something that is no time at all is not a state.
        let mut bad = json;
        bad["active_at"] = json!("yesterday");
        assert!(serde_json::from_value::<RandomGame>(bad).is_err());
    }

    // --- the pool the songs are drawn from --------------------------------------------

    #[test]
    fn a_session_draws_from_the_pool_it_was_started_with_song_after_song() {
        for pool in Section::ALL {
            let mut game = RandomGame::start(None, 1, pool, DAY, NOW);
            assert_eq!(game.pool(), pool);
            // Moves, a win, a loss and every next song leave it alone.
            game.skip(NOW).unwrap();
            game.guess(&the_song(), &another_song(), NOW).unwrap();
            win(&mut game);
            assert_eq!(game.pool(), pool);
            let mut game = game.next(2, DAY, NOW + 60).unwrap();
            assert_eq!(game.pool(), pool);
            lose(&mut game);
            let game = game.next(3, DAY, NOW + 120).unwrap();
            assert_eq!(game.pool(), pool);
            assert_eq!(game.round(), 3);
        }
    }

    #[test]
    fn a_new_session_draws_from_the_pool_it_is_given_whatever_the_last_one_drew_from() {
        let mut last = RandomGame::start(None, 1, ROCK, DAY, NOW);
        win(&mut last);

        // The caller carries the pool over, as it does the rest...
        let carried = RandomGame::start(Some(&last), 2, last.pool(), DAY, NOW + 2 * IDLE);
        assert_eq!(carried.pool(), ROCK);
        // ...or replaces it with the player's choice. Either way what
        // outlasts a session is carried over, and the score is not.
        for pool in Section::ALL {
            let fresh = RandomGame::start(Some(&last), 2, pool, DAY, NOW + 2 * IDLE);
            assert_eq!(fresh.pool(), pool);
            assert_eq!(fresh.round(), 2);
            assert_eq!(fresh.best_run(), 1);
            assert_eq!(fresh.recent(), [1, 2]);
            assert_eq!((fresh.run(), fresh.played(), fresh.won()), (0, 0, 0));
        }
        // The record it started from is not changed by it.
        assert_eq!(last.pool(), ROCK);
    }

    #[test]
    fn changing_the_pool_changes_nothing_else_and_is_not_playing() {
        let mut game = RandomGame::start(None, 1, Section::General, DAY, NOW);
        win(&mut game);
        let mut game = game.next(2, DAY, NOW + 60).unwrap();
        game.skip(NOW + 120).unwrap();
        game.guess(&the_song(), &another_song(), NOW + 180).unwrap();
        let before = game.clone();

        game.draw_from(POP);
        assert_eq!(game.pool(), POP);
        // The song in progress is not replaced, and none of its tries are
        // given back: this is no way to give a song up.
        assert_eq!(game.round(), before.round());
        assert_eq!(game.track_id(), before.track_id());
        assert_eq!(game.game(), before.game());
        assert_eq!(game.game().attempts().len(), 2);
        // The session and its score carry on across genres.
        assert_eq!(
            (game.run(), game.played(), game.won(), game.best_run()),
            (1, 1, 1, 1)
        );
        assert_eq!(game.recent(), before.recent());
        // It was last played when the last move was made, and ends when it
        // would have ended.
        assert_eq!(game.active_at(), NOW + 180);
        assert!(!game.is_over(NOW + 180 + IDLE - 1));
        assert!(game.is_over(NOW + 180 + IDLE));
        // Put back, it is the game it was, to the last field.
        game.draw_from(Section::General);
        assert_eq!(game, before);

        // A finished song can have its pool changed too, and stays finished.
        let mut over = before.clone();
        lose(&mut over);
        let finished = over.clone();
        over.draw_from(HIP_HOP);
        assert_eq!(over.game(), finished.game());
        assert_eq!(over.game().status(), Status::Lost);
        assert_eq!(over.active_at(), finished.active_at());
        // The next song is the first of the new pool's, in the same session.
        let next = over.next(3, DAY, NOW + 600).unwrap();
        assert_eq!(next.pool(), HIP_HOP);
        assert_eq!((next.played(), next.won(), next.best_run()), (2, 1, 1));
    }

    #[test]
    fn the_pool_is_stored_as_a_section_slug_and_a_record_without_one_draws_from_all_of_it() {
        for (pool, slug) in [
            (Section::General, "general"),
            (POP, "pop"),
            (ROCK, "rock"),
            (HIP_HOP, "hip-hop"),
        ] {
            let mut game = RandomGame::start(None, 7, Section::General, DAY, NOW);
            game.draw_from(pool);
            let json = serde_json::to_value(&game).unwrap();
            assert_eq!(json["pool"], slug);
            assert_eq!(serde_json::from_value::<RandomGame>(json).unwrap(), game);
        }

        // What was stored before the pool could be chosen: every song was
        // drawn from all of them, and that is how it reads.
        let old = json!({
            "round": 5,
            "track_id": 7,
            "game": { "day": "2026-10-01", "attempts": [{ "kind": "skip" }], "status": "playing" },
            "run": 1,
            "played": 3,
            "won": 2,
            "best_run": 4,
            "recent": [9, 8, 7],
            "active_at": NOW,
        });
        let old: RandomGame = serde_json::from_value(old).unwrap();
        assert_eq!(old.pool(), Section::General);
        // It is a session like any other: still going, and its next song is
        // drawn from everything.
        assert!(!old.is_over(NOW + 60));
        assert_eq!(old.game().attempts().len(), 1);
        // Written again, it says so.
        assert_eq!(serde_json::to_value(&old).unwrap()["pool"], "general");
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

    /// The pop songs and the rock songs of [`genres`]' pool.
    const POP_SONGS: [u64; 2] = [ALPHA, BETA];
    const ROCK_SONGS: [u64; 2] = [GAMMA, DELTA];

    /// A server whose pool has two pop songs ([`POP_SONGS`]), two rock songs
    /// ([`ROCK_SONGS`]), one song without a tag (`EPSILON`) and no hip-hop.
    /// The day's four songs are others, which have left the pool since, so
    /// every song of the pool can be drawn.
    async fn genres() -> Harness {
        genres_over(Arc::new(MemoryStore::new())).await
    }

    /// The same over a store of the test's choosing.
    async fn genres_over(store: Arc<dyn crate::store::Store>) -> Harness {
        let pool: [(u64, &[Genre]); 5] = [
            (ALPHA, &[Genre::Pop]),
            (BETA, &[Genre::Pop]),
            (GAMMA, &[Genre::Rock]),
            (DELTA, &[Genre::Rock]),
            (EPSILON, &[]),
        ];
        let harness = Harness::over(tracks(), &pool, store).await;
        for (section, track_id) in Section::ALL.into_iter().zip(FILLER..) {
            let pick = Pick {
                day: TODAY,
                section,
                track_id,
            };
            harness.store.save_pick(pick).await.unwrap();
        }
        harness
    }

    /// A browser that has started a session drawing from `pool`, and the
    /// song it is on. The start's response is checked like any playing-state
    /// response.
    async fn started_from(harness: &Harness, pool: Section) -> (Player, u64) {
        let mut player = harness.player();
        let reply = player.random_start_from(pool).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json()["pool"], pool.slug());
        let game = stored(harness, &player).await;
        assert_eq!(game.pool(), pool);
        assert_hides(&reply, game.track_id());
        (player, game.track_id())
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
                "pool": "general",
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
    async fn a_start_without_a_choice_keeps_the_pool_whatever_else_its_body_says() {
        let harness = start(&[ALPHA, BETA]).await;
        let mut player = harness.player();
        // No body, an empty one, an object that names no pool or names
        // none, and fields that are another request's: all of them are "the
        // session to play in", under any content type.
        for (content_type, body) in [
            ("application/json", ""),
            ("text/plain", "  \n"),
            ("application/json", "{}"),
            ("text/plain", "{}"),
            ("application/json", r#"{"pool": null}"#),
            ("application/x-www-form-urlencoded", r#"{"pool": null}"#),
            ("application/json", r#"{"round": 9, "skip": true}"#),
            ("application/json", r#"{"section": "rock", "Pool": "rock"}"#),
        ] {
            let reply = player
                .post_to("/api/random/start", content_type, body)
                .await;
            assert_eq!(
                reply.status,
                StatusCode::OK,
                "{body:?}: {}",
                reply.visible()
            );
            assert_eq!(reply.json()["status"], "playing");
            assert_eq!(reply.json()["pool"], "general", "{body:?}");
            assert_hides(&reply, BETA);
        }
        // The first began a session and the others joined it: whatever the
        // body said, it was the same request.
        let game = stored(&harness, &player).await;
        assert_eq!(game.round(), 1);
        assert_eq!(game.pool(), Section::General);

        // A choice is read under any content type too, and with none.
        let reply = player
            .post_to("/api/random/start", "text/plain", r#"{"pool": "rock"}"#)
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json()["pool"], "rock");
        let request = axum::http::Request::post("/api/random/start")
            .body(axum::body::Body::from(r#" {"pool":"pop","other":[1]} "#))
            .unwrap();
        let reply = player.send(request).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json()["pool"], "pop");
        assert_eq!(reply.json()["round"], 1);
        assert_hides(&reply, BETA);
    }

    /// The sentence a start that cannot be read is answered with.
    const BAD_START: &str = "Send {\"pool\": <\"general\", \"pop\", \"rock\" or \"hip-hop\">} as JSON, or no body to keep the pool as it is.";

    #[tokio::test]
    async fn a_start_that_cannot_be_read_is_a_bad_request_and_changes_nothing() {
        let harness = genres().await;
        let (mut player, track_id) = started(&harness).await;
        assert_eq!(player.random_skip(1).await.status, StatusCode::OK);
        let before = stored(&harness, &player).await;
        let hits = harness.deezer.api_hits();

        let bodies = [
            json!({ "pool": "jazz" }),
            json!({ "pool": "Pop" }),
            json!({ "pool": "hiphop" }),
            json!({ "pool": "" }),
            json!({ "pool": 7 }),
            json!({ "pool": true }),
            json!({ "pool": ["pop"] }),
            json!({ "pool": { "slug": "pop" } }),
            json!([]),
            json!(["pop"]),
            json!("pop"),
            json!(7),
            json!(null),
        ];
        for body in &bodies {
            let reply = player.random_start_with(body.clone()).await;
            reply.assert_error(StatusCode::BAD_REQUEST, "bad_request");
            assert_eq!(reply.json()["message"], BAD_START, "{body}");
            assert_hides(&reply, track_id);
            assert!(reply.headers.get(header::SET_COOKIE).is_none());
        }
        for (content_type, body) in [
            ("application/json", "{not json"),
            ("text/plain", "anything at all"),
            ("application/json", "pop"),
            ("application/json", r#"{"pool": "rock"} trailing"#),
        ] {
            player
                .post_to("/api/random/start", content_type, body)
                .await
                .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        }
        // The session is as it was, and nothing was drawn.
        assert_eq!(stored(&harness, &player).await, before);
        assert_eq!(harness.deezer.api_hits(), hits);

        // Nor does such a request begin a session, for a player whose last
        // one has ended...
        harness.clock.advance_minutes(IDLE_MINUTES);
        for body in &bodies {
            player
                .random_start_with(body.clone())
                .await
                .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        }
        assert_eq!(stored(&harness, &player).await, before);
        player
            .random()
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_game");

        // ...or for a browser the server has never seen: the request is
        // read before anyone is asked who is playing, and no ID is issued.
        let mut stranger = harness.player();
        let reply = stranger.random_start_with(json!({ "pool": "jazz" })).await;
        reply.assert_error(StatusCode::BAD_REQUEST, "bad_request");
        assert!(reply.headers.get(header::SET_COOKIE).is_none());
        assert!(stranger.cookie.is_none());
        harness
            .post("/api/random/start", Some(json!(["rock"])))
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
    }

    #[test]
    fn a_start_names_one_pool_or_none() {
        for body in [
            "",
            "  \n",
            "{}",
            r#"{"pool":null}"#,
            r#"{"other":1}"#,
            r#"{"section":"rock"}"#,
        ] {
            assert_eq!(chosen_pool(body.as_bytes()), Some(None), "{body:?}");
        }
        for pool in Section::ALL {
            let body = json!({ "pool": pool, "round": 3 }).to_string();
            assert_eq!(chosen_pool(body.as_bytes()), Some(Some(pool)), "{body}");
        }
        assert_eq!(
            chosen_pool(br#"{"pool":"hip-hop"}"#),
            Some(Some(HIP_HOP)),
            "the slug of the section, as in its address"
        );
        for body in [
            r#"{"pool":"jazz"}"#,
            r#"{"pool":"Rock"}"#,
            r#"{"pool":["pop"]}"#,
            r#"{"pool":0}"#,
            r#"["pop"]"#,
            r#""pop""#,
            "null",
            "{",
            "rock",
        ] {
            assert_eq!(chosen_pool(body.as_bytes()), None, "{body:?}");
        }
    }

    #[tokio::test]
    async fn a_session_begun_after_the_last_one_ended_starts_from_zero_and_keeps_the_best_run() {
        let harness = start(&[ALPHA, BETA, GAMMA, DELTA]).await;
        let (mut player, first) = started(&harness).await;
        let second = win_and_go_on(&harness, &mut player, 1, first).await;
        let reply = player.random_guess(2, second).await;
        let body = reply.json();
        assert_eq!(body["run"], 2);
        assert_eq!(body["bestRun"], 2);

        // Half an hour away, and a new session: the round goes on counting,
        // the score does not.
        harness.clock.advance_minutes(30);
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

        // A session that ends in the middle of a song abandons it: it is
        // neither played nor lost, and the song after it is the one played
        // longest ago.
        assert_eq!(player.random_skip(3).await.status, StatusCode::OK);
        harness.clock.advance_minutes(30);
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
        // And in every session that follows.
        for session in 1..=4 {
            harness.clock.advance_minutes(30);
            let reply = player.random_start().await;
            assert_eq!(reply.json()["round"], 7 + session, "{}", reply.visible());
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
        // recent, so the next session (half an hour later) draws it first,
        // finds it has no preview, marks it and falls back on the song it
        // knows.
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
        harness.clock.advance_minutes(30);
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

    // --- how long a session lasts, on the routes --------------------------------------

    /// Minutes without playing after which a session is over.
    const IDLE_MINUTES: i64 = 30;

    #[tokio::test]
    async fn a_session_not_played_for_half_an_hour_is_no_game_to_any_route_but_start() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let (mut player, first) = started(&harness).await;
        assert_eq!(player.random_skip(1).await.status, StatusCode::OK);
        let before = stored(&harness, &player).await;
        let id = harness.player_id(&player);

        // A minute short of it, the session is there to look at and listen to.
        harness.clock.advance_minutes(IDLE_MINUTES - 1);
        let reply = player.random().await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_hides(&reply, first);
        assert_eq!(player.random_audio().await.status, StatusCode::OK);
        // Neither counted as playing: nothing was written.
        assert_eq!(stored(&harness, &player).await, before);

        // So a minute later it is over, to every route that needs a session.
        harness.clock.advance_minutes(1);
        for reply in [
            player.random().await,
            player.random_audio().await,
            player.random_skip(1).await,
            player.random_guess(1, QUEEN).await,
            // Not even the right answer plays in a session that has ended.
            player.random_guess(1, first).await,
            player.random_next(1).await,
        ] {
            reply.assert_error(StatusCode::NOT_FOUND, "no_game");
            assert_hides(&reply, first);
            assert!(reply.headers.get(header::SET_COOKIE).is_none());
        }
        // Nothing was written: the record stays for the next session to
        // carry over from, and the browser is who it was.
        assert_eq!(stored(&harness, &player).await, before);
        assert_eq!(harness.player_id(&player), id);

        // It stays over, however much later it is looked at.
        harness.clock.advance_minutes(3 * 24 * 60);
        player
            .random()
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_game");
    }

    #[tokio::test]
    async fn a_move_and_a_next_song_keep_the_session_going() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let (mut player, first) = started(&harness).await;
        let began = harness.clock.now().as_second();
        assert_eq!(stored(&harness, &player).await.active_at(), began);

        // Each is made a minute before the session would have ended, and
        // each gives it another half hour.
        harness.clock.advance_minutes(IDLE_MINUTES - 1);
        assert_eq!(player.random_skip(1).await.status, StatusCode::OK);
        harness.clock.advance_minutes(IDLE_MINUTES - 1);
        assert_eq!(player.random_guess(1, QUEEN).await.status, StatusCode::OK);
        harness.clock.advance_minutes(IDLE_MINUTES - 1);
        let reply = player.random_guess(1, first).await;
        assert_eq!(reply.json()["status"], "won", "{}", reply.visible());
        harness.clock.advance_minutes(IDLE_MINUTES - 1);
        let reply = player.random_next(1).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json()["round"], 2);
        // The same session all along: the win is still its run.
        assert_eq!(reply.json()["run"], 1);
        let game = stored(&harness, &player).await;
        assert_eq!(
            game.active_at(),
            began + 4 * (IDLE_MINUTES - 1) * 60,
            "the next song was the last time it was played"
        );

        // A move that is refused is not playing: a stale round, a move that
        // is not understood, an unknown track.
        harness.clock.advance_minutes(IDLE_MINUTES - 1);
        player
            .random_skip(1)
            .await
            .assert_error(StatusCode::CONFLICT, "changed");
        player
            .random_move(json!({ "round": 2 }))
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        player
            .random_guess(2, UNKNOWN_ID)
            .await
            .assert_error(StatusCode::NOT_FOUND, "unknown_track");
        player
            .random_next(2)
            .await
            .assert_error(StatusCode::CONFLICT, "unfinished");
        assert_eq!(stored(&harness, &player).await, game);
        harness.clock.advance_minutes(1);
        player
            .random_skip(2)
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_game");
    }

    #[tokio::test]
    async fn a_clock_set_back_does_not_end_the_session() {
        let harness = start(&[ALPHA, BETA]).await;
        let (mut player, _) = started(&harness).await;
        harness.clock.advance_minutes(-90);
        assert_eq!(player.random().await.status, StatusCode::OK);
        assert_eq!(player.random_skip(1).await.status, StatusCode::OK);
        // The admin's day offset is another clock altogether: a simulated
        // next day is not a day without playing.
        let reply = harness.post("/api/admin/next-day", None).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(player.random().await.status, StatusCode::OK);
        assert_eq!(player.random_skip(1).await.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn starting_while_a_session_is_going_joins_it_and_draws_nothing() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let (mut player, first) = started(&harness).await;
        let id = harness.player_id(&player).unwrap();
        let reply = player.random_guess(1, QUEEN).await;
        let after_the_guess = reply.json();
        let before = stored(&harness, &player).await;
        let hits = harness.deezer.api_hits();

        // Another browser tab of the same player opens random mode, ten
        // minutes into the session.
        harness.clock.advance_minutes(10);
        let mut tab = harness.player_with_id(&id);
        for _ in 0..3 {
            let reply = tab.random_start().await;
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
            reply.assert_no_store();
            assert_hides(&reply, first);
            // The session as it stands: the same song, the try already used.
            assert_eq!(reply.json(), after_the_guess);
            assert_eq!(reply.json()["round"], 1);
            // The cookie is set again, for the same player.
            let cookie = reply.header(header::SET_COOKIE);
            assert!(cookie.contains("Max-Age=34560000"), "{cookie}");
            assert_eq!(harness.player_id(&tab), Some(id.clone()));
        }
        // Nothing was drawn and nothing was written.
        assert_eq!(harness.deezer.api_hits(), hits);
        assert_eq!(stored(&harness, &player).await, before);

        // Both tabs play the one session: a move in one is seen by the other.
        assert_eq!(tab.random_skip(1).await.status, StatusCode::OK);
        assert_eq!(
            player.random().await.json()["attempts"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        // Joining is not playing: the half hour runs from the last move.
        harness.clock.advance_minutes(IDLE_MINUTES - 1);
        assert_eq!(player.random_start().await.json()["round"], 1);
        harness.clock.advance_minutes(1);
        let reply = player.random_start().await;
        assert_eq!(reply.json()["round"], 2, "{}", reply.visible());
    }

    #[tokio::test]
    async fn a_record_from_before_sessions_ended_is_no_game_and_is_carried_over() {
        let harness = start(&[ALPHA, BETA, GAMMA]).await;
        let mut player = harness.player();
        assert_eq!(player.get("/api/today").await.status, StatusCode::OK);
        let id = harness.player_id(&player).unwrap();
        // A game stored by the server as it was before: no time in it.
        let old: RandomGame = serde_json::from_value(json!({
            "round": 8,
            "track_id": BETA,
            "game": { "day": "2026-10-01", "attempts": [], "status": "won" },
            "run": 3,
            "played": 4,
            "won": 3,
            "best_run": 6,
            "recent": [GAMMA, BETA],
        }))
        .unwrap();
        harness.store.save_random_game(&id, &old).await.unwrap();

        for reply in [
            player.random().await,
            player.random_audio().await,
            player.random_next(8).await,
        ] {
            reply.assert_error(StatusCode::NOT_FOUND, "no_game");
        }
        assert_eq!(stored(&harness, &player).await, old);

        let reply = player.random_start().await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        let body = reply.json();
        assert_eq!(body["round"], 9);
        assert_eq!(body["bestRun"], 6);
        assert_eq!(
            (&body["run"], &body["played"], &body["won"]),
            (&json!(0), &json!(0), &json!(0))
        );
        // Both songs it may draw are recent: the one played longest ago.
        let game = stored(&harness, &player).await;
        assert_eq!(game.track_id(), GAMMA);
        assert_eq!(game.recent(), [BETA, GAMMA]);
        assert_eq!(game.active_at(), harness.clock.now().as_second());
        assert_hides(&reply, GAMMA);
    }

    #[tokio::test]
    async fn starts_sent_at_once_after_a_session_ended_all_end_up_in_one_session() {
        let store = Arc::new(Unhurried(MemoryStore::new()));
        let harness = over(&[ALPHA, BETA, GAMMA, DELTA, EPSILON], store).await;
        let (mut player, first) = started(&harness).await;
        let id = harness.player_id(&player).unwrap();
        assert_eq!(player.random_guess(1, first).await.json()["status"], "won");
        harness.clock.advance_minutes(IDLE_MINUTES);

        // Five browser tabs come back at the same moment. Each finds the
        // session over and draws a song before any of them has begun the
        // next one: if each then began its own, every tab but the last
        // would be left in a session another tab has ended.
        let requests: Vec<_> = (0..5)
            .map(|_| {
                let mut tab = harness.player_with_id(&id);
                tokio::spawn(async move { tab.random_start().await })
            })
            .collect();
        let mut replies = Vec::new();
        for request in requests {
            replies.push(request.await.unwrap());
        }

        let game = stored(&harness, &player).await;
        // One session was begun, on one song: the round moved on by one.
        assert_eq!(game.round(), 2);
        assert_eq!((game.run(), game.played(), game.won()), (0, 0, 0));
        assert_eq!(game.best_run(), 1);
        assert_eq!(game.recent(), [first, game.track_id()]);
        for reply in &replies {
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
            assert_hides(reply, game.track_id());
            let body = reply.json();
            assert_eq!(body["round"], 2);
            assert_eq!(body["attempts"], json!([]));
            assert_eq!(body["run"], 0);
            assert_eq!(body["bestRun"], 1);
        }
        // And it is the session every tab plays in from here.
        assert_eq!(player.random_skip(2).await.status, StatusCode::OK);
        let mut tab = harness.player_with_id(&id);
        assert_eq!(tab.random_start().await.json()["round"], 2);
    }

    // --- the pool the songs are drawn from, on the routes --------------------------------

    #[tokio::test]
    async fn a_first_session_draws_from_the_pool_the_start_chose_and_from_all_of_it_without_one() {
        let harness = genres().await;

        // A browser the server has never seen, asking for rock: it is given
        // an ID, a session and a rock song.
        let mut player = harness.player();
        let reply = player.random_start_from(ROCK).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        reply.assert_no_store();
        let game = stored(&harness, &player).await;
        assert!(ROCK_SONGS.contains(&game.track_id()));
        assert_eq!(game.pool(), ROCK);
        assert_hides(&reply, game.track_id());
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
                "pool": "rock",
            })
        );
        let id = harness.player_id(&player).expect("a cookie was set");
        assert_eq!(harness.cookie_plaintext(&player), Some(id.to_string()));

        // Each of the pools, for a newcomer each: the song is one of that
        // pool's, every time.
        for _ in 0..8 {
            let (_, track_id) = started_from(&harness, POP).await;
            assert!(POP_SONGS.contains(&track_id), "{track_id}");
            let (_, track_id) = started_from(&harness, ROCK).await;
            assert!(ROCK_SONGS.contains(&track_id), "{track_id}");
        }
        // All of it, when the start says so and when it says nothing: over
        // many newcomers, the songs of every genre and of none.
        let mut drawn = std::collections::BTreeSet::new();
        for turn in 0..60 {
            let (player, track_id) = if turn % 2 == 0 {
                started_from(&harness, Section::General).await
            } else {
                started(&harness).await
            };
            assert_eq!(stored(&harness, &player).await.pool(), Section::General);
            drawn.insert(track_id);
        }
        assert_eq!(
            drawn.into_iter().collect::<Vec<_>>(),
            [ALPHA, BETA, GAMMA, DELTA, EPSILON]
        );
    }

    #[tokio::test]
    async fn a_song_a_section_plays_today_is_not_drawn_from_its_genre_either() {
        let pool: [(u64, &[Genre]); 5] = [
            (ALPHA, &[Genre::Pop]),
            (BETA, &[Genre::Pop]),
            (GAMMA, &[Genre::Rock]),
            (DELTA, &[Genre::Rock]),
            (EPSILON, &[]),
        ];
        let harness = Harness::with_pool(tracks(), &pool).await;
        // Pop plays one of its own, and General took a rock song.
        for (section, track_id) in [
            (POP, ALPHA),
            (Section::General, GAMMA),
            (ROCK, FILLER),
            (HIP_HOP, FILLER + 1),
        ] {
            let pick = Pick {
                day: TODAY,
                section,
                track_id,
            };
            harness.store.save_pick(pick).await.unwrap();
        }

        // One pop song is left, and it is the one every time, recent or not.
        let (mut player, first) = started_from(&harness, POP).await;
        assert_eq!(first, BETA);
        for round in 1..=3 {
            let next = win_and_go_on(&harness, &mut player, round, BETA).await;
            assert_eq!(next, BETA);
        }
        // The same for rock, whichever section it is that plays the other.
        let reply = player.random_guess(4, BETA).await;
        assert_eq!(reply.json()["status"], "won", "{}", reply.visible());
        assert_eq!(player.random_start_from(ROCK).await.json()["pool"], "rock");
        for round in 4..=6 {
            let reply = player.random_next(round).await;
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
            assert_eq!(stored(&harness, &player).await.track_id(), DELTA);
            assert_hides(&reply, DELTA);
            let reply = player.random_guess(round + 1, DELTA).await;
            assert_eq!(reply.json()["status"], "won", "{}", reply.visible());
        }
    }

    #[tokio::test]
    async fn a_choice_in_a_session_that_is_going_changes_the_pool_and_nothing_else() {
        let harness = genres().await;
        let (mut player, track_id) = started_from(&harness, POP).await;
        let id = harness.player_id(&player).unwrap();
        assert_eq!(player.random_skip(1).await.status, StatusCode::OK);
        let reply = player.random_guess(1, QUEEN).await;
        let after_the_guess = reply.json();
        assert_eq!(after_the_guess["pool"], "pop");
        let before = stored(&harness, &player).await;
        let clip = player.random_audio().await.body;
        let hits = harness.deezer.api_hits();

        // Ten minutes into the song, the player asks for rock.
        harness.clock.advance_minutes(10);
        let reply = player.random_start_from(ROCK).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        reply.assert_no_store();
        assert_hides(&reply, track_id);
        let cookie = reply.header(header::SET_COOKIE);
        assert!(cookie.contains("Max-Age=34560000"), "{cookie}");
        assert_eq!(harness.player_id(&player), Some(id.clone()));

        // The view is the one of before, but for the pool: the same song
        // with its two tries used, the same round, the same score.
        let mut expected = after_the_guess.clone();
        expected["pool"] = json!("rock");
        assert_eq!(reply.json(), expected);
        assert_eq!(reply.json()["round"], 1);
        assert_eq!(reply.json()["attempts"].as_array().unwrap().len(), 2);
        assert_eq!(reply.json()["clipSeconds"], 1.0);
        // And so is the record: the pop song is still the one being played,
        // and the session was last played when the guess was made.
        let mut changed = before.clone();
        changed.draw_from(ROCK);
        let after = stored(&harness, &player).await;
        assert_eq!(after, changed);
        assert_eq!(after.track_id(), track_id);
        assert_eq!(after.active_at(), before.active_at());
        // Nothing was drawn, and the clip is the one of before.
        assert_eq!(harness.deezer.api_hits(), hits);
        assert_eq!(player.random_audio().await.body, clip);

        // Every other route shows the new pool, in this tab and in another.
        let mut tab = harness.player_with_id(&id);
        for reply in [
            player.random().await,
            tab.random().await,
            // A start without a choice joins the session and keeps the pool.
            tab.random_start().await,
            tab.random_start_with(json!({})).await,
            // And so does one that asks for the pool it has.
            tab.random_start_from(ROCK).await,
        ] {
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
            assert_eq!(reply.json(), expected);
            assert_hides(&reply, track_id);
        }
        assert_eq!(stored(&harness, &player).await, changed);
        assert_eq!(harness.deezer.api_hits(), hits);

        // Choosing is not playing: the half hour runs from the guess. A
        // minute before it is up the session is still there, and a choice
        // made then does not put the end off either.
        harness.clock.advance_minutes(IDLE_MINUTES - 11);
        assert_eq!(player.random().await.status, StatusCode::OK);
        let reply = tab.random_start_from(Section::General).await;
        assert_eq!(reply.json()["pool"], "general", "{}", reply.visible());
        assert_eq!(reply.json()["round"], 1);
        assert_eq!(
            stored(&harness, &player).await.active_at(),
            before.active_at()
        );
        harness.clock.advance_minutes(1);
        for reply in [
            player.random().await,
            player.random_audio().await,
            player.random_skip(1).await,
        ] {
            reply.assert_error(StatusCode::NOT_FOUND, "no_game");
        }
    }

    #[tokio::test]
    async fn after_a_change_the_song_stays_and_the_next_one_is_the_first_of_the_new_pool() {
        let harness = genres().await;
        let (mut player, first) = started_from(&harness, POP).await;
        assert!(POP_SONGS.contains(&first));
        let reply = player.random_skip(1).await;
        assert_eq!(reply.json()["pool"], "pop");
        assert_hides(&reply, first);

        // Rock, in the middle of the pop song: it is no way out of the song.
        // It is still there to be guessed, with the try it cost so far...
        let reply = player.random_start_from(ROCK).await;
        assert_eq!(reply.json()["round"], 1);
        assert_eq!(reply.json()["status"], "playing");
        assert_eq!(reply.json()["attempts"], json!([{ "kind": "skip" }]));
        assert_hides(&reply, first);
        // ...the next cannot be asked for...
        player
            .random_next(1)
            .await
            .assert_error(StatusCode::CONFLICT, "unfinished");
        // ...and a miss costs what it always costs.
        let reply = player.random_guess(1, QUEEN).await;
        assert_eq!(reply.json()["attempts"].as_array().unwrap().len(), 2);
        assert_eq!(reply.json()["pool"], "rock");
        assert_hides(&reply, first);

        // Won, it counts for the session, whose pool is rock by now.
        let reply = player.random_guess(1, first).await;
        let body = reply.json();
        assert_eq!(body["status"], "won");
        assert_eq!(body["pool"], "rock");
        assert_eq!(body["run"], 1);
        let title = SONGS.iter().find(|song| song.0 == first).unwrap().1;
        assert_eq!(body["answer"]["title"], title);

        // The next song is the first to be drawn from rock, and the run
        // goes on across the genres.
        let reply = player.random_next(1).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        let second = stored(&harness, &player).await.track_id();
        assert!(ROCK_SONGS.contains(&second), "{second}");
        assert_hides(&reply, second);
        let body = reply.json();
        assert_eq!(body["round"], 2);
        assert_eq!(body["pool"], "rock");
        assert_eq!(body["status"], "playing");
        assert_eq!((&body["run"], &body["played"]), (&json!(1), &json!(1)));

        // Rock goes on being what is drawn: the other rock song, then the
        // first again, the one of them played longer ago, and never the pop
        // song or the untagged one that were not played lately.
        let third = win_and_go_on(&harness, &mut player, 2, second).await;
        assert_eq!(third, GAMMA + DELTA - second);
        let fourth = win_and_go_on(&harness, &mut player, 3, third).await;
        assert_eq!(fourth, second);
        let reply = player.random().await;
        assert_eq!(reply.json()["pool"], "rock");
        assert_eq!(reply.json()["run"], 3);
        assert_hides(&reply, fourth);

        // Back to all of it, on a finished song: the song stays finished
        // and revealed, and the one after it is one the player has not had.
        let reply = player.random_guess(4, fourth).await;
        assert_eq!(reply.json()["status"], "won");
        let reply = player.random_start_from(Section::General).await;
        let body = reply.json();
        assert_eq!(body["round"], 4);
        assert_eq!(body["status"], "won");
        assert_eq!(body["pool"], "general");
        assert_eq!(body["run"], 4);
        let title = SONGS.iter().find(|song| song.0 == fourth).unwrap().1;
        assert_eq!(body["answer"]["title"], title);
        let reply = player.random_next(4).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json()["round"], 5);
        assert_eq!(reply.json()["pool"], "general");
        let fifth = stored(&harness, &player).await.track_id();
        assert!(fifth == EPSILON || fifth == ALPHA + BETA - first, "{fifth}");
        assert_hides(&reply, fifth);
        let game = stored(&harness, &player).await;
        assert_eq!(game.pool(), Section::General);
        assert_eq!((game.run(), game.played(), game.won()), (4, 4, 4));
        assert_eq!(game.best_run(), 4);
    }

    #[tokio::test]
    async fn the_pool_carries_over_to_the_next_session_unless_the_start_chooses_another() {
        let harness = genres().await;
        let (mut player, first) = started_from(&harness, ROCK).await;
        assert!(ROCK_SONGS.contains(&first));

        // Half an hour away, and a start that says nothing: a new session,
        // and rock again, like the longest run.
        harness.clock.advance_minutes(IDLE_MINUTES);
        let reply = player.random_start().await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json()["round"], 2);
        assert_eq!(reply.json()["pool"], "rock");
        let second = stored(&harness, &player).await.track_id();
        assert_eq!(second, GAMMA + DELTA - first);
        assert_hides(&reply, second);

        // A start that chooses begins the next session in that pool.
        harness.clock.advance_minutes(IDLE_MINUTES);
        let reply = player.random_start_from(POP).await;
        assert_eq!(reply.json()["round"], 3, "{}", reply.visible());
        assert_eq!(reply.json()["pool"], "pop");
        assert_eq!(reply.json()["attempts"], json!([]));
        let third = stored(&harness, &player).await.track_id();
        assert!(POP_SONGS.contains(&third), "{third}");
        assert_hides(&reply, third);

        // Which is then the one that carries over, for a body that names no
        // pool as for no body.
        harness.clock.advance_minutes(IDLE_MINUTES);
        let reply = player.random_start_with(json!({ "pool": null })).await;
        assert_eq!(reply.json()["round"], 4, "{}", reply.visible());
        assert_eq!(reply.json()["pool"], "pop");
        let game = stored(&harness, &player).await;
        assert_eq!(game.track_id(), ALPHA + BETA - third);
        assert_eq!(game.pool(), POP);

        // And all of it can be chosen as any other pool can.
        harness.clock.advance_minutes(IDLE_MINUTES);
        let reply = player.random_start_from(Section::General).await;
        assert_eq!(reply.json()["round"], 5, "{}", reply.visible());
        assert_eq!(reply.json()["pool"], "general");
        // The one song not played lately, which has no genre.
        let game = stored(&harness, &player).await;
        assert_eq!(game.track_id(), EPSILON);
        assert_eq!(game.pool(), Section::General);
        assert_hides(&reply, EPSILON);
    }

    #[tokio::test]
    async fn a_record_from_before_the_pool_could_be_chosen_draws_from_all_of_it() {
        let harness = genres().await;
        let mut player = harness.player();
        assert_eq!(player.get("/api/today").await.status, StatusCode::OK);
        let id = harness.player_id(&player).unwrap();
        // A session stored by the server as it was before, still going: it
        // has no pool.
        let old: RandomGame = serde_json::from_value(json!({
            "round": 3,
            "track_id": ALPHA,
            "game": { "day": "2026-10-01", "attempts": [{ "kind": "skip" }], "status": "playing" },
            "run": 2,
            "played": 2,
            "won": 2,
            "best_run": 2,
            "recent": [BETA, GAMMA, DELTA, ALPHA],
            "active_at": harness.clock.now().as_second(),
        }))
        .unwrap();
        harness.store.save_random_game(&id, &old).await.unwrap();

        for reply in [player.random().await, player.random_start().await] {
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
            assert_eq!(reply.json()["pool"], "general");
            assert_eq!(reply.json()["round"], 3);
            assert_hides(&reply, ALPHA);
        }
        // Its next song is drawn from everything: the one song not played
        // lately has no genre at all.
        let next = win_and_go_on(&harness, &mut player, 3, ALPHA).await;
        assert_eq!(next, EPSILON);
        assert_eq!(player.random().await.json()["pool"], "general");

        // And when it has ended, the session after it does the same.
        harness.clock.advance_minutes(IDLE_MINUTES);
        harness.store.save_random_game(&id, &old).await.unwrap();
        let reply = player.random_start().await;
        assert_eq!(reply.json()["round"], 4, "{}", reply.visible());
        assert_eq!(reply.json()["pool"], "general");
        assert_eq!(stored(&harness, &player).await.track_id(), EPSILON);
    }

    #[tokio::test]
    async fn a_genre_without_songs_has_no_song_and_the_record_is_left_alone() {
        let harness = genres().await;

        // A newcomer who asks for hip-hop, of which there is none: no song,
        // no session and no cookie.
        let mut newcomer = harness.player();
        let reply = newcomer.random_start_from(HIP_HOP).await;
        reply.assert_error(StatusCode::NOT_FOUND, "no_song");
        assert!(reply.headers.get(header::SET_COOKIE).is_none());
        assert!(newcomer.cookie.is_none());
        // The other pools are there for them.
        assert_eq!(newcomer.random_start_from(POP).await.status, StatusCode::OK);

        // A player whose rock session has ended: the record stays as it is,
        // pool and all, and the session is still over.
        let (mut player, first) = started_from(&harness, ROCK).await;
        assert_eq!(player.random_guess(1, first).await.json()["status"], "won");
        harness.clock.advance_minutes(IDLE_MINUTES);
        let before = stored(&harness, &player).await;
        let reply = player.random_start_from(HIP_HOP).await;
        reply.assert_error(StatusCode::NOT_FOUND, "no_song");
        assert_hides_all(&reply);
        assert_eq!(stored(&harness, &player).await, before);
        player
            .random()
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_game");
        // So a start without a choice is in rock, as the last session was.
        let reply = player.random_start().await;
        assert_eq!(reply.json()["round"], 2, "{}", reply.visible());
        assert_eq!(reply.json()["pool"], "rock");
        assert_eq!(reply.json()["bestRun"], 1);
        let second = stored(&harness, &player).await.track_id();
        assert_eq!(second, GAMMA + DELTA - first);

        // In a session that is going, the choice is taken: nothing is drawn
        // for it, so there is nothing to refuse yet.
        let reply = player.random_start_from(HIP_HOP).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        assert_eq!(reply.json()["pool"], "hip-hop");
        assert_eq!(reply.json()["round"], 2);
        assert_hides(&reply, second);
        let reply = player.random_guess(2, second).await;
        assert_eq!(reply.json()["status"], "won");
        assert_eq!(reply.json()["pool"], "hip-hop");

        // It is the next song that cannot be had. The finished song, the
        // score and the pool stay as they are...
        let before = stored(&harness, &player).await;
        for _ in 0..2 {
            let reply = player.random_next(2).await;
            reply.assert_error(StatusCode::NOT_FOUND, "no_song");
            assert_eq!(stored(&harness, &player).await, before);
        }
        let reply = player.random().await;
        assert_eq!(reply.json()["status"], "won");
        assert_eq!(reply.json()["pool"], "hip-hop");
        // ...until the player chooses a pool that has songs, and asks again.
        let reply = player.random_start_from(POP).await;
        let body = reply.json();
        assert_eq!(body["round"], 2);
        assert_eq!(body["status"], "won");
        assert_eq!(body["pool"], "pop");
        let reply = player.random_next(2).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        let body = reply.json();
        assert_eq!(body["round"], 3);
        assert_eq!(body["pool"], "pop");
        assert_eq!(body["run"], 1);
        let third = stored(&harness, &player).await.track_id();
        assert!(POP_SONGS.contains(&third), "{third}");
        assert_hides(&reply, third);
    }

    /// The anti-leak rule for a response that is about no song in
    /// particular: it names none of the pool's.
    fn assert_hides_all(reply: &Reply) {
        for (track_id, ..) in SONGS {
            assert_hides(reply, track_id);
        }
    }

    #[tokio::test]
    async fn a_change_of_pool_sent_with_moves_loses_neither() {
        let store = Arc::new(Unhurried(MemoryStore::new()));
        let harness = genres_over(store).await;
        let (player, track_id) = started_from(&harness, POP).await;
        let id = harness.player_id(&player).unwrap();

        // Three skips and two changes of pool, all read before any is
        // written if nothing keeps them apart: a change that stored the
        // game it had read would take a skip back, and a skip would do the
        // same to the change.
        let mut requests = Vec::new();
        for turn in 0..5 {
            let mut tab = harness.player_with_id(&id);
            requests.push(tokio::spawn(async move {
                if turn % 2 == 0 {
                    tab.random_skip(1).await
                } else {
                    tab.random_start_from(ROCK).await
                }
            }));
        }
        for request in requests {
            let reply = request.await.unwrap();
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
            assert_eq!(reply.json()["round"], 1);
            assert_hides(&reply, track_id);
        }

        let game = stored(&harness, &player).await;
        assert_eq!(game.game().attempts().len(), 3);
        assert_eq!(game.pool(), ROCK);
        assert_eq!(game.track_id(), track_id);
    }

    #[tokio::test]
    async fn starts_sent_at_once_with_different_choices_end_up_in_one_session() {
        let store = Arc::new(Unhurried(MemoryStore::new()));
        let harness = genres_over(store).await;
        let (mut player, first) = started_from(&harness, POP).await;
        let id = harness.player_id(&player).unwrap();
        assert_eq!(player.random_guess(1, first).await.json()["status"], "won");
        harness.clock.advance_minutes(IDLE_MINUTES);

        // Five tabs come back at the same moment, each finds the session
        // over and draws a song, three of them with a pool of their own in
        // mind. One of them begins the session; the others join it, and a
        // choice that came with a joining start is made in that session.
        let choices = [Some(ROCK), None, Some(Section::General), None, Some(ROCK)];
        let requests: Vec<_> = choices
            .into_iter()
            .map(|choice| {
                let mut tab = harness.player_with_id(&id);
                tokio::spawn(async move {
                    let reply = match choice {
                        Some(pool) => tab.random_start_from(pool).await,
                        None => tab.random_start().await,
                    };
                    (choice, reply)
                })
            })
            .collect();
        let mut replies = Vec::new();
        for request in requests {
            replies.push(request.await.unwrap());
        }

        // One session was begun, on one song.
        let game = stored(&harness, &player).await;
        assert_eq!(game.round(), 2);
        assert_eq!((game.run(), game.played(), game.won()), (0, 0, 0));
        assert_eq!(game.best_run(), 1);
        assert_eq!(game.recent(), [first, game.track_id()]);
        for (choice, reply) in &replies {
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
            assert_hides(reply, game.track_id());
            let body = reply.json();
            assert_eq!(body["round"], 2);
            assert_eq!(body["attempts"], json!([]));
            // Whoever chose was answered with the pool they chose, whether
            // they began the session or joined it.
            if let Some(pool) = choice {
                assert_eq!(body["pool"], pool.slug(), "{}", reply.visible());
            }
        }
        // The pool it is left with is one of those asked for, or the one
        // that carried over, and every tab sees the same from here.
        assert!([ROCK, Section::General, POP].contains(&game.pool()));
        let mut tab = harness.player_with_id(&id);
        let reply = tab.random_start().await;
        assert_eq!(reply.json()["round"], 2);
        assert_eq!(reply.json()["pool"], game.pool().slug());
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
                "pool": "general",
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

        // A new session, begun in one tab once the last has ended, is the
        // same to the other.
        harness.clock.advance_minutes(30);
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
            player.random_start_from(POP).await,
            player.random_skip(1).await,
            player.random_guess(1, QUEEN).await,
            player.random_next(1).await,
            // A new player's first request, too.
            harness.player().random_start().await,
            harness.player().random_start_from(ROCK).await,
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
