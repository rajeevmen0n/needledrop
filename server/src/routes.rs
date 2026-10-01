//! HTTP handlers, the shared state and the encrypted-cookie game session.
//!
//! The rule every handler here keeps: while a game is `playing`, nothing that
//! identifies the song leaves the server. The answer goes out through one
//! place only ([`DailyView::new`]), error messages are fixed sentences, and
//! the audio is the clip the session has unlocked and no more.

use std::{
    collections::HashSet,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::Arc,
};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{
        FromRef, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, Key, PrivateCookieJar, SameSite};
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::{
    daily::{Answer, Daily, Song, today_utc},
    deezer::{Deezer, DeezerError, Track},
    game::{self, Attempt, GameError, GameState, MAX_ATTEMPTS, Status},
};

/// The session cookie: the player's [`GameState`] as JSON, encrypted.
const COOKIE_NAME: &str = "gts_daily";

/// A game lasts one UTC day; two days cover every time zone's "today" with
/// room to spare, and the state inside is discarded once its day is over.
const COOKIE_MAX_AGE: time::Duration = time::Duration::days(2);

/// The cookie key on disk, under the data directory.
const KEY_FILE: &str = "secret.key";

/// Length of the cookie key: 32 bytes to sign with and 32 to encrypt with.
const KEY_BYTES: usize = 64;

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

/// What every handler shares. Cheap to clone.
#[derive(Clone)]
pub struct AppState(Arc<Shared>);

struct Shared {
    deezer: Deezer,
    daily: Daily,
    launch_date: Date,
    key: Key,
}

impl AppState {
    pub fn new(deezer: Deezer, daily: Daily, launch_date: Date, key: Key) -> Self {
        Self(Arc::new(Shared {
            deezer,
            daily,
            launch_date,
            key,
        }))
    }

    /// The song played on `day`, loading it if needed.
    ///
    /// The reason for a failure names the track, so it stays in the log
    /// ([`Daily::song_for`] writes it); the client gets a fixed sentence.
    pub async fn song(&self, day: Date) -> Result<Arc<Song>, ApiError> {
        self.0.daily.song_for(day).await.map_err(|error| {
            tracing::debug!(%error, "no song to serve");
            ApiError::Upstream("The song could not be loaded. Try again in a moment.")
        })
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
        .route("/api/daily", get(daily))
        .route("/api/daily/audio", get(audio))
        .route("/api/daily/guess", post(guess))
        .route("/api/search", get(search))
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
    /// 404: Deezer has no track with the guessed ID.
    UnknownTrack,
    /// 404: no such route.
    NotFound,
    /// 409: a move on a game that is already won or lost.
    Finished,
    /// 502: Deezer failed, or this server is holding back to stay under
    /// Deezer's rate limit.
    Upstream(&'static str),
    /// 500: a bug or a broken environment on this side.
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
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", "There is nothing here."),
            Self::Finished => (
                StatusCode::CONFLICT,
                "finished",
                "Today's game is already over.",
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
fn no_store() -> [(header::HeaderName, HeaderValue); 1] {
    [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))]
}

// --- the session cookie ---------------------------------------------------------

/// The player's game for `today`, read from the cookie.
///
/// A cookie that is missing, does not decrypt (the jar then reports it as
/// absent) or does not hold a possible game is a fresh game, and so is one
/// left over from another day.
fn load_game(jar: &PrivateCookieJar, today: Date) -> GameState {
    jar.get(COOKIE_NAME)
        .and_then(|cookie| serde_json::from_str::<GameState>(cookie.value()).ok())
        .unwrap_or_else(|| GameState::new(today))
        .for_day(today)
}

/// Puts `game` into the cookie. `secure` marks it HTTPS-only.
fn store_game(
    jar: PrivateCookieJar,
    game: &GameState,
    secure: bool,
) -> Result<PrivateCookieJar, ApiError> {
    let json = serde_json::to_string(game).map_err(|error| {
        tracing::error!(%error, "could not serialize the game state");
        ApiError::Internal
    })?;
    let cookie = Cookie::build((COOKIE_NAME, json))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(COOKIE_MAX_AGE)
        .secure(secure)
        .build();
    Ok(jar.add(cookie))
}

/// Whether the player's browser reached the site over HTTPS.
///
/// The server itself only ever sees plain HTTP from nginx, which reports the
/// original scheme in `X-Forwarded-Proto`. A `Secure` cookie on plain HTTP
/// (the Vite dev server on localhost) would be dropped by the browser, so the
/// flag follows the header instead of being always on.
fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .is_some_and(|scheme| scheme.trim().eq_ignore_ascii_case("https"))
}

/// The cookie key could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "expected 128 hexadecimal characters (64 random bytes, for example from `openssl rand -hex 64`)"
)]
pub struct KeyFormatError;

