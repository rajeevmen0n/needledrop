//! The game's HTTP handlers and the state every handler shares.
//!
//! The admin routes live in [`crate::admin`], random mode in
//! [`crate::random`] and the player cookie and Clear my data in
//! [`crate::player`]; [`router`] mounts them all.
//!
//! The rule every handler here keeps: while a section's game is `playing`,
//! nothing that identifies that section's song leaves the server. The answer
//! goes out through one place only ([`DailyView::new`]; random mode has one of
//! its own), error messages are fixed sentences, the cookie holds an
//! anonymous ID and nothing about the game, and the audio is the clip the
//! player has unlocked and no more.
//!
//! There are four games a day, one per [`Section`], each with its own song
//! ([`crate::daily`]). A game is kept in the store under the player's ID, its
//! section and its day. It is written when a move is made and never by a
//! visit: a player who has made no move today has, by definition, a fresh
//! game, so a crawler or a visitor who only looks leaves nothing behind.
//!
//! The day is the server's day ([`Daily::today`]), read once per request and
//! passed down.

use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{
        FromRef, Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use axum_extra::extract::cookie::{Key, PrivateCookieJar};
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::{
    admin,
    daily::{Answer, Daily, DailyError, Playing, Song, Standing},
    deezer::{Deezer, DeezerError, Track},
    game::{self, Attempt, GameError, GameState, MAX_ATTEMPTS, Status, TrackMeta},
    pick,
    player::{self, MoveLocks},
    random,
    stats::Stats,
    store::{PlayerId, Section, Store, StoreError},
};

/// Autocomplete rows sent to the client.
const SEARCH_RESULTS: usize = 8;

/// Tracks asked of Deezer per search. More than [`SEARCH_RESULTS`], because
/// several of them are usually the same song in another release.
const SEARCH_FETCH: u32 = 25;

/// Shortest query worth sending to Deezer, in characters.
const MIN_QUERY_CHARS: usize = 2;

/// Longest query passed on; the rest is cut off. Nobody types a longer title
/// into autocomplete.
const MAX_QUERY_CHARS: usize = 100;

/// How often a request that only looks fetches the section's song again when
/// the song was replaced under it (see [`standing_song`]). Each further try
/// takes another re-roll or reset landing in the same few milliseconds.
const STANDING_TRIES: usize = 3;

/// What every handler shares. Cheap to clone.
#[derive(Clone)]
pub struct AppState(Arc<Shared>);

struct Shared {
    deezer: Deezer,
    daily: Daily,
    store: Arc<dyn Store>,
    move_locks: MoveLocks,
    launch_date: Date,
    key: Key,
}

impl AppState {
    pub fn new(
        deezer: Deezer,
        daily: Daily,
        store: Arc<dyn Store>,
        launch_date: Date,
        key: Key,
    ) -> Self {
        Self(Arc::new(Shared {
            deezer,
            daily,
            store,
            move_locks: MoveLocks::new(),
            launch_date,
            key,
        }))
    }

    /// The Deezer client every handler shares, with its caches and its
    /// request budget.
    pub fn deezer(&self) -> &Deezer {
        &self.0.deezer
    }

    /// The persistent data: the song pool, the players' games, the picks and
    /// the day offset.
    pub fn store(&self) -> &dyn Store {
        self.0.store.as_ref()
    }

    /// The locks that keep each player's moves one after another.
    pub fn move_locks(&self) -> &MoveLocks {
        &self.0.move_locks
    }

    /// The day, its songs and the operations that change them.
    pub fn daily(&self) -> &Daily {
        &self.0.daily
    }

    /// Day 1 of the game.
    pub fn launch_date(&self) -> Date {
        self.0.launch_date
    }

    /// The server's day. Read it once per request and pass it down.
    pub async fn today(&self) -> Result<Date, ApiError> {
        self.0.daily.today().await.map_err(daily_failed)
    }

    /// The song `section` plays on `day`, picking and loading it if needed.
    pub async fn song(&self, section: Section, day: Date) -> Result<Playing, ApiError> {
        self.0.daily.song(day, section).await.map_err(daily_failed)
    }

    /// Picks and loads today's songs, so that the first player does not wait
    /// for it. Failures are logged where they happen and retried by the
    /// requests that follow.
    pub async fn warm_up(&self) {
        match self.0.daily.today().await {
            Ok(today) => self.0.daily.warm(today).await,
            Err(error) => tracing::error!(%error, "could not read the day; no song was loaded"),
        }
    }
}

/// Lets [`PrivateCookieJar`] find the key it decrypts and encrypts with.
impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.0.key.clone()
    }
}

/// Every route the server exposes, all under `/api`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/today", get(today))
        .route("/api/daily/{section}", get(daily))
        .route("/api/daily/{section}/audio", get(audio))
        .route("/api/daily/{section}/guess", post(guess))
        .route("/api/player", delete(player::clear))
        .route("/api/search", get(search))
        .merge(random::routes())
        .merge(admin::routes())
        .fallback(not_found)
        .with_state(state)
}

// --- errors -------------------------------------------------------------------

/// A failed request, sent as `{ "error": "<code>", "message": "<sentence>" }`.
///
/// The messages are fixed text on purpose: an upstream error can carry a URL
/// or a track ID, and none of that may reach a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiError {
    /// 400: the request is not one the route understands.
    BadRequest(&'static str),
    /// 404: Deezer has no track with the given ID.
    UnknownTrack,
    /// 404: the song pool has no song with the given track ID.
    UnknownSong,
    /// 404: no such route, or no such section.
    NotFound,
    /// 404: the section has no song to play today. The client shows it as
    /// "No song today"; it is not a failure that a retry would mend.
    NoSong,
    /// 404, with the same code: random mode has nothing to draw. Its pool is
    /// empty, or everything in it plays in a section today or failed the
    /// preview check today.
    NothingToDraw,
    /// 404: the player has no random game: none was ever started under
    /// their ID, or it was cleared since.
    NoGame,
    /// 409: a move on a game that is already won or lost.
    Finished,
    /// 409, with the same code: a move on a random song that is already won
    /// or lost.
    SongOver,
    /// 409: the next random song was asked for while the current one is
    /// still being played.
    Unfinished,
    /// 409: the section's song was replaced (a re-roll, a reset) while the
    /// request was under way, so what the player was answering is gone. The
    /// move was not made; the client loads the game again.
    Changed,
    /// 409, with the same code: a move or a "next song" in random mode that
    /// names another song than the one the player is on, because another
    /// tab has moved on or started over. Nothing was changed; the client
    /// loads the game again.
    Moved,
    /// 502: Deezer failed, or this server is holding back to stay under
    /// Deezer's rate limit.
    Upstream(&'static str),
    /// 500: a bug or a broken environment on this side, such as a store that
    /// cannot be read or written.
    Internal,
}

impl ApiError {
    fn parts(self) -> (StatusCode, &'static str, &'static str) {
        match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message),
            Self::UnknownTrack => (
                StatusCode::NOT_FOUND,
                "unknown_track",
                "Deezer has no such track. Pick a song from the list.",
            ),
            Self::UnknownSong => (
                StatusCode::NOT_FOUND,
                "unknown_song",
                "That song is not in the pool.",
            ),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", "There is nothing here."),
            Self::NoSong => (
                StatusCode::NOT_FOUND,
                "no_song",
                "There is no song in this section today.",
            ),
            Self::NothingToDraw => (
                StatusCode::NOT_FOUND,
                "no_song",
                "There is no song to play in random mode right now.",
            ),
            Self::NoGame => (
                StatusCode::NOT_FOUND,
                "no_game",
                "No random game has been started.",
            ),
            Self::Finished => (
                StatusCode::CONFLICT,
                "finished",
                "Today's game is already over.",
            ),
            Self::SongOver => (
                StatusCode::CONFLICT,
                "finished",
                "This song is already over.",
            ),
            Self::Unfinished => (
                StatusCode::CONFLICT,
                "unfinished",
                "Finish this song first.",
            ),
            Self::Changed => (
                StatusCode::CONFLICT,
                "changed",
                "Today's song has just been changed. Load the game again.",
            ),
            Self::Moved => (
                StatusCode::CONFLICT,
                "changed",
                "That song is no longer the one being played. Load the game again.",
            ),
            Self::Upstream(message) => (StatusCode::BAD_GATEWAY, "upstream", message),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Something went wrong on the server.",
            ),
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    message: &'static str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error, message) = self.parts();
        (status, no_store(), Json(ErrorBody { error, message })).into_response()
    }
}

/// `Cache-Control: no-store`: every response here depends on the cookie or
/// changes with the next move, and the site sits behind a CDN.
pub(crate) fn no_store() -> [(header::HeaderName, HeaderValue); 1] {
    [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))]
}

/// A failed store operation as a response. What went wrong can name files,
/// tracks and players, so it goes to the log and the client gets the fixed
/// sentence.
pub(crate) fn store_failed(error: StoreError) -> ApiError {
    tracing::error!(%error, "the store failed");
    ApiError::Internal
}

/// Why there is no song, or no day, as a response. The reason names the
/// track, so it stays in the log ([`Daily::song`] writes it) and the client
/// gets a fixed sentence.
pub(crate) fn daily_failed(error: DailyError) -> ApiError {
    match error {
        DailyError::NoSong { .. } => ApiError::NoSong,
        DailyError::NoRandomSong { .. } => ApiError::NothingToDraw,
        DailyError::Store(error) => store_failed(error),
        DailyError::Clock(_) => {
            tracing::error!(%error, "the day cannot be worked out");
            ApiError::Internal
        }
        error => {
            tracing::debug!(%error, "no song to serve");
            ApiError::Upstream("The song could not be loaded. Try again in a moment.")
        }
    }
}

/// The section a route's address names. A slug that is not one of the four
/// is an address with nothing behind it.
fn named(section: Result<Path<Section>, PathRejection>) -> Result<Section, ApiError> {
    section
        .map(|Path(section)| section)
        .map_err(|_| ApiError::NotFound)
}

// --- GET /api/health ------------------------------------------------------------

#[derive(Serialize)]
struct Health {
    ok: bool,
}

async fn health() -> Json<Health> {
    Json(Health { ok: true })
}

async fn not_found() -> ApiError {
    ApiError::NotFound
}

// --- GET /api/today -------------------------------------------------------------

/// The body of `GET /api/today`: the day and where the player stands in each
/// of its four games. Nothing in it comes from a song.
#[derive(Debug, Serialize)]
struct TodayView {
    /// The server's day, `YYYY-MM-DD`.
    day: Date,
    /// 1 on the launch day.
    number: i64,
    /// The four sections, in the order the tabs are shown: [`Section::ALL`].
    sections: Vec<SectionToday>,
}

/// One section's line of [`TodayView`].
#[derive(Debug, Serialize)]
struct SectionToday {
    section: Section,
    /// The player's game there today. `playing` with no attempts is a game
    /// that has not been started.
    status: Status,
    /// How many tries have been used: skips and wrong guesses.
    attempts: usize,
    song: SongState,
}

/// Whether a section has a song today, as far as can be said without picking
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum SongState {
    /// The day's song is picked.
    Picked,
    /// Not picked yet, and there is something to pick from. The pick is made
    /// when the section's game is first asked for.
    Pending,
    /// Not picked, and nothing in the pool could be: no song today.
    None,
}

/// The overview behind the tabs. It reads and never picks: making the day's
/// picks can mean four downloads, and the page asks for this on every load.
/// The section's own route makes the pick and is the one that knows for sure;
/// `pending` here means "probably".
async fn today(
    State(app): State<AppState>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let day = app.today().await?;
    let known = player::known_player(&jar);
    let store = app.store();

    let picks = store.picks_on(day).await.map_err(store_failed)?;
    let picked = |section: Section| picks.iter().any(|pick| pick.section == section);
    // The pool is only needed to tell `pending` from `none`.
    let pool = if Section::ALL.into_iter().all(picked) {
        Vec::new()
    } else {
        store.songs().await.map_err(store_failed)?
    };
    let taken: BTreeSet<u64> = picks.iter().map(|pick| pick.track_id).collect();

    let mut sections = Vec::with_capacity(Section::ALL.len());
    for section in Section::ALL {
        let game = match &known {
            Some(player) => todays_game(&app, player, section, day).await?,
            None => GameState::new(day),
        };
        let song = if picked(section) {
            SongState::Picked
        } else if pick::candidates(section, day, &pool, &taken).is_empty() {
            SongState::None
        } else {
            SongState::Pending
        };
        sections.push(SectionToday {
            section,
            status: game.status(),
            attempts: game.attempts().len(),
            song,
        });
    }

    let view = TodayView {
        day,
        number: game::day_number(app.launch_date(), day),
        sections,
    };
    let player = known.unwrap_or_else(PlayerId::generate);
    let jar = player::remember(jar, &player, &headers);
    Ok((jar, no_store(), Json(view)).into_response())
}

// --- GET /api/daily/{section} ---------------------------------------------------

/// The body of `GET /api/daily/{section}` and of a successful
/// `POST /api/daily/{section}/guess`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DailyView<'a> {
    /// The server's day, the day of the game, `YYYY-MM-DD`.
    day: Date,
    /// 1 on the launch day.
    number: i64,
    /// The section this game belongs to, as its slug.
    section: Section,
    /// Clip length of each turn, in seconds.
    ladder: [f64; MAX_ATTEMPTS],
    attempts: &'a [Attempt],
    status: Status,
    /// The clip length unlocked right now, in seconds.
    clip_seconds: f64,
    /// `null` until the game is over.
    answer: Option<&'a Answer>,
    /// The player's record in this section, this game included once it is
    /// finished. It says nothing about the song, so it is there while
    /// playing too.
    stats: Stats,
}

impl<'a> DailyView<'a> {
    /// What the player may see of `game`. This is the only place the answer
    /// is handed out, and only for a finished game.
    fn new(
        game: &'a GameState,
        section: Section,
        stats: Stats,
        launch_date: Date,
        song: &'a Song,
    ) -> Self {
        Self {
            day: game.day(),
            number: game::day_number(launch_date, game.day()),
            section,
            ladder: game::ladder_seconds(),
            attempts: game.attempts(),
            status: game.status(),
            clip_seconds: f64::from(game.unlocked_ms()) / 1000.0,
            answer: game.is_finished().then_some(&song.answer),
            stats,
        }
    }
}

/// One player's games in one section, as the handlers need them: today's on
/// its own, the rest for the stats.
struct History {
    /// Today's game: the stored one, or a fresh one when the player has made
    /// no move today.
    today: GameState,
    /// The player's games on other days.
    other_days: Vec<GameState>,
}

impl History {
    /// The history of a player the server has not seen before: no games, and
    /// nothing to ask the store for.
    fn new(today: Date) -> Self {
        Self {
            today: GameState::new(today),
            other_days: Vec::new(),
        }
    }

    /// Reads the player's games in `section` from the store.
    async fn load(
        app: &AppState,
        player: &PlayerId,
        section: Section,
        today: Date,
    ) -> Result<Self, ApiError> {
        let mut games = app
            .store()
            .games(player, section)
            .await
            .map_err(store_failed)?;
        let todays = games.iter().position(|game| game.day() == today);
        Ok(Self {
            today: todays.map_or_else(|| GameState::new(today), |index| games.remove(index)),
            other_days: games,
        })
    }

    /// The record these games make, as of the day of today's game.
    fn stats(&self) -> Stats {
        let games = self.other_days.iter().chain([&self.today]);
        Stats::as_of(self.today.day(), games)
    }
}

/// The player's game in `section` for `today`, when that is all a handler
/// needs: the stored one, or a fresh one when they have made no move today.
async fn todays_game(
    app: &AppState,
    player: &PlayerId,
    section: Section,
    today: Date,
) -> Result<GameState, ApiError> {
    let stored = app
        .store()
        .game(player, section, today)
        .await
        .map_err(store_failed)?;
    // The store answers for the day it was asked about; `for_day` makes sure
    // that no backend's mistake turns another day's game into today's.
    Ok(stored
        .unwrap_or_else(|| GameState::new(today))
        .for_day(today))
}

/// The song `section` plays on `day`, with the guard that keeps it the
/// section's song. What a handler reads while it holds the guard (the
/// player's games) belongs to that song, and what it sends is about one song
/// and not two; see [`Daily::stands`].
///
/// For the requests that only look. If the song is replaced between fetching
/// it and taking the guard, the new one is fetched and the look is at the
/// new game, which is what the player would get a moment later anyway.
async fn standing_song(
    app: &AppState,
    section: Section,
    day: Date,
) -> Result<(Playing, Standing<'_>), ApiError> {
    for _ in 0..STANDING_TRIES {
        let playing = app.song(section, day).await?;
        let standing = app.daily().stands(&playing).await.map_err(store_failed)?;
        if let Some(standing) = standing {
            return Ok((playing, standing));
        }
    }
    Err(ApiError::Changed)
}

/// The state of today's game as a response, which also sets the player
/// cookie again so that it lasts from this visit.
fn game_response(
    app: &AppState,
    jar: PrivateCookieJar,
    headers: &HeaderMap,
    player: &PlayerId,
    section: Section,
    history: &History,
    song: &Song,
) -> Response {
    let view = DailyView::new(
        &history.today,
        section,
        history.stats(),
        app.launch_date(),
        song,
    );
    let jar = player::remember(jar, player, headers);
    (jar, no_store(), Json(view)).into_response()
}

async fn daily(
    State(app): State<AppState>,
    section: Result<Path<Section>, PathRejection>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    show_game(&app, named(section)?, jar, headers).await
}

async fn show_game(
    app: &AppState,
    section: Section,
    jar: PrivateCookieJar,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let today = app.today().await?;
    let known = player::known_player(&jar);
    let (playing, _standing) = standing_song(app, section, today).await?;
    let history = match &known {
        Some(player) => History::load(app, player, section, today).await?,
        // A new player has no games, so there is nothing to read; and the
        // visit stores nothing, so there is nothing to write.
        None => History::new(today),
    };
    let player = known.unwrap_or_else(PlayerId::generate);
    Ok(game_response(
        app,
        jar,
        &headers,
        &player,
        section,
        &history,
        &playing.song,
    ))
}

// --- GET /api/daily/{section}/audio ---------------------------------------------

/// The clip the player has unlocked: the leading frames of the preview, cut
/// by [`Mp3::prefix`](crate::mp3::Mp3::prefix), which also leaves out the tags.
///
/// The game is found through the player cookie. Without one it is a fresh
/// game, so the shortest clip, and no cookie is set: listening is not a
/// visit, and the page has asked for the daily state before it plays
/// anything.
///
/// There is no range support. A `Range` header gets the whole clip, which is
/// what a plain `fetch` wants and all the client does.
async fn audio(
    State(app): State<AppState>,
    section: Result<Path<Section>, PathRejection>,
    jar: PrivateCookieJar,
) -> Result<Response, ApiError> {
    play_clip(&app, named(section)?, jar).await
}

async fn play_clip(
    app: &AppState,
    section: Section,
    jar: PrivateCookieJar,
) -> Result<Response, ApiError> {
    let today = app.today().await?;
    // The song first and the game under its guard: how much is unlocked is
    // only true of the song the game was played against.
    let (playing, _standing) = standing_song(app, section, today).await?;
    let game = match player::known_player(&jar) {
        Some(player) => todays_game(app, &player, section, today).await?,
        None => GameState::new(today),
    };
    let clip = playing.song.mp3.prefix(game.unlocked_ms());
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static("audio/mpeg")),
        (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        (header::CONTENT_LENGTH, HeaderValue::from(clip.len())),
    ];
    Ok((headers, clip).into_response())
}

// --- POST /api/daily/{section}/guess --------------------------------------------

/// The body of a move: `{ "trackId": 123 }` or `{ "skip": true }`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuessRequest {
    track_id: Option<u64>,
    skip: Option<bool>,
}

/// What a valid [`GuessRequest`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    Skip,
    Guess(u64),
}

impl GuessRequest {
    /// The move, or `None` when the body names both or neither.
    fn into_move(self) -> Option<Move> {
        match (self.track_id, self.skip.unwrap_or(false)) {
            (Some(track_id), false) => Some(Move::Guess(track_id)),
            (None, true) => Some(Move::Skip),
            _ => None,
        }
    }
}

/// What a guessed track is, asked of Deezer by its ID: the title and artist
/// never come from the client, so a guess cannot be forged.
pub(crate) async fn guessed_track(app: &AppState, track_id: u64) -> Result<TrackMeta, ApiError> {
    match app.deezer().track_meta(track_id).await {
        Ok(guessed) if !guessed.title.trim().is_empty() => Ok(guessed),
        Ok(_) | Err(DeezerError::NotFound) => Err(ApiError::UnknownTrack),
        Err(error) => {
            tracing::warn!(%error, track_id, "could not look up a guessed track");
            Err(ApiError::Upstream(
                "Deezer did not answer, so the guess was not counted. Try again in a moment.",
            ))
        }
    }
}

async fn guess(
    State(app): State<AppState>,
    section: Result<Path<Section>, PathRejection>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
    body: Result<Json<GuessRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    make_move(&app, named(section)?, jar, headers, body).await
}

async fn make_move(
    app: &AppState,
    section: Section,
    jar: PrivateCookieJar,
    headers: HeaderMap,
    body: Result<Json<GuessRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let chosen = body
        .ok()
        .and_then(|Json(request)| request.into_move())
        .ok_or(ApiError::BadRequest(
            "Send either {\"trackId\": <number>} or {\"skip\": true} as JSON.",
        ))?;

    let today = app.today().await?;
    let known = player::known_player(&jar);
    // Checked before anything is looked up: a finished game costs Deezer
    // nothing. It is only a first look; the check that counts is the one
    // made under the lock below.
    if let Some(player) = &known
        && todays_game(app, player, section, today)
            .await?
            .is_finished()
    {
        return Err(ApiError::Finished);
    }
    let player = known.unwrap_or_else(PlayerId::generate);

    // Everything slow happens here, before the player's turn is taken: the
    // song (a download, the first time) and what the guessed track is.
    let playing = app.song(section, today).await?;
    let guessed = match chosen {
        Move::Skip => None,
        Move::Guess(track_id) => Some(guessed_track(app, track_id).await?),
    };

    // The move itself: read the game, apply the move, store the result, as
    // one critical section per player (see `MoveLocks` for why). Two moves
    // sent at once are made one after the other, each on the game the one
    // before it left, so each costs its own try and none is made on a game
    // that has ended in the meantime.
    let history = {
        let _turn = app.move_locks().lock(&player).await;
        // The player's own lock says nothing about the admin, who may have
        // re-rolled this section (or reset everything) since the song was
        // fetched a few lines up. That deleted the game this move was meant
        // for, and storing a game judged against the old song under the new
        // one would be worse than losing the move. So the move is made only
        // while the song it is judged against provably stands; otherwise the
        // player is told, and nothing is stored. The move lock is taken
        // first and the admin never takes it, so the two cannot deadlock.
        let _standing = app
            .daily()
            .stands(&playing)
            .await
            .map_err(store_failed)?
            .ok_or(ApiError::Changed)?;
        let mut history = History::load(app, &player, section, today).await?;
        let game = &mut history.today;
        let moved = match &guessed {
            None => game.skip(),
            Some(guessed) => game.guess(&playing.song.meta, guessed),
        };
        moved.map_err(|GameError::Finished| ApiError::Finished)?;
        app.store()
            .save_game(&player, section, game)
            .await
            .map_err(store_failed)?;
        history
    };

    Ok(game_response(
        app,
        jar,
        &headers,
        &player,
        section,
        &history,
        &playing.song,
    ))
}

// --- GET /api/search ------------------------------------------------------------

/// The query string of the two search routes: `?q=`.
#[derive(Debug, Deserialize)]
pub(crate) struct SearchParams {
    #[serde(default)]
    q: String,
}

/// What Deezer finds for a search request, best match first: up to
/// [`SEARCH_FETCH`] tracks, as they came. The player's search and the admin's
/// both start here, so they trim the query alike and share one cached answer.
///
/// `q` is trimmed and cut at [`MAX_QUERY_CHARS`]. Fewer than
/// [`MIN_QUERY_CHARS`] characters find nothing, and Deezer is not asked.
pub(crate) async fn find_tracks(
    app: &AppState,
    params: Result<Query<SearchParams>, QueryRejection>,
) -> Result<Arc<[Track]>, ApiError> {
    let Query(params) =
        params.map_err(|_| ApiError::BadRequest("The search needs one `q` parameter."))?;
    let query: String = params.q.trim().chars().take(MAX_QUERY_CHARS).collect();
    if query.chars().count() < MIN_QUERY_CHARS {
        return Ok(Arc::from(Vec::new()));
    }

    app.deezer()
        .search_tracks(&query, SEARCH_FETCH)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Deezer search failed");
            ApiError::Upstream("The song search is not answering. Try again in a moment.")
        })
}

/// One autocomplete row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SearchHit {
    id: u64,
    /// Deezer's full title, version included, so that two rows that differ
    /// only in the version can be told apart.
    title: String,
    artist: String,
    album: String,
    /// 56 px album cover URL; empty when the album has no artwork.
    cover: String,
}

/// Turns Deezer's result list into autocomplete rows: the first track of each
/// song (see [`TrackMeta::match_key`](crate::game::TrackMeta::match_key)), in
/// Deezer's order, at most `limit` of them.
///
/// A search for a well-known song returns the single, the album cut, two
/// remasters and a live version. They all count as the same guess, so showing
/// more than one would only push other songs off the list.
fn search_hits(tracks: &[Track], limit: usize) -> Vec<SearchHit> {
    let mut seen = HashSet::new();
    tracks
        .iter()
        .filter(|track| !track.title.trim().is_empty())
        .filter(|track| seen.insert(track.meta().match_key()))
        .take(limit)
        .map(|track| SearchHit {
            id: track.id,
            title: track.title.clone(),
            artist: track.artist.name.clone(),
            album: track.album.title.clone(),
            cover: track.album.cover_small.clone(),
        })
        .collect()
}