/// The key the session cookie is encrypted with.
///
/// `secret` is `GTS_SECRET`: 128 hexadecimal characters, that is 64 random
/// bytes (`openssl rand -hex 64`). Without it the key lives in
/// `<data_dir>/secret.key`, in the same format, and is generated on the first
/// start. The file is created readable by its owner only.
///
/// Changing the key makes every existing cookie undecryptable, which the game
/// treats as "no cookie": each player starts the day again.
pub fn session_key(secret: Option<&str>, data_dir: &Path) -> anyhow::Result<Key> {
    if let Some(secret) = secret {
        // The error does not repeat the value: it would end up in the log.
        return parse_key(secret).context("GTS_SECRET is not a usable cookie key");
    }

    let path = data_dir.join(KEY_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_key(&text).with_context(|| {
            format!(
                "the cookie key in {} is unusable; delete the file to have a new one generated",
                path.display()
            )
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let key = Key::try_generate().context("no secure random source for the cookie key")?;
            std::fs::create_dir_all(data_dir)
                .with_context(|| format!("creating the data directory {}", data_dir.display()))?;
            write_key_file(&path, &key)
                .with_context(|| format!("writing the cookie key to {}", path.display()))?;
            tracing::info!(path = %path.display(), "generated a new cookie key");
            Ok(key)
        }
        Err(error) => {
            Err(error).with_context(|| format!("reading the cookie key from {}", path.display()))
        }
    }
}

/// Writes the key as hex, to a file only its owner can read. Fails if the
/// file already exists, so a key is never silently replaced.
fn write_key_file(path: &Path, key: &Key) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    let mut text = encode_hex(key.master());
    text.push('\n');
    file.write_all(text.as_bytes())
}

fn parse_key(text: &str) -> Result<Key, KeyFormatError> {
    let bytes = decode_hex(text.trim()).ok_or(KeyFormatError)?;
    if bytes.len() != KEY_BYTES {
        return Err(KeyFormatError);
    }
    Key::try_from(bytes.as_slice()).map_err(|_| KeyFormatError)
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        // Writing to a `String` cannot fail.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let nibble = |byte: u8| char::from(byte).to_digit(16);
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|&[high, low]| u8::try_from(nibble(high)? * 16 + nibble(low)?).ok())
        .collect()
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

// --- GET /api/daily -------------------------------------------------------------

/// The body of `GET /api/daily` and of a successful `POST /api/daily/guess`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DailyView<'a> {
    /// The UTC date of the game, `YYYY-MM-DD`.
    day: Date,
    /// 1 on the launch day.
    number: i64,
    /// Clip length of each turn, in seconds.
    ladder: [f64; MAX_ATTEMPTS],
    attempts: &'a [Attempt],
    status: Status,
    /// The clip length unlocked right now, in seconds.
    clip_seconds: f64,
    /// `null` until the game is over.
    answer: Option<&'a Answer>,
}

impl<'a> DailyView<'a> {
    /// What the player may see of `game`. This is the only place the answer
    /// is handed out, and only for a finished game.
    fn new(game: &'a GameState, launch_date: Date, song: &'a Song) -> Self {
        Self {
            day: game.day(),
            number: game::day_number(launch_date, game.day()),
            ladder: game::ladder_seconds(),
            attempts: game.attempts(),
            status: game.status(),
            clip_seconds: f64::from(game.unlocked_ms()) / 1000.0,
            answer: game.is_finished().then_some(&song.answer),
        }
    }
}

/// The state of `game` as a response that also stores it in the cookie.
fn game_response(
    app: &AppState,
    jar: PrivateCookieJar,
    headers: &HeaderMap,
    game: &GameState,
    song: &Song,
) -> Result<Response, ApiError> {
    let view = DailyView::new(game, app.0.launch_date, song);
    let jar = store_game(jar, game, is_https(headers))?;
    Ok((jar, no_store(), Json(view)).into_response())
}

async fn daily(
    State(app): State<AppState>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let today = today_utc();
    let game = load_game(&jar, today);
    let song = app.song(today).await?;
    game_response(&app, jar, &headers, &game, &song)
}

// --- GET /api/daily/audio -------------------------------------------------------

/// The clip the session has unlocked: the leading frames of the preview, cut
/// by [`Mp3::prefix`](crate::mp3::Mp3::prefix), which also leaves out the tags.
///
/// There is no range support. A `Range` header gets the whole clip, which is
/// what a plain `fetch` wants and all the client does.
async fn audio(State(app): State<AppState>, jar: PrivateCookieJar) -> Result<Response, ApiError> {
    let today = today_utc();
    let game = load_game(&jar, today);
    let song = app.song(today).await?;
    let clip = song.mp3.prefix(game.unlocked_ms());
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static("audio/mpeg")),
        (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        (header::CONTENT_LENGTH, HeaderValue::from(clip.len())),
    ];
    Ok((headers, clip).into_response())
}

// --- POST /api/daily/guess ------------------------------------------------------

/// The body of `POST /api/daily/guess`: `{ "trackId": 123 }` or `{ "skip": true }`.
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

async fn guess(
    State(app): State<AppState>,
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

    let today = today_utc();
    let mut game = load_game(&jar, today);
    // Checked before anything is looked up: a finished game costs Deezer nothing.
    if game.is_finished() {
        return Err(ApiError::Finished);
    }
    let song = app.song(today).await?;

    let moved = match chosen {
        Move::Skip => game.skip(),
        Move::Guess(track_id) => {
            // The title and artist come from Deezer by ID, never from the
            // client, so a guess cannot be forged.
            let guessed = match app.0.deezer.track_meta(track_id).await {
                Ok(guessed) if !guessed.title.trim().is_empty() => guessed,
                Ok(_) | Err(DeezerError::NotFound) => return Err(ApiError::UnknownTrack),
                Err(error) => {
                    tracing::warn!(%error, track_id, "could not look up a guessed track");
                    return Err(ApiError::Upstream(
                        "Deezer did not answer, so the guess was not counted. Try again in a moment.",
                    ));
                }
            };
            game.guess(&song.meta, &guessed)
        }
    };
    moved.map_err(|GameError::Finished| ApiError::Finished)?;

    game_response(&app, jar, &headers, &game, &song)
}