async fn search(
    State(app): State<AppState>,
    params: Result<Query<SearchParams>, QueryRejection>,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    let tracks = find_tracks(&app, params).await?;
    Ok(Json(search_hits(&tracks, SEARCH_RESULTS)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        game::{FULL_CLIP_MS, LADDER_MS},
        mp3::Mp3,
        random::RandomGame,
        store::{Genre, Genres, MemoryStore, NewSong, Pick, PoolSong, SqliteStore},
        testutil::{
            BrokenStore, Harness, ID3_MARKER, LAUNCH, MockTrack, PREVIEW_FRAMES, Preview, Reply,
            TODAY, Unhurried, lost_game, playing_game, synthetic_mp3, won_game,
        },
    };
    use async_trait::async_trait;
    use axum::{body::Body, http::Request};
    use jiff::civil::date;
    use serde_json::{Value, json};

    /// The song being played. The ID is long so that it cannot turn up by
    /// chance in a `Content-Length` or in the cookie's ciphertext.
    const ANSWER_ID: u64 = 987_654_321;
    /// The same song in another release: a different ID that must still win.
    const ANSWER_OTHER_RELEASE: u64 = 987_654_322;
    const QUEEN: u64 = 10;
    const QUEEN_REMASTER: u64 = 11;
    const UNKNOWN_ID: u64 = 555;
    /// Songs for the genre sections, with IDs as unlikely to turn up by
    /// chance as the answer's.
    const POP_SONG: u64 = 300_000_001;
    const ROCK_SONG: u64 = 300_000_002;
    const HIP_HOP_SONG: u64 = 300_000_003;
    /// A second pop song, for a section whose song can change.
    const POP_OTHER: u64 = 300_000_004;
    /// A track Deezer knows but will not play: `readable: false`, no preview.
    const WITHDRAWN: u64 = 300_000_009;

    const POP: Section = Section::Genre(Genre::Pop);
    const ROCK: Section = Section::Genre(Genre::Rock);
    const HIP_HOP: Section = Section::Genre(Genre::HipHop);

    /// Strings that identify the answer. None may appear while playing.
    const SECRETS: [&str; 5] = [
        "987654321",
        "Zanzibar",
        "The Answers",
        "Night Album",
        "cover987654321",
    ];

    fn tracks() -> Vec<MockTrack> {
        let mut tracks = vec![
            MockTrack::new(
                ANSWER_ID,
                "Zanzibar Nights (Remastered 2011)",
                "The Answers",
            )
            .title_short("Zanzibar Nights")
            .album("Night Album"),
            MockTrack::new(ANSWER_OTHER_RELEASE, "Zanzibar Nights", "The Answers")
                .album("Night Album (Deluxe)"),
            MockTrack::new(QUEEN, "Under Pressure", "Queen").album("Hot Space"),
            MockTrack::new(QUEEN_REMASTER, "Under Pressure (Remastered 2011)", "Queen")
                .title_short("Under Pressure")
                .album("Greatest Hits"),
            MockTrack::new(POP_SONG, "Bubblegum Avenue", "The Poppers").album("Sugar Album"),
            MockTrack::new(ROCK_SONG, "Granite Thunder", "The Rockers").album("Stone Album"),
            MockTrack::new(HIP_HOP_SONG, "Concrete Verses", "The Rhymers").album("Street Album"),
            MockTrack::new(POP_OTHER, "Lollipop Lane", "The Candies").album("Candy Album"),
            MockTrack::new(WITHDRAWN, "Vanished Hit", "The Ghosts").preview(Preview::None),
        ];
        // Two releases each of twelve songs, for the de-duplication and the cap.
        for n in 0..12 {
            let title = format!("Filler Song {n}");
            tracks.push(MockTrack::new(100 + n, &title, "Padding"));
            tracks.push(
                MockTrack::new(200 + n, &format!("{title} (Live)"), "Padding").title_short(&title),
            );
        }
        tracks
    }

    /// The router over a local Deezer stand-in and an in-memory store whose
    /// pool is the one song [`ANSWER_ID`]: General plays it, and the genre
    /// sections have no song.
    async fn start() -> Harness {
        Harness::start(tracks(), ANSWER_ID).await
    }

    /// The anti-leak rule, as a check on one response: headers, cookie and
    /// body together.
    trait NoSecrets {
        fn assert_no_secrets(&self);
    }

    impl NoSecrets for Reply {
        fn assert_no_secrets(&self) {
            self.assert_lacks(&SECRETS);
        }
    }

    /// The record of a player who has finished no game.
    fn no_stats() -> Value {
        json!({
            "played": 0,
            "won": 0,
            "winPercent": 0,
            "currentStreak": 0,
            "bestStreak": 0,
            "guessDistribution": [0, 0, 0, 0, 0, 0, 0],
        })
    }

    fn reference_mp3() -> Mp3 {
        Mp3::parse(synthetic_mp3(PREVIEW_FRAMES)).unwrap()
    }

    // --- GET /api/daily/{section} -------------------------------------------

    #[tokio::test]
    async fn health_still_answers() {
        let harness = start().await;
        let reply = harness.player().get("/api/health").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json(), json!({ "ok": true }));
    }

    #[tokio::test]
    async fn a_first_visit_gets_a_fresh_game_and_an_empty_record() {
        let harness = start().await;
        let mut player = harness.player();
        let reply = player.get("/api/daily/general").await;

        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(
            reply.json(),
            json!({
                "day": "2026-10-03",
                "number": 3,
                "section": "general",
                "ladder": [0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0],
                "attempts": [],
                "status": "playing",
                "clipSeconds": 0.1,
                "answer": null,
                "stats": no_stats(),
            })
        );
        assert_eq!(reply.header(header::CACHE_CONTROL), "no-store");
        // The cookie that comes with it is `player::tests`' subject; here it
        // only matters that it gives nothing away.
        assert!(reply.header(header::SET_COOKIE).starts_with("nd_player="));
        reply.assert_no_secrets();
    }

    #[tokio::test]
    async fn a_visit_stores_nothing_and_the_first_move_stores_the_game() {
        let harness = start().await;
        let today = TODAY;
        let mut player = harness.player();

        // Looking, reloading and listening are not moves.
        player.get("/api/daily/general").await;
        player.get("/api/daily/general").await;
        player.get("/api/daily/general/audio").await;
        let id = harness.player_id(&player).unwrap();
        let store = &harness.store;
        assert_eq!(
            store.game(&id, Section::General, today).await.unwrap(),
            None
        );
        assert_eq!(
            store.games(&id, Section::General).await.unwrap(),
            Vec::new()
        );

        // A request that is not a valid move stores nothing either.
        player.post(json!({})).await;
        player.guess(UNKNOWN_ID).await;
        assert_eq!(
            store.games(&id, Section::General).await.unwrap(),
            Vec::new()
        );

        // The first move does, under the General section and today's date.
        player.skip().await;
        assert_eq!(harness.player_id(&player), Some(id.clone()));
        assert_eq!(
            store.games(&id, Section::General).await.unwrap(),
            vec![playing_game(today, 1)]
        );
        for genre in Genre::ALL {
            assert_eq!(store.games(&id, genre.into()).await.unwrap(), Vec::new());
        }

        // Each later move replaces that one game; a visit changes nothing.
        player.skip().await;
        player.get("/api/daily/general").await;
        assert_eq!(
            store.games(&id, Section::General).await.unwrap(),
            vec![playing_game(today, 2)]
        );
    }

    #[tokio::test]
    async fn a_first_move_without_a_visit_starts_the_game_and_issues_the_cookie() {
        let harness = start().await;
        let mut player = harness.player();

        let reply = player.skip().await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["attempts"], json!([{ "kind": "skip" }]));
        reply.assert_no_secrets();

        // The cookie from that response finds the game again.
        let id = harness.player_id(&player).unwrap();
        let body = player.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
        assert_eq!(harness.player_id(&player), Some(id));
    }

    // --- the game is the player's, in the store ---------------------------------

    #[tokio::test]
    async fn moves_are_found_again_with_nothing_but_the_id_cookie() {
        let harness = start().await;
        let mut player = harness.player();
        player.skip().await;
        let wrong = player.guess(QUEEN).await.json();
        let id = harness.player_id(&player).unwrap();

        // Another browser object holding a cookie for the same ID, made from
        // the ID alone: all it carries is who the player is.
        let mut elsewhere = harness.player_with_id(&id);
        assert_eq!(elsewhere.get("/api/daily/general").await.json(), wrong);
        let clip = elsewhere.get("/api/daily/general/audio").await;
        assert_eq!(clip.body, reference_mp3().prefix(1_000));

        // And a move made there is seen here.
        elsewhere.skip().await;
        let body = player.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"].as_array().unwrap().len(), 3);
        assert_eq!(body["clipSeconds"], 3.0);
    }

    #[tokio::test]
    async fn replaying_an_old_cookie_does_not_take_moves_back() {
        let harness = start().await;
        let mut player = harness.player();
        player.get("/api/daily/general").await;
        // The cookie as it was before any move. When the game lived in the
        // cookie, sending this one again undid every attempt.
        let before_any_move = player.cookie.clone();

        player.skip().await;
        player.guess(QUEEN).await;
        let after_two = player.get("/api/daily/general").await.json();
        assert_eq!(after_two["attempts"].as_array().unwrap().len(), 2);

        player.cookie = before_any_move.clone();
        let replayed = player.get("/api/daily/general").await;
        assert_eq!(replayed.json(), after_two);
        replayed.assert_no_secrets();
        player.cookie = before_any_move.clone();
        let clip = player.get("/api/daily/general/audio").await;
        assert_eq!(clip.body, reference_mp3().prefix(1_000));

        // A move sent with the old cookie is the third, not the first again.
        player.cookie = before_any_move.clone();
        let body = player.skip().await.json();
        assert_eq!(body["attempts"].as_array().unwrap().len(), 3);
        assert_eq!(body["clipSeconds"], 3.0);

        // Nor does it reopen a game that is over.
        for _ in 0..MAX_ATTEMPTS - 3 {
            player.skip().await;
        }
        player.cookie = before_any_move;
        assert_eq!(
            player.get("/api/daily/general").await.json()["status"],
            "lost"
        );
        player
            .skip()
            .await
            .assert_error(StatusCode::CONFLICT, "finished");
    }

    #[tokio::test]
    async fn two_players_do_not_see_each_others_games() {
        let harness = start().await;
        let mut first = harness.player();
        let mut second = harness.player();

        first.skip().await;
        first.guess(QUEEN).await;
        let reply = second.get("/api/daily/general").await;
        assert_eq!(reply.json()["attempts"], json!([]));
        assert_eq!(reply.json()["clipSeconds"], 0.1);
        assert_eq!(
            second
                .get("/api/daily/general/audio")
                .await
                .content_length(),
            8 * 418
        );

        // One winning tells the other nothing.
        assert_eq!(first.guess(ANSWER_ID).await.json()["status"], "won");
        let reply = second.skip().await;
        let body = reply.json();
        assert_eq!(body["status"], "playing");
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
        assert_eq!(body["answer"], Value::Null);
        assert_eq!(body["stats"], no_stats());
        reply.assert_no_secrets();

        let body = first.get("/api/daily/general").await.json();
        assert_eq!(body["status"], "won");
        assert_eq!(body["attempts"].as_array().unwrap().len(), 2);
        assert_ne!(harness.player_id(&first), harness.player_id(&second));
    }

    #[tokio::test]
    async fn yesterdays_game_does_not_carry_over() {
        let harness = start().await;
        let today = TODAY;
        let yesterday = today.yesterday().unwrap();
        let mut player = harness.player();
        player.get("/api/daily/general").await;
        let id = harness.player_id(&player).unwrap();
        let store = &harness.store;

        // Lost yesterday, with every clip unlocked and the answer shown.
        store
            .save_game(&id, Section::General, &lost_game(yesterday))
            .await
            .unwrap();

        let reply = player.get("/api/daily/general").await;
        let body = reply.json();
        assert_eq!(body["day"], today.to_string());
        assert_eq!(body["status"], "playing");
        assert_eq!(body["attempts"], json!([]));
        assert_eq!(body["clipSeconds"], 0.1);
        assert_eq!(body["answer"], Value::Null);
        reply.assert_no_secrets();
        assert_eq!(
            player
                .get("/api/daily/general/audio")
                .await
                .content_length(),
            8 * 418
        );

        // It is in the record, though, and it stays as it was when today's
        // game is played.
        assert_eq!(body["stats"]["played"], 1);
        assert_eq!(body["stats"]["won"], 0);
        let body = player.skip().await.json();
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
        assert_eq!(
            store.games(&id, Section::General).await.unwrap(),
            vec![lost_game(yesterday), playing_game(today, 1)]
        );

        // The same for a game still open yesterday: its attempts are not
        // today's, and a game dated tomorrow is not today's either.
        let other = PlayerId::generate();
        for game in [
            playing_game(yesterday, 5),
            won_game(today.tomorrow().unwrap(), 0),
        ] {
            store
                .save_game(&other, Section::General, &game)
                .await
                .unwrap();
        }
        let body = harness
            .player_with_id(&other)
            .get("/api/daily/general")
            .await
            .json();
        assert_eq!(body["attempts"], json!([]));
        assert_eq!(body["status"], "playing");
        assert_eq!(body["stats"], no_stats());
    }

    #[tokio::test]
    async fn a_game_in_another_section_is_not_this_one() {
        let harness = start().await;
        let today = TODAY;
        let id = PlayerId::generate();
        // What is stored under a genre is that genre's game.
        for genre in Genre::ALL {
            harness
                .store
                .save_game(&id, genre.into(), &won_game(today, 0))
                .await
                .unwrap();
        }

        let mut player = harness.player_with_id(&id);
        let reply = player.get("/api/daily/general").await;
        let body = reply.json();
        assert_eq!(body["section"], "general");
        assert_eq!(body["status"], "playing");
        assert_eq!(body["stats"], no_stats());
        reply.assert_no_secrets();
    }

    // --- stats --------------------------------------------------------------

    #[tokio::test]
    async fn the_record_is_worked_out_from_the_stored_games() {
        let harness = start().await;
        let today = TODAY;
        let ago = |days: i64| today.checked_sub(jiff::Span::new().days(days)).unwrap();
        let id = PlayerId::generate();
        let store = &harness.store;
        for game in [
            won_game(ago(6), 0),
            lost_game(ago(5)),
            won_game(ago(3), 2),
            won_game(ago(2), 2),
            won_game(ago(1), 6),
        ] {
            store.save_game(&id, Section::General, &game).await.unwrap();
        }
        let mut player = harness.player_with_id(&id);

        // Before today's game: the streak ran through yesterday and stands.
        let reply = player.get("/api/daily/general").await;
        assert_eq!(
            reply.json()["stats"],
            json!({
                "played": 5,
                "won": 4,
                "winPercent": 80,
                "currentStreak": 3,
                "bestStreak": 3,
                "guessDistribution": [1, 0, 2, 0, 0, 0, 1],
            })
        );
        reply.assert_no_secrets();

        // While playing it does not move.
        let reply = player.skip().await;
        let body = reply.json();
        assert_eq!(body["status"], "playing");
        assert_eq!(body["stats"]["played"], 5);
        assert_eq!(body["stats"]["currentStreak"], 3);
        reply.assert_no_secrets();

        // A win on the second try is in the response to the winning guess.
        let body = player.guess(ANSWER_ID).await.json();
        assert_eq!(body["status"], "won");
        let won = json!({
            "played": 6,
            "won": 5,
            "winPercent": 83,
            "currentStreak": 4,
            "bestStreak": 4,
            "guessDistribution": [1, 1, 2, 0, 0, 0, 1],
        });
        assert_eq!(body["stats"], won);
        // And in every later look at the day.
        assert_eq!(player.get("/api/daily/general").await.json()["stats"], won);
    }

    #[tokio::test]
    async fn a_loss_ends_the_streak_and_counts_as_played() {
        let harness = start().await;
        let today = TODAY;
        let id = PlayerId::generate();
        for game in [
            won_game(today.yesterday().unwrap().yesterday().unwrap(), 1),
            won_game(today.yesterday().unwrap(), 1),
        ] {
            harness
                .store
                .save_game(&id, Section::General, &game)
                .await
                .unwrap();
        }
        let mut player = harness.player_with_id(&id);
        for _ in 0..MAX_ATTEMPTS - 1 {
            let body = player.skip().await.json();
            assert_eq!(body["stats"]["currentStreak"], 2);
        }

        let body = player.skip().await.json();
        assert_eq!(body["status"], "lost");
        assert_eq!(
            body["stats"],
            json!({
                "played": 3,
                "won": 2,
                "winPercent": 67,
                "currentStreak": 0,
                "bestStreak": 2,
                "guessDistribution": [0, 2, 0, 0, 0, 0, 0],
            })
        );
    }

    #[tokio::test]
    async fn a_new_players_first_win_is_a_streak_of_one() {
        let harness = start().await;
        let mut player = harness.player();
        let body = player.guess(ANSWER_ID).await.json();
        assert_eq!(body["status"], "won");
        assert_eq!(
            body["stats"],
            json!({
                "played": 1,
                "won": 1,
                "winPercent": 100,
                "currentStreak": 1,
                "bestStreak": 1,
                "guessDistribution": [1, 0, 0, 0, 0, 0, 0],
            })
        );
    }

    // --- two moves at once --------------------------------------------------

    /// Sends every request in `moves` at the same moment, each from its own
    /// task and all as the player `id`, and returns the replies in order.
    async fn all_at_once(harness: &Harness, id: &PlayerId, moves: Vec<Value>) -> Vec<Reply> {
        let requests: Vec<_> = moves
            .into_iter()
            .map(|body| {
                let mut player = harness.player_with_id(id);
                tokio::spawn(async move { player.post(body).await })
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
        let harness =
            Harness::with_store(tracks(), ANSWER_ID, Arc::new(Unhurried(MemoryStore::new()))).await;
        let today = TODAY;
        let id = PlayerId::generate();
        // Load the song first, so that the requests meet at the store.
        harness.player_with_id(&id).get("/api/daily/general").await;

        // Three wrong guesses and two skips, all read before any is written
        // if nothing keeps them apart. Then they would all store "one
        // attempt", and five tries would have cost one.
        let moves = vec![
            json!({ "trackId": QUEEN }),
            json!({ "skip": true }),
            json!({ "trackId": 100 }),
            json!({ "trackId": 101 }),
            json!({ "skip": true }),
        ];
        let replies = all_at_once(&harness, &id, moves).await;

        // Each was made on the game the one before it left: the replies show
        // one to five attempts, each exactly once.
        let mut counts: Vec<usize> = replies
            .iter()
            .map(|reply| {
                assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
                reply.assert_no_secrets();
                reply.json()["attempts"].as_array().unwrap().len()
            })
            .collect();
        counts.sort_unstable();
        assert_eq!(counts, vec![1, 2, 3, 4, 5]);

        let stored = harness
            .store
            .game(&id, Section::General, today)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.attempts().len(), 5);
        let body = harness
            .player_with_id(&id)
            .get("/api/daily/general")
            .await
            .json();
        assert_eq!(body["attempts"].as_array().unwrap().len(), 5);
        assert_eq!(body["clipSeconds"], 16.0);
    }

    #[tokio::test]
    async fn more_moves_at_once_than_tries_left_end_the_game_and_no_more() {
        let harness =
            Harness::with_store(tracks(), ANSWER_ID, Arc::new(Unhurried(MemoryStore::new()))).await;
        let id = PlayerId::generate();
        harness.player_with_id(&id).get("/api/daily/general").await;

        // Ten skips. All of them pass the first look ("not finished yet");
        // the look that counts is the one each takes on its turn.
        let replies = all_at_once(&harness, &id, vec![json!({ "skip": true }); 10]).await;

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
        let lost = replies
            .iter()
            .filter(|reply| reply.status == StatusCode::OK && reply.json()["status"] == "lost")
            .count();
        assert_eq!(lost, 1);

        let body = harness
            .player_with_id(&id)
            .get("/api/daily/general")
            .await
            .json();
        assert_eq!(body["status"], "lost");
        assert_eq!(body["attempts"].as_array().unwrap().len(), MAX_ATTEMPTS);
        assert_eq!(body["stats"]["played"], 1);
    }

    #[tokio::test]
    async fn a_win_and_a_miss_at_once_do_not_overwrite_each_other() {
        let harness =
            Harness::with_store(tracks(), ANSWER_ID, Arc::new(Unhurried(MemoryStore::new()))).await;
        let id = PlayerId::generate();
        harness.player_with_id(&id).get("/api/daily/general").await;

        let moves = vec![json!({ "skip": true }), json!({ "trackId": ANSWER_ID })];
        let replies = all_at_once(&harness, &id, moves).await;

        // Whichever came second was made on the result of the first: either
        // the skip and then the win, or the win and then a refused skip.
        // Never a won game turned back into one that is being played.
        let body = harness
            .player_with_id(&id)
            .get("/api/daily/general")
            .await
            .json();
        assert_eq!(body["status"], "won");
        assert_eq!(replies[1].json()["status"], "won");
        match replies[0].status {
            StatusCode::OK => assert_eq!(body["attempts"], json!([{ "kind": "skip" }])),
            _ => {
                replies[0].assert_error(StatusCode::CONFLICT, "finished");
                assert_eq!(body["attempts"], json!([]));
            }
        }
    }

    #[tokio::test]
    async fn different_players_moving_at_once_keep_their_own_games() {
        let harness =
            Harness::with_store(tracks(), ANSWER_ID, Arc::new(Unhurried(MemoryStore::new()))).await;
        harness.get("/api/daily/general").await;
        let ids: Vec<PlayerId> = (0..8).map(|_| PlayerId::generate()).collect();

        // Player n skips n times, everybody at the same moment.
        let mut requests = Vec::new();
        for (n, id) in ids.iter().enumerate() {
            for _ in 0..n {
                let mut player = harness.player_with_id(id);
                requests.push(tokio::spawn(async move { player.skip().await }));
            }
        }
        for request in requests {
            assert_eq!(request.await.unwrap().status, StatusCode::OK);
        }

        for (n, id) in ids.iter().enumerate() {
            let body = harness
                .player_with_id(id)
                .get("/api/daily/general")
                .await
                .json();
            assert_eq!(body["attempts"].as_array().unwrap().len(), n, "player {n}");
        }
    }

    // --- the store behind the game ------------------------------------------

    #[tokio::test]
    async fn a_store_failure_is_an_internal_error_with_a_fixed_message() {
        let harness = Harness::with_store(tracks(), ANSWER_ID, Arc::new(BrokenStore)).await;
        let mut player = harness.player_with_id(&PlayerId::generate());

        for reply in [
            player.get("/api/daily/general").await,
            player.get("/api/daily/general/audio").await,
            player.skip().await,
            player.guess(QUEEN).await,
        ] {
            reply.assert_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
            reply.assert_lacks(&["fire", "needledrop.db"]);
            reply.assert_no_secrets();
            // No new identity is handed out over a failure.
            assert!(reply.headers.get(header::SET_COOKIE).is_none());
        }
        // The day itself is in the store, so nothing was asked of Deezer.
        assert_eq!(harness.deezer.api_hits(), 0);

        // A browser without a cookie has no games to read, but the day and
        // its songs are in the store too: nothing works for it either, and
        // no identity is handed out over a failure.
        let mut stranger = harness.player();
        for reply in [
            stranger.get("/api/daily/general").await,
            stranger.get("/api/daily/pop/audio").await,
            stranger.get("/api/today").await,
            stranger.skip().await,
        ] {
            reply.assert_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
            reply.assert_lacks(&["fire", "needledrop.db"]);
            reply.assert_no_secrets();
            assert!(reply.headers.get(header::SET_COOKIE).is_none());
        }
        // What needs neither still answers.
        assert_eq!(harness.get("/api/health").await.status, StatusCode::OK);
        assert_eq!(
            harness.get("/api/search?q=under+pressure").await.status,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn the_game_is_the_same_over_the_sqlite_store_and_outlives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("needledrop.db");
        let today = TODAY;

        let store = Arc::new(SqliteStore::open(&path).unwrap());
        let harness = Harness::with_store(tracks(), ANSWER_ID, store).await;
        let mut player = harness.player();
        assert_eq!(
            player.get("/api/daily/general").await.json()["stats"],
            no_stats()
        );
        player.skip().await;
        let reply = player.guess(QUEEN).await;
        reply.assert_no_secrets();
        let before = reply.json();
        assert_eq!(
            before["attempts"],
            json!([
                { "kind": "skip" },
                { "kind": "wrong", "title": "Under Pressure", "artist": "Queen" },
            ])
        );
        let id = harness.player_id(&player).unwrap();
        // An earlier day, for the record.
        harness
            .store
            .save_game(
                &id,
                Section::General,
                &won_game(today.yesterday().unwrap(), 4),
            )
            .await
            .unwrap();
        drop(harness);

        // A new process over the same file. (Its cookie key is another one,
        // which a real restart keeps; the ID is what matters here.)
        let store = Arc::new(SqliteStore::open(&path).unwrap());
        let restarted = Harness::with_store(tracks(), ANSWER_ID, store).await;
        let mut player = restarted.player_with_id(&id);
        let body = player.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"], before["attempts"]);
        assert_eq!(body["clipSeconds"], 1.0);
        assert_eq!(body["stats"]["played"], 1);
        assert_eq!(body["stats"]["currentStreak"], 1);

        let body = player.guess(ANSWER_ID).await.json();
        assert_eq!(body["status"], "won");
        assert_eq!(body["stats"]["currentStreak"], 2);
        assert_eq!(
            body["stats"]["guessDistribution"],
            json!([0, 0, 1, 0, 1, 0, 0])
        );

        // Clear my data empties the file's games for that player.
        assert_eq!(player.clear().await.status, StatusCode::NO_CONTENT);
        assert_eq!(
            restarted.store.games(&id, Section::General).await.unwrap(),
            Vec::new()
        );
    }

    // --- moves --------------------------------------------------------------

    #[tokio::test]
    async fn each_skip_adds_an_attempt_and_unlocks_a_longer_clip() {
        let harness = start().await;
        let mut player = harness.player();
        let mp3 = reference_mp3();
        let mut lengths = Vec::new();

        for (turn, &clip_ms) in LADDER_MS.iter().enumerate() {
            let state = player.get("/api/daily/general").await.json();
            assert_eq!(state["attempts"].as_array().unwrap().len(), turn);
            assert_eq!(state["status"], "playing");
            assert_eq!(state["clipSeconds"], f64::from(clip_ms) / 1000.0);
            assert_eq!(state["answer"], Value::Null);

            let clip = player.get("/api/daily/general/audio").await;
            assert_eq!(clip.status, StatusCode::OK);
            assert_eq!(clip.content_length(), clip.body.len());
            assert_eq!(clip.body, mp3.prefix(clip_ms), "turn {}", turn + 1);
            clip.assert_no_secrets();
            lengths.push(clip.content_length());

            let moved = player.skip().await;
            assert_eq!(moved.status, StatusCode::OK);
            let after = moved.json();
            assert_eq!(after["attempts"].as_array().unwrap().len(), turn + 1);
            assert_eq!(after["attempts"][turn], json!({ "kind": "skip" }));
            if turn + 1 < MAX_ATTEMPTS {
                assert_eq!(after["status"], "playing");
                moved.assert_no_secrets();
            } else {
                assert_eq!(after["status"], "lost");
            }
        }

        assert!(
            lengths.windows(2).all(|pair| pair[0] < pair[1]),
            "{lengths:?}"
        );
        // 0.1 s is four frames of audio plus four of padding; the last step is everything.
        assert_eq!(lengths[0], 8 * 418);
        assert_eq!(lengths[6], PREVIEW_FRAMES * 418);
    }

    #[tokio::test]
    async fn seven_misses_lose_and_reveal_the_answer() {
        let harness = start().await;
        let mut player = harness.player();
        for _ in 0..MAX_ATTEMPTS - 1 {
            player.skip().await;
        }
        let last = player.guess(QUEEN).await;
        assert_eq!(last.status, StatusCode::OK);

        let expected_answer = json!({
            "title": "Zanzibar Nights (Remastered 2011)",
            "artist": "The Answers",
            "album": "Night Album",
            "cover": "https://cdn-images.dzcdn.net/images/cover/cover987654321/500x500-000000-80-0-0.jpg",
            "link": "https://www.deezer.com/track/987654321",
        });
        let body = last.json();
        assert_eq!(body["status"], "lost");
        assert_eq!(body["clipSeconds"], f64::from(FULL_CLIP_MS) / 1000.0);
        assert_eq!(body["attempts"].as_array().unwrap().len(), MAX_ATTEMPTS);
        assert_eq!(body["answer"], expected_answer);

        // The store remembers it: a reload shows the same finished game.
        assert_eq!(player.get("/api/daily/general").await.json(), body);

        let clip = player.get("/api/daily/general/audio").await;
        assert_eq!(clip.body, reference_mp3().prefix(FULL_CLIP_MS));
        assert_eq!(clip.content_length(), PREVIEW_FRAMES * 418);
    }

    #[tokio::test]
    async fn a_move_after_the_end_is_a_conflict() {
        let harness = start().await;
        let mut player = harness.player();
        for _ in 0..MAX_ATTEMPTS {
            player.skip().await;
        }
        let before = player.get("/api/daily/general").await.json();
        let lookups = harness.deezer.api_hits();

        player
            .skip()
            .await
            .assert_error(StatusCode::CONFLICT, "finished");
        player
            .guess(QUEEN)
            .await
            .assert_error(StatusCode::CONFLICT, "finished");
        // The right answer is too late as well, and nothing was looked up.
        player
            .guess(ANSWER_ID)
            .await
            .assert_error(StatusCode::CONFLICT, "finished");
        assert_eq!(harness.deezer.api_hits(), lookups);

        assert_eq!(player.get("/api/daily/general").await.json(), before);
    }

    #[tokio::test]
    async fn a_wrong_guess_is_recorded_with_the_servers_title_and_artist() {
        let harness = start().await;
        let mut player = harness.player();

        // Extra fields a client might add to forge the attempt are ignored.
        let reply = player
            .post(json!({ "trackId": QUEEN, "title": "Zanzibar Nights", "artist": "The Answers" }))
            .await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(body["status"], "playing");
        assert_eq!(
            body["attempts"],
            json!([{ "kind": "wrong", "title": "Under Pressure", "artist": "Queen" }])
        );
        assert_eq!(body["clipSeconds"], 0.3);
        assert_eq!(body["answer"], Value::Null);
        reply.assert_no_secrets();

        let clip = player.get("/api/daily/general/audio").await;
        assert_eq!(clip.body, reference_mp3().prefix(300));
    }

    #[tokio::test]
    async fn the_right_song_wins_whatever_release_was_picked() {
        let harness = start().await;

        for track_id in [ANSWER_ID, ANSWER_OTHER_RELEASE] {
            let mut player = harness.player();
            player.skip().await;
            let reply = player.guess(track_id).await;
            assert_eq!(reply.status, StatusCode::OK);
            let body = reply.json();
            assert_eq!(body["status"], "won", "track {track_id}");
            // The winning guess is not an attempt.
            assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
            assert_eq!(body["clipSeconds"], 30.0);
            assert_eq!(body["answer"]["title"], "Zanzibar Nights (Remastered 2011)");
            assert_eq!(body["answer"]["artist"], "The Answers");

            let clip = player.get("/api/daily/general/audio").await;
            assert_eq!(clip.content_length(), PREVIEW_FRAMES * 418);
            player
                .skip()
                .await
                .assert_error(StatusCode::CONFLICT, "finished");
        }
    }

    #[tokio::test]
    async fn a_guess_picked_from_search_needs_no_second_lookup() {
        let harness = start().await;
        let mut player = harness.player();
        player.get("/api/daily/general").await;
        player.get("/api/search?q=under+pressure").await;
        let after_search = harness.deezer.api_hits();

        let body = player.guess(QUEEN).await.json();
        assert_eq!(body["attempts"][0]["title"], "Under Pressure");
        assert_eq!(harness.deezer.api_hits(), after_search);
    }

    #[tokio::test]
    async fn an_unknown_track_is_not_found_and_costs_no_turn() {
        let harness = start().await;
        let mut player = harness.player();
        player.skip().await;

        let reply = player.guess(UNKNOWN_ID).await;
        reply.assert_error(StatusCode::NOT_FOUND, "unknown_track");
        reply.assert_no_secrets();

        let body = player.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
    }

    #[tokio::test]
    async fn malformed_moves_are_bad_requests() {
        let harness = start().await;
        let mut player = harness.player();

        for body in [
            json!({}),
            json!({ "trackId": QUEEN, "skip": true }),
            json!({ "skip": false }),
            json!({ "trackId": "10" }),
            json!({ "trackId": -1 }),
            json!({ "trackId": 1.5 }),
            json!({ "skip": "yes" }),
            json!([1, 2]),
            json!(null),
        ] {
            let reply = player.post(body.clone()).await;
            reply.assert_error(StatusCode::BAD_REQUEST, "bad_request");
            reply.assert_no_secrets();
            assert!(reply.headers.get(header::SET_COOKIE).is_none(), "{body}");
        }
        // Not even a player was made of it.
        assert_eq!(player.cookie, None);
        player
            .post_raw("application/json", "{not json")
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        player
            .post_raw("application/json", "")
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        player
            .post_raw("text/plain", r#"{"skip":true}"#)
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");

        // None of it counted as a turn.
        let body = player.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"], json!([]));
    }

    #[tokio::test]
    async fn a_deezer_failure_during_a_guess_is_an_upstream_error() {
        let harness = start().await;
        let mut player = harness.player();
        player.get("/api/daily/general").await;

        harness.deezer.set_failing(true);
        let reply = player.guess(QUEEN).await;
        reply.assert_error(StatusCode::BAD_GATEWAY, "upstream");
        reply.assert_no_secrets();
        // A skip needs nothing from Deezer.
        assert_eq!(player.skip().await.status, StatusCode::OK);

        harness.deezer.set_failing(false);
        let body = player.guess(QUEEN).await.json();
        assert_eq!(body["attempts"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_song_that_cannot_be_loaded_is_an_upstream_error_until_deezer_is_back() {
        let harness = start().await;
        let mut player = harness.player();
        harness.deezer.set_failing(true);

        for uri in ["/api/daily/general", "/api/daily/general/audio"] {
            let reply = player.get(uri).await;
            reply.assert_error(StatusCode::BAD_GATEWAY, "upstream");
            reply.assert_no_secrets();
        }
        let reply = player.skip().await;
        reply.assert_error(StatusCode::BAD_GATEWAY, "upstream");

        harness.deezer.set_failing(false);
        assert_eq!(
            player.get("/api/daily/general").await.status,
            StatusCode::OK
        );
        assert_eq!(
            player.get("/api/daily/general/audio").await.status,
            StatusCode::OK
        );
    }

    // --- GET /api/daily/{section}/audio -------------------------------------

    #[tokio::test]
    async fn audio_without_a_cookie_is_the_shortest_clip() {
        let harness = start().await;
        let mut player = harness.player();
        let clip = player.get("/api/daily/general/audio?t=1727740800000").await;

        assert_eq!(clip.status, StatusCode::OK);
        assert_eq!(clip.header(header::CONTENT_TYPE), "audio/mpeg");
        assert_eq!(clip.header(header::CACHE_CONTROL), "no-store");
        assert_eq!(clip.content_length(), 8 * 418);
        assert!(clip.headers.get(header::ACCEPT_RANGES).is_none());
        // Frames only: it starts at a frame header and carries no ID3 tag.
        assert_eq!(clip.body[..2], [0xFF, 0xFB]);
        assert!(
            !clip
                .body
                .windows(ID3_MARKER.len())
                .any(|window| window == ID3_MARKER)
        );
        // Listening is not a move: no cookie is set.
        assert!(clip.headers.get(header::SET_COOKIE).is_none());
    }

    #[tokio::test]
    async fn a_range_request_gets_the_whole_clip_and_no_more() {
        let harness = start().await;
        let mut player = harness.player();
        let request = Request::get("/api/daily/general/audio")
            .header(header::RANGE, "bytes=100000-")
            .body(Body::empty())
            .unwrap();
        let clip = player.send(request).await;
        assert_eq!(clip.status, StatusCode::OK);
        assert!(clip.headers.get(header::CONTENT_RANGE).is_none());
        assert_eq!(clip.body, reference_mp3().prefix(100));
    }

    // --- GET /api/search ----------------------------------------------------

    #[tokio::test]
    async fn short_queries_do_not_reach_deezer() {
        let harness = start().await;
        let mut player = harness.player();
        for uri in [
            "/api/search",
            "/api/search?q=",
            "/api/search?q=a",
            "/api/search?q=%20a%20%20",
            "/api/search?other=1",
        ] {
            let reply = player.get(uri).await;
            assert_eq!(reply.status, StatusCode::OK, "{uri}");
            assert_eq!(reply.json(), json!([]), "{uri}");
        }
        assert_eq!(harness.deezer.api_hits(), 0);
    }

    #[tokio::test]
    async fn search_maps_and_deduplicates_the_results() {
        let harness = start().await;
        let mut player = harness.player();

        let reply = player.get("/api/search?q=%20Under%20%20PRESSURE%20").await;
        assert_eq!(reply.status, StatusCode::OK);
        // The remaster is the same song, so only the first release is listed.
        assert_eq!(
            reply.json(),
            json!([{
                "id": QUEEN,
                "title": "Under Pressure",
                "artist": "Queen",
                "album": "Hot Space",
                "cover": "https://cdn-images.dzcdn.net/images/cover/cover10/56x56-000000-80-0-0.jpg",
            }])
        );

        // 24 tracks match, 12 distinct songs: eight rows, no song twice.
        let rows = player.get("/api/search?q=filler").await.json();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), SEARCH_RESULTS);
        let titles: HashSet<&str> = rows
            .iter()
            .map(|row| row["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles.len(), SEARCH_RESULTS);
        assert!(titles.iter().all(|title| !title.contains("(Live)")));

        // The same query again, differently spaced, is answered from the cache.
        let hits = harness.deezer.api_hits();
        player.get("/api/search?q=FILLER%20").await;
        assert_eq!(harness.deezer.api_hits(), hits);
    }

    #[tokio::test]
    async fn a_failed_search_is_an_upstream_error() {
        let harness = start().await;
        harness.deezer.set_failing(true);
        harness
            .player()
            .get("/api/search?q=queen")
            .await
            .assert_error(StatusCode::BAD_GATEWAY, "upstream");
    }

    #[tokio::test]
    async fn unknown_routes_are_json_errors() {
        let harness = start().await;
        harness
            .player()
            .get("/api/nope")
            .await
            .assert_error(StatusCode::NOT_FOUND, "not_found");
    }

    // --- the four sections ----------------------------------------------------

    /// A server whose every section has exactly one song it can play, so the
    /// day's picks are known: [`POP_SONG`], [`ROCK_SONG`], [`HIP_HOP_SONG`],
    /// and for General, which gets what the genres leave, [`ANSWER_ID`].
    async fn four_sections() -> Harness {
        Harness::with_pool(
            tracks(),
            &[
                (ANSWER_ID, &[]),
                (POP_SONG, &[Genre::Pop]),
                (ROCK_SONG, &[Genre::Rock]),
                (HIP_HOP_SONG, &[Genre::HipHop]),
            ],
        )
        .await
    }

    /// Strings that identify the song a section plays in [`four_sections`].
    fn secrets_of(section: Section) -> &'static [&'static str] {
        match section {
            Section::General => &SECRETS,
            Section::Genre(Genre::Pop) => &["300000001", "Bubblegum", "The Poppers", "Sugar Album"],
            Section::Genre(Genre::Rock) => &["300000002", "Granite", "The Rockers", "Stone Album"],
            Section::Genre(Genre::HipHop) => {
                &["300000003", "Concrete", "The Rhymers", "Street Album"]
            }
        }
    }

    /// The anti-leak rule for a response that may not name any of the day's
    /// four songs.
    fn assert_no_song(reply: &Reply) {
        for section in Section::ALL {
            reply.assert_lacks(secrets_of(section));
        }
    }

    #[tokio::test]
    async fn each_section_is_a_game_of_its_own_with_its_own_song_and_record() {
        let harness = four_sections().await;
        let mut player = harness.player();

        // Four fresh games, none of which says anything about any song.
        for section in Section::ALL {
            let reply = player.daily(section).await;
            assert_eq!(reply.status, StatusCode::OK, "{section}");
            assert_eq!(
                reply.json(),
                json!({
                    "day": "2026-10-03",
                    "number": 3,
                    "section": section.slug(),
                    "ladder": [0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0],
                    "attempts": [],
                    "status": "playing",
                    "clipSeconds": 0.1,
                    "answer": null,
                    "stats": no_stats(),
                })
            );
            assert_eq!(reply.header(header::CACHE_CONTROL), "no-store");
            assert_no_song(&reply);

            let clip = player.audio(section).await;
            assert_eq!(clip.status, StatusCode::OK, "{section}");
            assert_eq!(clip.content_length(), 8 * 418);
            assert_no_song(&clip);
        }
        // One browser, one player, whichever section it looked at first.
        let id = harness.player_id(&player).unwrap();
        // Each section plays its own song.
        assert_eq!(harness.pick(TODAY, POP).await, Some(POP_SONG));
        assert_eq!(harness.pick(TODAY, ROCK).await, Some(ROCK_SONG));
        assert_eq!(harness.pick(TODAY, HIP_HOP).await, Some(HIP_HOP_SONG));
        assert_eq!(harness.pick(TODAY, Section::General).await, Some(ANSWER_ID));

        // Pop is won at the first try.
        let body = player.guess_in(POP, POP_SONG).await.json();
        assert_eq!(body["section"], "pop");
        assert_eq!(body["status"], "won");
        assert_eq!(
            body["answer"],
            json!({
                "title": "Bubblegum Avenue",
                "artist": "The Poppers",
                "album": "Sugar Album",
                "cover": "https://cdn-images.dzcdn.net/images/cover/cover300000001/500x500-000000-80-0-0.jpg",
                "link": "https://www.deezer.com/track/300000001",
            })
        );

        // The same guess is a miss in Rock, and tells nothing about Rock's
        // song, or about the other two.
        let reply = player.guess_in(ROCK, POP_SONG).await;
        let body = reply.json();
        assert_eq!(body["section"], "rock");
        assert_eq!(body["status"], "playing");
        assert_eq!(
            body["attempts"],
            json!([{ "kind": "wrong", "title": "Bubblegum Avenue", "artist": "The Poppers" }])
        );
        assert_eq!(body["answer"], Value::Null);
        for section in [ROCK, HIP_HOP, Section::General] {
            reply.assert_lacks(secrets_of(section));
        }

        // General's song is a miss in Hip-hop.
        player.skip_in(HIP_HOP).await;
        let reply = player.guess_in(HIP_HOP, ANSWER_ID).await;
        assert_eq!(reply.json()["status"], "playing");
        assert_eq!(reply.json()["attempts"].as_array().unwrap().len(), 2);
        for section in [ROCK, HIP_HOP, POP] {
            reply.assert_lacks(secrets_of(section));
        }

        // Each game is where it was left, with a record of its own.
        let pop = player.daily(POP).await.json();
        assert_eq!(pop["status"], "won");
        assert_eq!(pop["attempts"], json!([]));
        assert_eq!(pop["clipSeconds"], 30.0);
        assert_eq!(
            pop["stats"],
            json!({
                "played": 1,
                "won": 1,
                "winPercent": 100,
                "currentStreak": 1,
                "bestStreak": 1,
                "guessDistribution": [1, 0, 0, 0, 0, 0, 0],
            })
        );
        assert_eq!(
            player.audio(POP).await.content_length(),
            PREVIEW_FRAMES * 418
        );

        let reply = player.daily(ROCK).await;
        let rock = reply.json();
        assert_eq!(rock["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(rock["clipSeconds"], 0.3);
        assert_eq!(rock["stats"], no_stats());
        reply.assert_lacks(secrets_of(ROCK));
        assert_eq!(player.audio(ROCK).await.body, reference_mp3().prefix(300));

        let hip_hop = player.daily(HIP_HOP).await.json();
        assert_eq!(hip_hop["clipSeconds"], 1.0);
        assert_eq!(hip_hop["stats"], no_stats());
        assert_eq!(
            player.audio(HIP_HOP).await.body,
            reference_mp3().prefix(1_000)
        );

        let reply = player.daily(Section::General).await;
        assert_eq!(reply.json()["attempts"], json!([]));
        assert_eq!(reply.json()["stats"], no_stats());
        reply.assert_no_secrets();

        // And each is stored under its own section.
        let store = &harness.store;
        assert_eq!(
            store.games(&id, POP).await.unwrap(),
            vec![won_game(TODAY, 0)]
        );
        assert_eq!(store.games(&id, ROCK).await.unwrap().len(), 1);
        assert_eq!(
            store
                .game(&id, HIP_HOP, TODAY)
                .await
                .unwrap()
                .unwrap()
                .attempts()
                .len(),
            2
        );
        assert_eq!(
            store.games(&id, Section::General).await.unwrap(),
            Vec::new()
        );
        assert_eq!(harness.player_id(&player), Some(id));
    }

    #[tokio::test]
    async fn the_overview_shows_the_day_and_where_the_player_stands_in_each_section() {
        let harness = four_sections().await;
        let mut player = harness.player();

        // A first visit: nothing played, and nothing picked, because the
        // overview does not pick. It still knows there is something to pick.
        let reply = player.get("/api/today").await;
        assert_eq!(reply.status, StatusCode::OK);
        let unplayed = |song: &str| {
            json!({
                "day": "2026-10-03",
                "number": 3,
                "sections": [
                    { "section": "general", "status": "playing", "attempts": 0, "song": song },
                    { "section": "pop", "status": "playing", "attempts": 0, "song": song },
                    { "section": "rock", "status": "playing", "attempts": 0, "song": song },
                    { "section": "hip-hop", "status": "playing", "attempts": 0, "song": song },
                ],
            })
        };
        assert_eq!(reply.json(), unplayed("pending"));
        reply.assert_no_store();
        assert_no_song(&reply);
        assert_eq!(harness.deezer.api_hits(), 0);
        assert_eq!(harness.store.picks_on(TODAY).await.unwrap(), Vec::new());
        // Like the daily state, it makes a browser a player.
        assert!(reply.header(header::SET_COOKIE).starts_with("nd_player="));
        let id = harness.player_id(&player).unwrap();

        // One section's game asked for: the sections up to it in the pick
        // order are picked, the ones after it are not.
        player.daily(ROCK).await;
        let sections = player.get("/api/today").await.json()["sections"].clone();
        let songs: Vec<&str> = sections
            .as_array()
            .unwrap()
            .iter()
            .map(|section| section["song"].as_str().unwrap())
            .collect();
        assert_eq!(songs, ["pending", "picked", "picked", "pending"]);

        // A day's play: Pop won on the second try, Rock lost, Hip-hop begun.
        player.skip_in(POP).await;
        player.guess_in(POP, POP_SONG).await;
        for _ in 0..MAX_ATTEMPTS {
            player.skip_in(ROCK).await;
        }
        player.guess_in(HIP_HOP, QUEEN).await;
        player.daily(Section::General).await;

        let reply = player.get("/api/today").await;
        assert_eq!(
            reply.json(),
            json!({
                "day": "2026-10-03",
                "number": 3,
                "sections": [
                    { "section": "general", "status": "playing", "attempts": 0, "song": "picked" },
                    { "section": "pop", "status": "won", "attempts": 1, "song": "picked" },
                    { "section": "rock", "status": "lost", "attempts": 7, "song": "picked" },
                    { "section": "hip-hop", "status": "playing", "attempts": 1, "song": "picked" },
                ],
            })
        );
        // Not even for the finished games does it name a song.
        assert_no_song(&reply);
        // The cookie is set again, for the same player.
        assert!(
            reply
                .header(header::SET_COOKIE)
                .contains("Max-Age=34560000")
        );
        assert_eq!(harness.player_id(&player), Some(id));

        // Somebody else sees the same day and their own, empty, games.
        assert_eq!(
            harness.player().get("/api/today").await.json(),
            unplayed("picked")
        );

        // Clear my data empties all four at once.
        assert_eq!(player.clear().await.status, StatusCode::NO_CONTENT);
        assert_eq!(player.get("/api/today").await.json(), unplayed("picked"));
        for section in Section::ALL {
            let reply = player.daily(section).await;
            assert_eq!(reply.json()["attempts"], json!([]), "{section}");
            assert_eq!(reply.json()["stats"], no_stats(), "{section}");
            assert_no_song(&reply);
        }
    }

    #[tokio::test]
    async fn a_section_without_a_song_says_so_and_takes_no_moves() {
        // The pool is one untagged song: General has it, the genres nothing.
        let harness = start().await;
        let mut player = harness.player();

        for genre in Genre::ALL {
            let section = Section::Genre(genre);
            for reply in [
                player.daily(section).await,
                player.audio(section).await,
                player.skip_in(section).await,
                player.guess_in(section, ANSWER_ID).await,
            ] {
                reply.assert_error(StatusCode::NOT_FOUND, "no_song");
                reply.assert_no_secrets();
                assert!(reply.headers.get(header::SET_COOKIE).is_none());
            }
        }
        // Nothing was asked of Deezer for them, not even about the guess.
        assert_eq!(harness.deezer.api_hits(), 0);
        assert_eq!(player.cookie, None);

        // The overview tells the page before it asks.
        let sections = player.get("/api/today").await.json()["sections"].clone();
        assert_eq!(
            sections,
            json!([
                { "section": "general", "status": "playing", "attempts": 0, "song": "pending" },
                { "section": "pop", "status": "playing", "attempts": 0, "song": "none" },
                { "section": "rock", "status": "playing", "attempts": 0, "song": "none" },
                { "section": "hip-hop", "status": "playing", "attempts": 0, "song": "none" },
            ])
        );
        // General plays as ever.
        assert_eq!(player.skip().await.status, StatusCode::OK);
        let id = harness.player_id(&player).unwrap();
        for genre in Genre::ALL {
            let games = harness.store.games(&id, genre.into()).await.unwrap();
            assert_eq!(games, Vec::new());
        }
    }

    #[tokio::test]
    async fn general_has_no_song_when_a_genre_took_the_only_one() {
        let harness = Harness::with_pool(tracks(), &[(POP_SONG, &[Genre::Pop])]).await;
        let mut player = harness.player();

        // Before anything is picked the overview can only say "probably":
        // there is a song General could play.
        let sections = player.get("/api/today").await.json()["sections"].clone();
        assert_eq!(sections[0]["song"], "pending");
        assert_eq!(sections[1]["song"], "pending");

        // Asked for first, General still picks last, and finds its one song
        // taken.
        player
            .daily(Section::General)
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_song");
        assert_eq!(harness.pick(TODAY, POP).await, Some(POP_SONG));
        assert_eq!(harness.pick(TODAY, Section::General).await, None);
        assert_eq!(player.daily(POP).await.status, StatusCode::OK);

        let sections = player.get("/api/today").await.json()["sections"].clone();
        assert_eq!(sections[0]["song"], "none");
        assert_eq!(sections[1]["song"], "picked");
    }

    #[tokio::test]
    async fn an_unknown_section_is_not_found() {
        let harness = four_sections().await;
        let mut player = harness.player();

        for uri in [
            "/api/daily/jazz",
            "/api/daily/General",
            "/api/daily/hiphop",
            "/api/daily/hip-hop%20",
            "/api/daily/1",
            "/api/daily/jazz/audio",
            "/api/daily/pop/video",
            "/api/daily/pop/audio/more",
        ] {
            let reply = player.get(uri).await;
            reply.assert_error(StatusCode::NOT_FOUND, "not_found");
            assert_no_song(&reply);
        }
        // The check on the section comes before the one on the body.
        for body in [json!({ "skip": true }), json!({})] {
            player
                .post_to(
                    "/api/daily/jazz/guess",
                    "application/json",
                    body.to_string(),
                )
                .await
                .assert_error(StatusCode::NOT_FOUND, "not_found");
        }
        // The fixed words of the old routes are not sections either.
        player
            .get("/api/daily/guess/audio")
            .await
            .assert_error(StatusCode::NOT_FOUND, "not_found");
        player
            .get("/api/daily/audio/audio")
            .await
            .assert_error(StatusCode::NOT_FOUND, "not_found");

        assert_eq!(player.cookie, None);
        assert_eq!(harness.deezer.api_hits(), 0);
        assert_eq!(harness.store.picks_on(TODAY).await.unwrap(), Vec::new());
    }

    #[tokio::test]
    async fn no_song_is_the_answer_in_two_sections_on_one_day() {
        // Songs with several tags, so that sections compete for them, and
        // fewer songs than a week has days, so that pools run out.
        let harness = Harness::with_pool(
            tracks(),
            &[
                (100, &[Genre::Pop, Genre::Rock]),
                (101, &[Genre::Rock, Genre::HipHop]),
                (102, &[Genre::Pop, Genre::HipHop]),
                (103, &[]),
                (104, &[Genre::Pop, Genre::Rock, Genre::HipHop]),
            ],
        )
        .await;
        let pool = harness.store.songs().await.unwrap();
        let mut player = harness.player();

        let mut day = TODAY;
        for _ in 0..8 {
            // Asked for in the order of the tabs, which is not the order of
            // the picks.
            for section in Section::ALL {
                let reply = player.daily(section).await;
                assert_eq!(reply.status, StatusCode::OK, "{day} {section}");
                assert_eq!(reply.json()["day"], day.to_string());
            }

            let picks = harness.store.picks_on(day).await.unwrap();
            assert_eq!(picks.len(), 4, "{day}");
            let distinct: HashSet<u64> = picks.iter().map(|pick| pick.track_id).collect();
            assert_eq!(distinct.len(), 4, "{day}: {picks:?}");
            // And each genre plays a song that carries its tag.
            for pick in &picks {
                let song = pool
                    .iter()
                    .find(|song| song.track_id == pick.track_id)
                    .unwrap();
                if let Some(genre) = pick.section.genre() {
                    assert!(song.genres.contains(&genre), "{day}: {pick:?}");
                }
            }

            let reply = harness.post("/api/admin/next-day", None).await;
            assert_eq!(reply.status, StatusCode::OK);
            day = day.tomorrow().unwrap();
        }
    }

    #[tokio::test]
    async fn a_song_without_a_preview_is_skipped_for_the_day_and_marked() {
        let harness = Harness::with_pool(
            tracks(),
            &[(WITHDRAWN, &[Genre::Rock]), (ANSWER_ID, &[]), (QUEEN, &[])],
        )
        .await;
        let mut player = harness.player();

        // Rock's only song cannot be played: no song there today.
        let reply = player.daily(ROCK).await;
        reply.assert_error(StatusCode::NOT_FOUND, "no_song");
        reply.assert_lacks(&["300000009", "Vanished", "The Ghosts"]);
        // General draws from all three and never ends up with it.
        assert_eq!(player.daily(Section::General).await.status, StatusCode::OK);
        let general = harness.pick(TODAY, Section::General).await.unwrap();
        assert!([ANSWER_ID, QUEEN].contains(&general));
        assert_eq!(harness.pick(TODAY, ROCK).await, None);

        // It stays in the pool, with the day it failed on for the owner.
        let song = harness.store.song(WITHDRAWN).await.unwrap().unwrap();
        assert_eq!(song.preview_failed_on, Some(TODAY));
        let failed = harness.get("/api/admin/songs?failed=true").await.json();
        assert_eq!(failed["total"], 1);
        assert_eq!(failed["counts"]["previewFailed"], 1);
        let row = &failed["songs"][0];
        assert_eq!(row["trackId"], WITHDRAWN);
        assert_eq!(row["previewFailedOn"], "2026-10-03");
        // It is not asked about again that day.
        let hits = harness.deezer.api_hits();
        player
            .daily(ROCK)
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_song");
        assert_eq!(harness.deezer.api_hits(), hits);

        // The next day it is tried again, and marked again.
        harness.clock.set(TODAY.tomorrow().unwrap());
        player
            .daily(ROCK)
            .await
            .assert_error(StatusCode::NOT_FOUND, "no_song");
        assert_eq!(harness.deezer.api_hits(), hits + 1);
        let song = harness.store.song(WITHDRAWN).await.unwrap().unwrap();
        assert_eq!(song.preview_failed_on, Some(TODAY.tomorrow().unwrap()));
    }

    #[tokio::test]
    async fn deezer_being_down_is_an_upstream_error_that_burns_no_song() {
        let harness = four_sections().await;
        let mut player = harness.player();
        harness.deezer.set_failing(true);

        for section in Section::ALL {
            for reply in [
                player.daily(section).await,
                player.audio(section).await,
                player.skip_in(section).await,
            ] {
                reply.assert_error(StatusCode::BAD_GATEWAY, "upstream");
                assert_no_song(&reply);
            }
        }
        // Nothing was picked, nothing was skipped, and no game was started.
        assert_eq!(harness.store.picks_on(TODAY).await.unwrap(), Vec::new());
        for song in harness.store.songs().await.unwrap() {
            assert_eq!(song.preview_failed_on, None, "{}", song.track_id);
        }
        assert_eq!(player.cookie, None);
        // The overview needs no song and says they are still to come.
        let sections = player.get("/api/today").await.json()["sections"].clone();
        for section in sections.as_array().unwrap() {
            assert_eq!(section["song"], "pending");
        }

        // Deezer is back: the same requests work, with the songs they would
        // have had.
        harness.deezer.set_failing(false);
        for section in Section::ALL {
            assert_eq!(player.daily(section).await.status, StatusCode::OK);
        }
        assert_eq!(harness.pick(TODAY, POP).await, Some(POP_SONG));
        assert_eq!(harness.pick(TODAY, Section::General).await, Some(ANSWER_ID));
    }

    #[tokio::test]
    async fn the_days_songs_and_games_survive_a_restart() {
        // Two songs per genre: a pick that was drawn again after the restart
        // would differ half the time.
        let mut harness = Harness::with_pool(
            tracks(),
            &[
                (100, &[Genre::Pop]),
                (101, &[Genre::Pop]),
                (102, &[Genre::Rock]),
                (103, &[Genre::Rock]),
                (104, &[Genre::HipHop]),
                (105, &[Genre::HipHop]),
            ],
        )
        .await;
        let mut player = harness.player();
        for section in Section::ALL {
            player.skip_in(section).await;
        }
        let id = harness.player_id(&player).unwrap();
        let picks = harness.store.picks_on(TODAY).await.unwrap();
        assert_eq!(picks.len(), 4);
        let general = harness.pick(TODAY, Section::General).await.unwrap();
        let won = player.guess(general).await.json();
        assert_eq!(won["status"], "won");
        let hits = harness.deezer.api_hits();

        for _ in 0..5 {
            // A new process, and no Deezer to draw or download from: the
            // picks are in the store and the previews on disk.
            harness.restart();
            harness.deezer.set_failing(true);
            let mut player = harness.player_with_id(&id);
            for section in Section::ALL {
                let body = player.daily(section).await.json();
                assert_eq!(body["section"], section.slug());
                assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
                assert_eq!(
                    player.audio(section).await.status,
                    StatusCode::OK,
                    "{section}"
                );
            }
            assert_eq!(player.daily(Section::General).await.json(), won);
            assert_eq!(harness.store.picks_on(TODAY).await.unwrap(), picks);
            harness.deezer.set_failing(false);
        }
        assert_eq!(harness.deezer.api_hits(), hits);
        // One pick per section is all the history there is.
        for section in Section::ALL {
            assert_eq!(harness.store.pick_history(section).await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn a_new_real_day_is_a_new_game_in_every_section() {
        let harness = four_sections().await;
        let mut player = harness.player();
        assert_eq!(player.guess_in(POP, POP_SONG).await.json()["status"], "won");
        player.skip_in(ROCK).await;

        // Midnight passes.
        harness.clock.set(date(2026, 10, 4));
        for section in Section::ALL {
            let reply = player.daily(section).await;
            let body = reply.json();
            assert_eq!(body["day"], "2026-10-04", "{section}");
            assert_eq!(body["number"], 4);
            assert_eq!(body["status"], "playing");
            assert_eq!(body["attempts"], json!([]));
            assert_eq!(body["answer"], Value::Null);
            // Each section has one song, so it is the same as yesterday's;
            // the new game hides it all the same.
            assert_no_song(&reply);
        }
        // Yesterday's win carries the streak into today.
        let pop = player.daily(POP).await.json();
        assert_eq!(pop["stats"]["played"], 1);
        assert_eq!(pop["stats"]["currentStreak"], 1);
        // Yesterday's unfinished game in Rock is a day not won.
        let rock = player.daily(ROCK).await.json();
        assert_eq!(rock["stats"], no_stats());
        let today = player.get("/api/today").await.json();
        assert_eq!(today["day"], "2026-10-04");
        assert_eq!(today["number"], 4);
        // Both days have their picks on record.
        assert_eq!(harness.store.pick_history(POP).await.unwrap().len(), 2);
    }

    // --- the routes from before the sections -----------------------------------

    #[tokio::test]
    async fn the_routes_without_a_section_are_gone() {
        let harness = four_sections().await;
        let mut player = harness.player();

        // The client from before the sections asked for these three. They
        // were aliases of General until the client moved to the per-section
        // routes; now they are addresses like any other the server lacks.
        player
            .get("/api/daily")
            .await
            .assert_error(StatusCode::NOT_FOUND, "not_found");
        player
            .get("/api/daily/")
            .await
            .assert_error(StatusCode::NOT_FOUND, "not_found");
        // "audio" and "guess" are read as section slugs, which they are not.
        player
            .get("/api/daily/audio?t=0-playing")
            .await
            .assert_error(StatusCode::NOT_FOUND, "not_found");
        player
            .get("/api/daily/guess")
            .await
            .assert_error(StatusCode::NOT_FOUND, "not_found");
        // A move sent to the old address finds no route, or one ("guess" as
        // a section's game) that has no POST: axum's 405 with an empty body,
        // as for any method a route does not have.
        let skip = json!({ "skip": true }).to_string();
        player
            .post_to("/api/daily", "application/json", skip.clone())
            .await
            .assert_error(StatusCode::NOT_FOUND, "not_found");
        let reply = player
            .post_to("/api/daily/guess", "application/json", skip)
            .await;
        assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
        assert!(reply.body.is_empty());

        // None of it was a visit or a move: no cookie, no pick, no game.
        assert_eq!(player.cookie, None);
        assert_eq!(harness.deezer.api_hits(), 0);
        assert_eq!(harness.store.picks_on(TODAY).await.unwrap(), Vec::new());
        // The section's own routes are where the game is.
        let reply = player.daily(Section::General).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["section"], "general");
    }

    // --- a song replaced under a request --------------------------------------

    /// Where [`Gate`] holds a request up.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Point {
        /// After the day's picks have been read.
        PicksRead,
        /// After a player's games have been read.
        GamesRead,
    }

    /// A store that stops the first operation to reach an armed point until
    /// the test lets it go on, so that a test can put a re-roll exactly
    /// between two steps of a request.
    struct Gate {
        store: MemoryStore,
        armed: std::sync::Mutex<Option<Point>>,
        reached: tokio::sync::Notify,
        resume: tokio::sync::Notify,
    }

    impl Gate {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                store: MemoryStore::new(),
                armed: std::sync::Mutex::new(None),
                reached: tokio::sync::Notify::new(),
                resume: tokio::sync::Notify::new(),
            })
        }

        /// The next operation to come to `point` waits there.
        fn arm(&self, point: Point) {
            *self.armed.lock().unwrap() = Some(point);
        }

        /// Waits until an operation is held at the armed point.
        async fn reached(&self) {
            self.reached.notified().await;
        }

        /// Lets the held operation go on.
        fn open(&self) {
            self.resume.notify_one();
        }

        async fn pass(&self, point: Point) {
            let held = {
                let mut armed = self.armed.lock().unwrap();
                let held = *armed == Some(point);
                if held {
                    *armed = None;
                }
                held
            };
            if held {
                self.reached.notify_one();
                self.resume.notified().await;
            }
        }
    }

    #[async_trait]
    impl Store for Gate {
        async fn songs(&self) -> Result<Vec<PoolSong>, StoreError> {
            self.store.songs().await
        }
        async fn song(&self, track_id: u64) -> Result<Option<PoolSong>, StoreError> {
            self.store.song(track_id).await
        }
        async fn add_song(&self, song: NewSong, genres: Genres) -> Result<PoolSong, StoreError> {
            self.store.add_song(song, genres).await
        }
        async fn set_song_genres(
            &self,
            track_id: u64,
            genres: Genres,
        ) -> Result<Option<PoolSong>, StoreError> {
            self.store.set_song_genres(track_id, genres).await
        }
        async fn remove_song(&self, track_id: u64) -> Result<bool, StoreError> {
            self.store.remove_song(track_id).await
        }
        async fn set_preview_failed_on(
            &self,
            track_id: u64,
            day: Option<Date>,
        ) -> Result<bool, StoreError> {
            self.store.set_preview_failed_on(track_id, day).await
        }
        async fn game(
            &self,
            player: &PlayerId,
            section: Section,
            day: Date,
        ) -> Result<Option<GameState>, StoreError> {
            self.store.game(player, section, day).await
        }
        async fn save_game(
            &self,
            player: &PlayerId,
            section: Section,
            game: &GameState,
        ) -> Result<(), StoreError> {
            self.store.save_game(player, section, game).await
        }
        async fn games(
            &self,
            player: &PlayerId,
            section: Section,
        ) -> Result<Vec<GameState>, StoreError> {
            let games = self.store.games(player, section).await;
            self.pass(Point::GamesRead).await;
            games
        }
        async fn random_game(&self, player: &PlayerId) -> Result<Option<RandomGame>, StoreError> {
            self.store.random_game(player).await
        }
        async fn save_random_game(
            &self,
            player: &PlayerId,
            game: &RandomGame,
        ) -> Result<(), StoreError> {
            self.store.save_random_game(player, game).await
        }
        async fn delete_player(&self, player: &PlayerId) -> Result<usize, StoreError> {
            self.store.delete_player(player).await
        }
        async fn delete_games(&self, section: Section, day: Date) -> Result<usize, StoreError> {
            self.store.delete_games(section, day).await
        }
        async fn picks_on(&self, day: Date) -> Result<Vec<Pick>, StoreError> {
            let picks = self.store.picks_on(day).await;
            self.pass(Point::PicksRead).await;
            picks
        }
        async fn pick_history(&self, section: Section) -> Result<Vec<Pick>, StoreError> {
            self.store.pick_history(section).await
        }
        async fn save_pick(&self, pick: Pick) -> Result<Pick, StoreError> {
            self.store.save_pick(pick).await
        }
        async fn remove_pick(&self, day: Date, section: Section) -> Result<bool, StoreError> {
            self.store.remove_pick(day, section).await
        }
        async fn day_offset(&self) -> Result<i64, StoreError> {
            self.store.day_offset().await
        }
        async fn set_day_offset(&self, days: i64) -> Result<(), StoreError> {
            self.store.set_day_offset(days).await
        }
        async fn wipe_games_and_picks(&self) -> Result<(), StoreError> {
            self.store.wipe_games_and_picks().await
        }
    }

    /// Strings that identify either of the two songs Pop can play in
    /// [`gated`].
    const POP_SECRETS: [&str; 8] = [
        "300000001",
        "300000004",
        "Bubblegum",
        "Lollipop",
        "The Poppers",
        "The Candies",
        "Sugar Album",
        "Candy Album",
    ];

    /// A server over a [`Gate`] whose Pop section has two songs, so that a
    /// re-roll always changes its song. One player has skipped once in Pop.
    /// Returns the song Pop plays, too.
    async fn gated() -> (Harness, Arc<Gate>, PlayerId, u64) {
        let gate = Gate::new();
        let pool: [(u64, &[Genre]); 2] = [(POP_SONG, &[Genre::Pop]), (POP_OTHER, &[Genre::Pop])];
        let harness = Harness::over(tracks(), &pool, gate.clone()).await;
        let mut player = harness.player();
        assert_eq!(player.skip_in(POP).await.status, StatusCode::OK);
        let id = harness.player_id(&player).unwrap();
        let song = harness.pick(TODAY, POP).await.unwrap();
        (harness, gate, id, song)
    }

    fn reroll_pop(harness: &Harness) -> tokio::task::JoinHandle<Reply> {
        let mut admin = harness.player();
        tokio::spawn(async move {
            let request = Request::post("/api/admin/reroll")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "section": "pop" }).to_string()))
                .unwrap();
            admin.send(request).await
        })
    }

    #[tokio::test]
    async fn a_move_whose_song_was_rerolled_away_is_refused_and_not_stored() {
        let (harness, gate, id, old) = gated().await;

        // The winning guess, held up right after it has fetched the song it
        // will be judged against...
        gate.arm(Point::PicksRead);
        let mut mover = harness.player_with_id(&id);
        let moving = tokio::spawn(async move { mover.guess_in(POP, old).await });
        gate.reached().await;

        // ...while the admin re-rolls the section: the game with the skip is
        // deleted and Pop plays its other song.
        let reply = reroll_pop(&harness).await.unwrap();
        assert_eq!(reply.status, StatusCode::OK);
        let new = harness.pick(TODAY, POP).await.unwrap();
        assert_ne!(new, old);
        assert_eq!(harness.store.games(&id, POP).await.unwrap(), Vec::new());

        // The move goes on. Had it been stored, the player would hold a won
        // game of the new song for a guess at the old one.
        gate.open();
        let reply = moving.await.unwrap();
        reply.assert_error(StatusCode::CONFLICT, "changed");
        reply.assert_lacks(&POP_SECRETS);
        assert!(reply.headers.get(header::SET_COOKIE).is_none());
        assert_eq!(harness.store.games(&id, POP).await.unwrap(), Vec::new());

        // What the player has now is a fresh game of the new song, in which
        // the old song is a wrong guess.
        let mut player = harness.player_with_id(&id);
        let reply = player.daily(POP).await;
        assert_eq!(reply.json()["attempts"], json!([]));
        assert_eq!(reply.json()["stats"], no_stats());
        reply.assert_lacks(&POP_SECRETS);
        let body = player.guess_in(POP, old).await.json();
        assert_eq!(body["status"], "playing");
        assert_eq!(body["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(player.guess_in(POP, new).await.json()["status"], "won");
    }

    #[tokio::test]
    async fn a_skip_whose_song_was_reset_away_is_refused_too() {
        let (harness, gate, id, _) = gated().await;

        gate.arm(Point::PicksRead);
        let mut mover = harness.player_with_id(&id);
        let moving = tokio::spawn(async move { mover.skip_in(POP).await });
        gate.reached().await;

        // Reset to day 1: every pick and every game is gone, and "today" is
        // another day.
        let reply = harness.post("/api/admin/reset", None).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["day"], "2026-10-01");

        gate.open();
        moving
            .await
            .unwrap()
            .assert_error(StatusCode::CONFLICT, "changed");
        // Nothing of the old day came back.
        assert_eq!(harness.store.games(&id, POP).await.unwrap(), Vec::new());
        assert_eq!(harness.store.picks_on(TODAY).await.unwrap(), Vec::new());
    }

    #[tokio::test]
    async fn a_reroll_waits_for_a_move_that_is_being_made_and_then_deletes_it() {
        let (harness, gate, id, old) = gated().await;

        // The winning guess, held up in the middle of its turn: the song is
        // confirmed, the game is read, nothing is saved yet.
        gate.arm(Point::GamesRead);
        let mut mover = harness.player_with_id(&id);
        let moving = tokio::spawn(async move { mover.guess_in(POP, old).await });
        gate.reached().await;

        // A re-roll that arrives now cannot get in between.
        let mut rerolling = reroll_pop(&harness);
        let early = tokio::time::timeout(std::time::Duration::from_millis(50), &mut rerolling);
        assert!(early.await.is_err());
        assert_eq!(harness.pick(TODAY, POP).await, Some(old));

        // The move finishes as a move on the old song: a win, stored.
        gate.open();
        let reply = moving.await.unwrap();
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["status"], "won");

        // Then the re-roll happens, and takes that game with it.
        assert_eq!(rerolling.await.unwrap().status, StatusCode::OK);
        assert_ne!(harness.pick(TODAY, POP).await, Some(old));
        assert_eq!(harness.store.games(&id, POP).await.unwrap(), Vec::new());
        let reply = harness.player_with_id(&id).daily(POP).await;
        assert_eq!(reply.json()["status"], "playing");
        assert_eq!(reply.json()["answer"], Value::Null);
        reply.assert_lacks(&POP_SECRETS);
    }

    #[tokio::test]
    async fn a_look_that_straddles_a_reroll_shows_a_game_with_the_song_it_was_played_on() {
        let (harness, gate, id, old) = gated().await;

        // The state is asked for and held up after fetching the old song...
        gate.arm(Point::PicksRead);
        let mut looker = harness.player_with_id(&id);
        let looking = tokio::spawn(async move { looker.daily(POP).await });
        gate.reached().await;

        // ...the section is re-rolled, and the player wins the new game.
        assert_eq!(reroll_pop(&harness).await.unwrap().status, StatusCode::OK);
        let new = harness.pick(TODAY, POP).await.unwrap();
        assert_ne!(new, old);
        let mut player = harness.player_with_id(&id);
        let won = player.guess_in(POP, new).await.json();
        assert_eq!(won["status"], "won");

        // The look goes on and finds a finished game. Put together with the
        // song it had fetched, it would name the old song as the answer; it
        // fetches the song again instead, and shows the game as it is.
        gate.open();
        let reply = looking.await.unwrap();
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json(), won);
        let old_title = if old == POP_SONG {
            "Bubblegum"
        } else {
            "Lollipop"
        };
        reply.assert_lacks(&[old_title]);
    }

    #[tokio::test]
    async fn a_look_at_a_finished_game_never_shows_the_song_that_replaced_its_own() {
        let (harness, gate, id, old) = gated().await;
        let mut player = harness.player_with_id(&id);
        let won = player.guess_in(POP, old).await.json();
        assert_eq!(won["status"], "won");

        // The state and the clip are asked for, and each is held up after
        // fetching the old song; the re-roll lands in between.
        for audio in [false, true] {
            gate.arm(Point::PicksRead);
            let mut looker = harness.player_with_id(&id);
            let looking = tokio::spawn(async move {
                if audio {
                    looker.audio(POP).await
                } else {
                    looker.daily(POP).await
                }
            });
            gate.reached().await;
            assert_eq!(reroll_pop(&harness).await.unwrap().status, StatusCode::OK);
            gate.open();

            // What comes back is the new game, whole: nothing unlocked and
            // nothing revealed. Not the old game's "won" over the new song.
            let reply = looking.await.unwrap();
            assert_eq!(reply.status, StatusCode::OK);
            reply.assert_lacks(&POP_SECRETS);
            if audio {
                assert_eq!(reply.content_length(), 8 * 418);
            } else {
                let body = reply.json();
                assert_eq!(body["status"], "playing");
                assert_eq!(body["attempts"], json!([]));
                assert_eq!(body["answer"], Value::Null);
            }

            // Win again for the next round, on whatever Pop plays now.
            let now = harness.pick(TODAY, POP).await.unwrap();
            let mut player = harness.player_with_id(&id);
            assert_eq!(player.guess_in(POP, now).await.json()["status"], "won");
        }
    }

    // --- pure pieces --------------------------------------------------------

    fn track(id: u64, title: &str, title_short: &str, artist: &str) -> Track {
        serde_json::from_value(json!({
            "id": id,
            "title": title,
            "title_short": title_short,
            "artist": { "name": artist },
            "album": { "title": "LP", "cover_small": format!("small-{id}"), "cover_big": "big" },
        }))
        .unwrap()
    }

    #[test]
    fn search_hits_keep_the_first_of_each_song_in_order() {
        let tracks = [
            track(1, "Halo", "Halo", "Beyoncé"),
            track(2, "Halo (Live)", "Halo", "Beyonce"),
            track(3, "Halo", "Halo", "Depeche Mode"),
            track(4, "", "", "Nobody"),
            track(5, "HALO - Remastered 2011", "", "beyoncé"),
            track(6, "Hello", "Hello", "Adele"),
        ];
        let hits = search_hits(&tracks, 8);
        assert_eq!(
            hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            vec![1, 3, 6]
        );
        assert_eq!(
            hits[0],
            SearchHit {
                id: 1,
                title: "Halo".to_owned(),
                artist: "Beyoncé".to_owned(),
                album: "LP".to_owned(),
                cover: "small-1".to_owned(),
            }
        );
        // The cap counts songs, not tracks looked at.
        assert_eq!(
            search_hits(&tracks, 2)
                .iter()
                .map(|hit| hit.id)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert!(search_hits(&[], 8).is_empty());
    }

    fn song() -> Song {
        let track: Track = serde_json::from_value(
            MockTrack::new(ANSWER_ID, "Zanzibar Nights", "The Answers")
                .album("Night Album")
                .json("http://mock"),
        )
        .unwrap();
        Song::new(&track, Mp3::parse(synthetic_mp3(20)).unwrap())
    }

    /// The view of a player's only game, in the General section.
    fn view_of<'a>(game: &'a GameState, song: &'a Song) -> DailyView<'a> {
        let stats = Stats::as_of(game.day(), [game]);
        DailyView::new(game, Section::General, stats, LAUNCH, song)
    }

    #[test]
    fn the_view_has_no_answer_while_playing() {
        let song = song();
        let today = date(2026, 10, 3);
        let mut game = GameState::new(today);
        game.skip().unwrap();
        game.guess(&song.meta, &TrackMeta::new("Under Pressure", "", "Queen"))
            .unwrap();

        let stats = Stats::as_of(today, [&won_game(date(2026, 10, 2), 1), &game]);
        let view = DailyView::new(&game, Section::Genre(Genre::HipHop), stats, LAUNCH, &song);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(
            json,
            json!({
                "day": "2026-10-03",
                "number": 3,
                "section": "hip-hop",
                "ladder": [0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0],
                "attempts": [
                    { "kind": "skip" },
                    { "kind": "wrong", "title": "Under Pressure", "artist": "Queen" },
                ],
                "status": "playing",
                "clipSeconds": 1.0,
                "answer": null,
                "stats": {
                    "played": 1,
                    "won": 1,
                    "winPercent": 100,
                    "currentStreak": 1,
                    "bestStreak": 1,
                    "guessDistribution": [0, 1, 0, 0, 0, 0, 0],
                },
            })
        );
        let text = json.to_string();
        for secret in SECRETS {
            assert!(!text.contains(secret), "{secret:?} in {text}");
        }
    }

    #[test]
    fn the_view_has_the_answer_once_won_or_lost() {
        let song = song();
        let today = date(2026, 10, 1);

        let mut won = GameState::new(today);
        won.guess(&song.meta, &song.meta).unwrap();
        let view = serde_json::to_value(view_of(&won, &song)).unwrap();
        assert_eq!(view["status"], "won");
        assert_eq!(view["section"], "general");
        assert_eq!(view["stats"]["currentStreak"], 1);
        assert_eq!(view["number"], 1);
        assert_eq!(view["clipSeconds"], 30.0);
        assert_eq!(
            view["answer"],
            json!({
                "title": "Zanzibar Nights",
                "artist": "The Answers",
                "album": "Night Album",
                "cover": "https://cdn-images.dzcdn.net/images/cover/cover987654321/500x500-000000-80-0-0.jpg",
                "link": "https://www.deezer.com/track/987654321",
            })
        );

        let mut lost = GameState::new(today);
        for _ in 0..MAX_ATTEMPTS {
            lost.skip().unwrap();
        }
        let view = serde_json::to_value(view_of(&lost, &song)).unwrap();
        assert_eq!(view["status"], "lost");
        assert_eq!(view["stats"]["played"], 1);
        assert_eq!(view["answer"]["title"], "Zanzibar Nights");
    }

    #[test]
    fn guess_bodies_name_exactly_one_move() {
        let parse = |body: Value| {
            serde_json::from_value::<GuessRequest>(body)
                .ok()
                .and_then(GuessRequest::into_move)
        };
        assert_eq!(parse(json!({ "trackId": 7 })), Some(Move::Guess(7)));
        assert_eq!(
            parse(json!({ "trackId": 7, "skip": false })),
            Some(Move::Guess(7))
        );
        assert_eq!(parse(json!({ "skip": true })), Some(Move::Skip));
        assert_eq!(
            parse(json!({ "skip": true, "trackId": null })),
            Some(Move::Skip)
        );
        assert_eq!(parse(json!({})), None);
        assert_eq!(parse(json!({ "skip": false })), None);
        assert_eq!(parse(json!({ "trackId": 7, "skip": true })), None);
        assert_eq!(parse(json!({ "track_id": 7 })), None);
    }
}