// --- GET /api/search ------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SearchParams {
    #[serde(default)]
    q: String,
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
    let Query(params) =
        params.map_err(|_| ApiError::BadRequest("The search needs one `q` parameter."))?;
    let query: String = params.q.trim().chars().take(MAX_QUERY_CHARS).collect();
    if query.chars().count() < MIN_QUERY_CHARS {
        return Ok(Json(Vec::new()));
    }

    let tracks = app
        .0
        .deezer
        .search_tracks(&query, SEARCH_FETCH)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Deezer search failed");
            ApiError::Upstream("The song search is not answering. Try again in a moment.")
        })?;
    Ok(Json(search_hits(&tracks, SEARCH_RESULTS)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        game::{FULL_CLIP_MS, LADDER_MS, TrackMeta},
        mp3::Mp3,
        testutil::{ID3_MARKER, MockDeezer, MockTrack, PREVIEW_FRAMES, synthetic_mp3},
    };
    use axum::{
        body::Body,
        http::{Method, Request},
    };
    use jiff::civil::date;
    use serde_json::{Value, json};
    use std::os::unix::fs::PermissionsExt;
    use tower::ServiceExt;

    const LAUNCH: Date = date(2026, 10, 1);

    /// The song being played. The ID is long so that it cannot turn up by
    /// chance in a `Content-Length` or in the cookie's ciphertext.
    const ANSWER_ID: u64 = 987_654_321;
    /// The same song in another release: a different ID that must still win.
    const ANSWER_OTHER_RELEASE: u64 = 987_654_322;
    const QUEEN: u64 = 10;
    const QUEEN_REMASTER: u64 = 11;
    const UNKNOWN_ID: u64 = 555;

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

    /// The router over a local Deezer stand-in and a temporary data directory.
    struct Harness {
        deezer: MockDeezer,
        app: Router,
        _data_dir: tempfile::TempDir,
    }

    impl Harness {
        async fn start() -> Self {
            let deezer = MockDeezer::start(tracks()).await;
            let data_dir = tempfile::tempdir().unwrap();
            let client = deezer.client();
            let daily = Daily::new(client.clone(), data_dir.path(), ANSWER_ID)
                .with_retry_after(std::time::Duration::ZERO);
            let state = AppState::new(client, daily, LAUNCH, Key::generate());
            Self {
                deezer,
                app: router(state),
                _data_dir: data_dir,
            }
        }

        fn player(&self) -> Player {
            Player {
                app: self.app.clone(),
                cookie: None,
            }
        }
    }

    /// A browser: it keeps the session cookie between requests.
    struct Player {
        app: Router,
        /// `gts_daily=<value>`, as last set by the server.
        cookie: Option<String>,
    }

    struct Reply {
        status: StatusCode,
        headers: HeaderMap,
        body: bytes::Bytes,
    }

    impl Reply {
        fn json(&self) -> Value {
            serde_json::from_slice(&self.body)
                .unwrap_or_else(|error| panic!("not JSON ({error}): {:?}", self.body))
        }

        fn header(&self, name: header::HeaderName) -> &str {
            self.headers
                .get(&name)
                .unwrap_or_else(|| panic!("no {name} header"))
                .to_str()
                .unwrap()
        }

        fn content_length(&self) -> usize {
            self.header(header::CONTENT_LENGTH).parse().unwrap()
        }

        /// Everything a client can see of this response, as text.
        fn visible(&self) -> String {
            format!(
                "{:?}\n{}",
                self.headers,
                String::from_utf8_lossy(&self.body)
            )
        }

        fn assert_error(&self, status: StatusCode, code: &str) {
            assert_eq!(self.status, status, "{}", self.visible());
            let body = self.json();
            assert_eq!(body["error"], code);
            assert!(
                body["message"].as_str().is_some_and(|m| !m.is_empty()),
                "{body}"
            );
        }

        fn assert_no_secrets(&self) {
            let visible = self.visible();
            for secret in SECRETS {
                assert!(
                    !visible.contains(secret),
                    "{secret:?} leaked in:\n{visible}"
                );
            }
        }
    }

    impl Player {
        async fn send(&mut self, mut request: Request<Body>) -> Reply {
            if let Some(cookie) = &self.cookie {
                request
                    .headers_mut()
                    .insert(header::COOKIE, cookie.parse().unwrap());
            }
            let response = self.app.clone().oneshot(request).await.unwrap();
            let (parts, body) = response.into_parts();
            let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            for set in parts.headers.get_all(header::SET_COOKIE) {
                let pair = set.to_str().unwrap().split(';').next().unwrap();
                if pair.starts_with(COOKIE_NAME) {
                    self.cookie = Some(pair.to_owned());
                }
            }
            Reply {
                status: parts.status,
                headers: parts.headers,
                body,
            }
        }

        async fn get(&mut self, uri: &str) -> Reply {
            self.send(Request::get(uri).body(Body::empty()).unwrap())
                .await
        }

        async fn post(&mut self, body: Value) -> Reply {
            self.post_raw("application/json", body.to_string()).await
        }

        async fn post_raw(&mut self, content_type: &str, body: impl Into<Body>) -> Reply {
            let request = Request::builder()
                .method(Method::POST)
                .uri("/api/daily/guess")
                .header(header::CONTENT_TYPE, content_type)
                .body(body.into())
                .unwrap();
            self.send(request).await
        }

        async fn skip(&mut self) -> Reply {
            self.post(json!({ "skip": true })).await
        }

        async fn guess(&mut self, track_id: u64) -> Reply {
            self.post(json!({ "trackId": track_id })).await
        }
    }

    fn reference_mp3() -> Mp3 {
        Mp3::parse(synthetic_mp3(PREVIEW_FRAMES)).unwrap()
    }

    // --- GET /api/daily -----------------------------------------------------

    #[tokio::test]
    async fn health_still_answers() {
        let harness = Harness::start().await;
        let reply = harness.player().get("/api/health").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json(), json!({ "ok": true }));
    }

    #[tokio::test]
    async fn a_first_visit_gets_a_fresh_game_and_a_cookie() {
        let harness = Harness::start().await;
        let mut player = harness.player();
        let reply = player.get("/api/daily").await;

        assert_eq!(reply.status, StatusCode::OK);
        let today = today_utc();
        assert_eq!(
            reply.json(),
            json!({
                "day": today.to_string(),
                "number": game::day_number(LAUNCH, today),
                "ladder": [0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0],
                "attempts": [],
                "status": "playing",
                "clipSeconds": 0.1,
                "answer": null,
            })
        );
        assert_eq!(reply.header(header::CACHE_CONTROL), "no-store");

        let cookie = reply.header(header::SET_COOKIE);
        assert!(cookie.starts_with("gts_daily="), "{cookie}");
        for attribute in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=172800"] {
            assert!(cookie.contains(attribute), "{attribute} missing: {cookie}");
        }
        // Plain HTTP (the dev server): a `Secure` cookie would be dropped.
        assert!(!cookie.contains("Secure"), "{cookie}");
        // Encrypted: the state is not readable in the cookie.
        assert!(!cookie.contains("playing"), "{cookie}");
        assert!(!cookie.contains("attempts"), "{cookie}");
        reply.assert_no_secrets();
    }

    #[tokio::test]
    async fn the_cookie_is_secure_behind_https() {
        let harness = Harness::start().await;
        let mut player = harness.player();
        let request = Request::get("/api/daily")
            .header("x-forwarded-proto", "https")
            .body(Body::empty())
            .unwrap();
        let reply = player.send(request).await;
        assert!(
            reply.header(header::SET_COOKIE).contains("Secure"),
            "{}",
            reply.visible()
        );
    }

    #[tokio::test]
    async fn a_garbage_cookie_is_a_fresh_game() {
        let harness = Harness::start().await;
        let mut player = harness.player();
        player.skip().await;
        let real = player.cookie.clone().unwrap();

        // Not a cookie this server made, then a real one with its end cut off.
        for garbage in [
            "gts_daily=not-even-base64!!".to_owned(),
            "gts_daily=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            real[..real.len() - 6].to_owned(),
        ] {
            player.cookie = Some(garbage.clone());
            let reply = player.get("/api/daily").await;
            assert_eq!(reply.status, StatusCode::OK, "{garbage}");
            let body = reply.json();
            assert_eq!(body["attempts"], json!([]), "{garbage}");
            assert_eq!(body["status"], "playing");
            // And it is replaced by a good one.
            assert_ne!(player.cookie.as_deref(), Some(garbage.as_str()));
        }

        // The untouched cookie still holds the skip.
        player.cookie = Some(real);
        let body = player.get("/api/daily").await.json();
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
    }

    #[tokio::test]
    async fn another_players_key_does_not_decrypt_the_cookie() {
        let first = Harness::start().await;
        let second = Harness::start().await;
        let mut player = first.player();
        player.skip().await;

        // The same cookie, sent to a server with a different key.
        player.app = second.app.clone();
        let body = player.get("/api/daily").await.json();
        assert_eq!(body["attempts"], json!([]));
    }

    // --- moves --------------------------------------------------------------

    #[tokio::test]
    async fn each_skip_adds_an_attempt_and_unlocks_a_longer_clip() {
        let harness = Harness::start().await;
        let mut player = harness.player();
        let mp3 = reference_mp3();
        let mut lengths = Vec::new();

        for (turn, &clip_ms) in LADDER_MS.iter().enumerate() {
            let state = player.get("/api/daily").await.json();
            assert_eq!(state["attempts"].as_array().unwrap().len(), turn);
            assert_eq!(state["status"], "playing");
            assert_eq!(state["clipSeconds"], f64::from(clip_ms) / 1000.0);
            assert_eq!(state["answer"], Value::Null);

            let clip = player.get("/api/daily/audio").await;
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
        let harness = Harness::start().await;
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

        // The cookie remembers it: a reload shows the same finished game.
        assert_eq!(player.get("/api/daily").await.json(), body);

        let clip = player.get("/api/daily/audio").await;
        assert_eq!(clip.body, reference_mp3().prefix(FULL_CLIP_MS));
        assert_eq!(clip.content_length(), PREVIEW_FRAMES * 418);
    }

    #[tokio::test]
    async fn a_move_after_the_end_is_a_conflict() {
        let harness = Harness::start().await;
        let mut player = harness.player();
        for _ in 0..MAX_ATTEMPTS {
            player.skip().await;
        }
        let before = player.get("/api/daily").await.json();
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

        assert_eq!(player.get("/api/daily").await.json(), before);
    }

    #[tokio::test]
    async fn a_wrong_guess_is_recorded_with_the_servers_title_and_artist() {
        let harness = Harness::start().await;
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

        let clip = player.get("/api/daily/audio").await;
        assert_eq!(clip.body, reference_mp3().prefix(300));
    }

    #[tokio::test]
    async fn the_right_song_wins_whatever_release_was_picked() {
        let harness = Harness::start().await;

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

            let clip = player.get("/api/daily/audio").await;
            assert_eq!(clip.content_length(), PREVIEW_FRAMES * 418);
            player
                .skip()
                .await
                .assert_error(StatusCode::CONFLICT, "finished");
        }
    }

    #[tokio::test]
    async fn a_guess_picked_from_search_needs_no_second_lookup() {
        let harness = Harness::start().await;
        let mut player = harness.player();
        player.get("/api/daily").await;
        player.get("/api/search?q=under+pressure").await;
        let after_search = harness.deezer.api_hits();

        let body = player.guess(QUEEN).await.json();
        assert_eq!(body["attempts"][0]["title"], "Under Pressure");
        assert_eq!(harness.deezer.api_hits(), after_search);
    }

    #[tokio::test]
    async fn an_unknown_track_is_not_found_and_costs_no_turn() {
        let harness = Harness::start().await;
        let mut player = harness.player();
        player.skip().await;

        let reply = player.guess(UNKNOWN_ID).await;
        reply.assert_error(StatusCode::NOT_FOUND, "unknown_track");
        reply.assert_no_secrets();

        let body = player.get("/api/daily").await.json();
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
    }

    #[tokio::test]
    async fn malformed_moves_are_bad_requests() {
        let harness = Harness::start().await;
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
        let body = player.get("/api/daily").await.json();
        assert_eq!(body["attempts"], json!([]));
    }

    #[tokio::test]
    async fn a_deezer_failure_during_a_guess_is_an_upstream_error() {
        let harness = Harness::start().await;
        let mut player = harness.player();
        player.get("/api/daily").await;

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
        let harness = Harness::start().await;
        let mut player = harness.player();
        harness.deezer.set_failing(true);

        for uri in ["/api/daily", "/api/daily/audio"] {
            let reply = player.get(uri).await;
            reply.assert_error(StatusCode::BAD_GATEWAY, "upstream");
            reply.assert_no_secrets();
        }
        let reply = player.skip().await;
        reply.assert_error(StatusCode::BAD_GATEWAY, "upstream");

        harness.deezer.set_failing(false);
        assert_eq!(player.get("/api/daily").await.status, StatusCode::OK);
        assert_eq!(player.get("/api/daily/audio").await.status, StatusCode::OK);
    }

    // --- GET /api/daily/audio ----------------------------------------------

    #[tokio::test]
    async fn audio_without_a_cookie_is_the_shortest_clip() {
        let harness = Harness::start().await;
        let mut player = harness.player();
        let clip = player.get("/api/daily/audio?t=1727740800000").await;

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
        let harness = Harness::start().await;
        let mut player = harness.player();
        let request = Request::get("/api/daily/audio")
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
        let harness = Harness::start().await;
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
        let harness = Harness::start().await;
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
        let harness = Harness::start().await;
        harness.deezer.set_failing(true);
        harness
            .player()
            .get("/api/search?q=queen")
            .await
            .assert_error(StatusCode::BAD_GATEWAY, "upstream");
    }

    #[tokio::test]
    async fn unknown_routes_are_json_errors() {
        let harness = Harness::start().await;
        harness
            .player()
            .get("/api/nope")
            .await
            .assert_error(StatusCode::NOT_FOUND, "not_found");
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

    #[test]
    fn the_view_has_no_answer_while_playing() {
        let song = song();
        let today = date(2026, 10, 3);
        let mut game = GameState::new(today);
        game.skip().unwrap();
        game.guess(&song.meta, &TrackMeta::new("Under Pressure", "", "Queen"))
            .unwrap();

        let view = DailyView::new(&game, LAUNCH, &song);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(
            json,
            json!({
                "day": "2026-10-03",
                "number": 3,
                "ladder": [0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0],
                "attempts": [
                    { "kind": "skip" },
                    { "kind": "wrong", "title": "Under Pressure", "artist": "Queen" },
                ],
                "status": "playing",
                "clipSeconds": 1.0,
                "answer": null,
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
        let view = serde_json::to_value(DailyView::new(&won, LAUNCH, &song)).unwrap();
        assert_eq!(view["status"], "won");
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
        let view = serde_json::to_value(DailyView::new(&lost, LAUNCH, &song)).unwrap();
        assert_eq!(view["status"], "lost");
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

    #[test]
    fn https_is_read_from_the_forwarding_header() {
        let with = |value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert("x-forwarded-proto", HeaderValue::from_static(value));
            is_https(&headers)
        };
        assert!(with("https"));
        assert!(with("HTTPS"));
        assert!(with("https, http"));
        assert!(!with("http"));
        assert!(!with("http, https"));
        assert!(!with(""));
        assert!(!is_https(&HeaderMap::new()));
    }

    // --- the cookie ---------------------------------------------------------

    /// The `Set-Cookie` header [`store_game`] produces.
    fn set_cookie(key: &Key, game: &GameState, secure: bool) -> String {
        let jar = store_game(PrivateCookieJar::new(key.clone()), game, secure).unwrap();
        let response = jar.into_response();
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned()
    }

    /// A jar as the server sees it when the browser sends `set_cookie` back.
    fn jar_from(key: &Key, set_cookie: &str) -> PrivateCookieJar {
        let mut headers = HeaderMap::new();
        let pair = set_cookie.split(';').next().unwrap();
        headers.insert(header::COOKIE, pair.parse().unwrap());
        PrivateCookieJar::from_headers(&headers, key.clone())
    }

    #[test]
    fn the_game_survives_the_cookie_round_trip() {
        let key = Key::generate();
        let today = date(2026, 10, 1);
        let mut game = GameState::new(today);
        game.skip().unwrap();
        game.guess(
            &TrackMeta::new("A", "", "B"),
            &TrackMeta::new("Señorita \"Live\"", "", "Beyoncé; x=y"),
        )
        .unwrap();

        let jar = jar_from(&key, &set_cookie(&key, &game, false));
        assert_eq!(load_game(&jar, today), game);
    }

    #[test]
    fn yesterdays_cookie_is_a_fresh_game_today() {
        let key = Key::generate();
        let yesterday = date(2026, 10, 1);
        let today = date(2026, 10, 2);
        let mut game = GameState::new(yesterday);
        for _ in 0..MAX_ATTEMPTS {
            game.skip().unwrap();
        }

        let jar = jar_from(&key, &set_cookie(&key, &game, false));
        assert_eq!(load_game(&jar, yesterday), game);
        assert_eq!(load_game(&jar, today), GameState::new(today));
    }

    #[test]
    fn the_largest_game_still_fits_in_a_cookie() {
        // Seven wrong guesses whose titles and artists are as long as an
        // attempt stores: 80 two-byte characters each.
        let long = "é".repeat(200);
        let answer = TrackMeta::new("A", "", "B");
        let mut game = GameState::new(date(2026, 10, 1));
        for _ in 0..MAX_ATTEMPTS {
            game.guess(&answer, &TrackMeta::new(&long, "", &long))
                .unwrap();
        }
        assert_eq!(game.status(), Status::Lost);

        let key = Key::generate();
        let header = set_cookie(&key, &game, true);
        // Browsers refuse a cookie whose name and value exceed 4096 bytes;
        // stay under that with the attributes counted too.
        assert!(header.len() < 4096, "{} bytes", header.len());
        assert_eq!(load_game(&jar_from(&key, &header), date(2026, 10, 1)), game);
    }

    // --- the cookie key -----------------------------------------------------

    #[test]
    fn hex_round_trips() {
        let bytes: Vec<u8> = (0..=255).collect();
        let text = encode_hex(&bytes);
        assert_eq!(text.len(), 512);
        assert!(text.starts_with("000102") && text.ends_with("fdfeff"));
        assert_eq!(decode_hex(&text), Some(bytes.clone()));
        assert_eq!(decode_hex(&text.to_uppercase()), Some(bytes));
        assert_eq!(decode_hex(""), Some(Vec::new()));
        assert_eq!(decode_hex("abc"), None);
        assert_eq!(decode_hex("zz"), None);
        assert_eq!(decode_hex("+1"), None);
        assert_eq!(decode_hex("éé"), None);
    }

    #[test]
    fn a_key_is_generated_once_and_kept_private() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");

        let first = session_key(None, &data_dir).unwrap();
        let path = data_dir.join("secret.key");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.trim().len(), 128);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        // A restart reads the same key, so cookies survive it.
        let second = session_key(None, &data_dir).unwrap();
        assert_eq!(first.master(), second.master());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn the_secret_variable_wins_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let secret = "0123456789abcdef".repeat(8);
        let key = session_key(Some(&secret), dir.path()).unwrap();
        assert_eq!(encode_hex(key.master()), secret);
        assert!(!dir.path().join("secret.key").exists());

        // The key file's own format works as the variable, newline and all.
        let generated = session_key(None, dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join("secret.key")).unwrap();
        let from_env = session_key(Some(&text), dir.path()).unwrap();
        assert_eq!(generated.master(), from_env.master());
    }

    #[test]
    fn an_unusable_secret_is_an_error_that_does_not_repeat_it() {
        let dir = tempfile::tempdir().unwrap();
        for secret in ["hunter2", &"ab".repeat(63), &"zz".repeat(64)] {
            let error = session_key(Some(secret), dir.path()).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("GTS_SECRET"), "{message}");
            assert!(message.contains("128 hexadecimal"), "{message}");
            assert!(!message.contains(secret), "{message}");
        }
    }

    #[test]
    fn a_damaged_key_file_is_an_error_not_a_new_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        std::fs::write(&path, "truncated").unwrap();

        let error = session_key(None, dir.path()).unwrap_err();
        assert!(format!("{error:#}").contains("secret.key"), "{error:#}");
        // Left alone for the operator to look at.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "truncated");
    }
}
