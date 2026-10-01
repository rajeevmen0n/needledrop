//! Test support: synthetic MP3 data, a local stand-in for the Deezer API, so
//! handler and loader tests never touch the network, and the harness the
//! handler tests share: the whole router, a browser that keeps its cookie, a
//! store that always fails, and ready-made games.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{
        HeaderMap, Method, Request, StatusCode,
        header::{self, CONTENT_TYPE},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use axum_extra::extract::cookie::{Cookie, Key, PrivateCookieJar};
use jiff::civil::{Date, date};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;

use crate::{
    daily::Daily,
    deezer::Deezer,
    game::{GameState, MAX_ATTEMPTS, Status, TrackMeta},
    player,
    routes::{AppState, router},
    store::{Genres, MemoryStore, NewSong, PlayerId, PoolSong, Section, Store, StoreError},
};

/// Frames in a real Deezer preview (29.988 s).
pub const PREVIEW_FRAMES: usize = 1148;

/// Text placed in the ID3 tag of every synthetic MP3. It must never reach a
/// client: the audio handler serves frames only.
pub const ID3_MARKER: &[u8] = b"TIT2-secret-title-in-the-tag";

/// An MP3 shaped like a Deezer preview: an ID3v2 tag, then `frames` frames of
/// MPEG-1 Layer III at 128 kbps / 44.1 kHz, 418 bytes each.
pub fn synthetic_mp3(frames: usize) -> Vec<u8> {
    const HEADER: [u8; 4] = [0xFF, 0xFB, 0x92, 0x64];
    const FRAME_LEN: usize = 418;

    let mut bytes = vec![b'I', b'D', b'3', 4, 0, 0, 0, 0, 0, ID3_MARKER.len() as u8];
    bytes.extend_from_slice(ID3_MARKER);
    for index in 0..frames {
        bytes.extend_from_slice(&HEADER);
        bytes.resize(bytes.len() + FRAME_LEN - HEADER.len(), (index % 200) as u8);
    }
    bytes
}

/// What the stand-in serves as a track's preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preview {
    /// A synthetic MP3 of [`PREVIEW_FRAMES`] frames.
    Mp3,
    /// An HTML error page, as the CDN sends for an expired URL.
    Html,
    /// No preview: `readable: false` and an empty URL, a withdrawn track.
    None,
}

/// A track the stand-in knows.
#[derive(Debug, Clone)]
pub struct MockTrack {
    id: u64,
    title: String,
    title_short: String,
    artist: String,
    album: String,
    preview: Preview,
}

impl MockTrack {
    pub fn new(id: u64, title: &str, artist: &str) -> Self {
        Self {
            id,
            title: title.to_owned(),
            title_short: title.to_owned(),
            artist: artist.to_owned(),
            album: format!("{title} (album)"),
            preview: Preview::Mp3,
        }
    }

    pub fn title_short(mut self, title_short: &str) -> Self {
        self.title_short = title_short.to_owned();
        self
    }

    pub fn album(mut self, album: &str) -> Self {
        self.album = album.to_owned();
        self
    }

    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = preview;
        self
    }

    /// The track as Deezer's JSON, with the preview served from `base`.
    pub fn json(&self, base: &str) -> Value {
        let id = self.id;
        let cover = |size: &str| {
            format!("https://cdn-images.dzcdn.net/images/cover/cover{id}/{size}-000000-80-0-0.jpg")
        };
        let playable = self.preview != Preview::None;
        json!({
            "id": id,
            "readable": playable,
            "title": self.title,
            "title_short": self.title_short,
            "link": format!("https://www.deezer.com/track/{id}"),
            "duration": 230,
            "rank": 500_000,
            "preview": if playable { format!("{base}/preview/{id}?hdnea=exp=1~hmac=ff") } else { String::new() },
            "artist": { "id": 1, "name": self.artist, "type": "artist" },
            "album": {
                "id": 2,
                "title": self.album,
                "cover_small": cover("56x56"),
                "cover_medium": cover("250x250"),
                "cover_big": cover("500x500"),
                "type": "album",
            },
            "type": "track",
        })
    }
}

struct MockState {
    base: String,
    tracks: Vec<MockTrack>,
    by_id: HashMap<u64, usize>,
    api_hits: AtomicUsize,
    failing: AtomicBool,
}

impl MockState {
    /// Counts an API request. `Some` is the rate-limit body to answer with
    /// while the stand-in is told to fail.
    fn hit(&self) -> Option<Json<Value>> {
        self.api_hits.fetch_add(1, Ordering::SeqCst);
        self.failing.load(Ordering::SeqCst).then(|| {
            Json(json!({
                "error": { "type": "Exception", "message": "Quota limit exceeded", "code": 4 }
            }))
        })
    }
}

/// A Deezer stand-in listening on a local port: `/track/{id}`,
/// `/search/track` and the preview downloads. Stops when dropped.
pub struct MockDeezer {
    state: Arc<MockState>,
    server: JoinHandle<()>,
}

impl MockDeezer {
    pub async fn start(tracks: Vec<MockTrack>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let by_id = tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (track.id, index))
            .collect();
        let state = Arc::new(MockState {
            base,
            tracks,
            by_id,
            api_hits: AtomicUsize::new(0),
            failing: AtomicBool::new(false),
        });
        let app = Router::new()
            .route("/track/{id}", get(track))
            .route("/search/track", get(search))
            .route("/preview/{id}", get(preview))
            .with_state(Arc::clone(&state));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { state, server }
    }

    /// A client pointed at this stand-in, with caches of its own.
    pub fn client(&self) -> Deezer {
        Deezer::for_tests(&self.state.base)
    }

    /// API requests received so far (track lookups and searches, not preview
    /// downloads).
    pub fn api_hits(&self) -> usize {
        self.state.api_hits.load(Ordering::SeqCst)
    }

    /// While failing, every API request gets Deezer's rate-limit error body.
    pub fn set_failing(&self, failing: bool) {
        self.state.failing.store(failing, Ordering::SeqCst);
    }
}

impl Drop for MockDeezer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn no_data() -> Json<Value> {
    Json(json!({ "error": { "type": "DataException", "message": "no data", "code": 800 } }))
}

async fn track(State(state): State<Arc<MockState>>, Path(id): Path<u64>) -> Json<Value> {
    if let Some(failure) = state.hit() {
        return failure;
    }
    match state.by_id.get(&id) {
        Some(&index) => Json(state.tracks[index].json(&state.base)),
        None => no_data(),
    }
}

/// Tracks whose title or artist contain every word of `q`, in the order they
/// were given to [`MockDeezer::start`].
async fn search(
    State(state): State<Arc<MockState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    if let Some(failure) = state.hit() {
        return failure;
    }
    let query = params
        .get("q")
        .map(|q| q.to_lowercase())
        .unwrap_or_default();
    let limit = params
        .get("limit")
        .and_then(|limit| limit.parse().ok())
        .unwrap_or(25);
    let data: Vec<Value> = state
        .tracks
        .iter()
        .filter(|track| {
            let text = format!("{} {}", track.title, track.artist).to_lowercase();
            query.split_whitespace().all(|word| text.contains(word))
        })
        .take(limit)
        .map(|track| track.json(&state.base))
        .collect();
    Json(json!({ "total": data.len(), "data": data }))
}

async fn preview(State(state): State<Arc<MockState>>, Path(id): Path<u64>) -> Response {
    let kind = state
        .by_id
        .get(&id)
        .map(|&index| state.tracks[index].preview);
    match kind {
        Some(Preview::Mp3) => (
            [(CONTENT_TYPE, "audio/mpeg")],
            synthetic_mp3(PREVIEW_FRAMES),
        )
            .into_response(),
        _ => (
            [(CONTENT_TYPE, "text/html")],
            "<html><body>Access denied</body></html>",
        )
            .into_response(),
    }
}

// --- games ----------------------------------------------------------------------

/// A game on `day` that is still being played after `misses` skips (at most
/// six: the seventh loses).
pub fn playing_game(day: Date, misses: usize) -> GameState {
    let mut game = GameState::new(day);
    for _ in 0..misses {
        game.skip().unwrap();
    }
    assert_eq!(
        game.status(),
        Status::Playing,
        "{misses} misses end the game"
    );
    game
}

/// A game on `day` that was won after `misses` skips, so on try `misses + 1`.
pub fn won_game(day: Date, misses: usize) -> GameState {
    let mut game = playing_game(day, misses);
    let song = TrackMeta::new("The Song", "", "Someone");
    game.guess(&song, &song).unwrap();
    game
}

/// A game on `day` that was lost: seven skips.
pub fn lost_game(day: Date) -> GameState {
    let mut game = playing_game(day, MAX_ATTEMPTS - 1);
    game.skip().unwrap();
    game
}

// --- the router under test ------------------------------------------------------

/// Day 1 of the game in every [`Harness`].
pub const LAUNCH: Date = date(2026, 10, 1);

/// The whole router over a Deezer stand-in, a store the test can reach and a
/// temporary data directory.
pub struct Harness {
    pub deezer: MockDeezer,
    pub store: Arc<dyn Store>,
    pub app: Router,
    /// The cookie key, so a test can look inside a cookie or make one.
    key: Key,
    _data_dir: tempfile::TempDir,
}

impl Harness {
    /// A server whose Deezer knows `tracks`, that plays `answer` and keeps
    /// its data in an empty [`MemoryStore`].
    pub async fn start(tracks: Vec<MockTrack>, answer: u64) -> Self {
        Self::with_store(tracks, answer, Arc::new(MemoryStore::new())).await
    }

    /// The same over a store of the test's choosing.
    pub async fn with_store(tracks: Vec<MockTrack>, answer: u64, store: Arc<dyn Store>) -> Self {
        let deezer = MockDeezer::start(tracks).await;
        let data_dir = tempfile::tempdir().unwrap();
        let client = deezer.client();
        let daily = Daily::new(client.clone(), data_dir.path(), answer)
            .with_retry_after(std::time::Duration::ZERO);
        let key = Key::generate();
        let state = AppState::new(client, daily, Arc::clone(&store), LAUNCH, key.clone());
        Self {
            deezer,
            store,
            app: router(state),
            key,
            _data_dir: data_dir,
        }
    }

    /// A browser that has not been here before.
    pub fn player(&self) -> Player {
        Player {
            app: self.app.clone(),
            cookie: None,
        }
    }

    /// A browser that already holds the cookie of the player `id`.
    pub fn player_with_id(&self, id: &PlayerId) -> Player {
        Player {
            app: self.app.clone(),
            cookie: Some(self.cookie_holding(player::COOKIE_NAME, id.as_str())),
        }
    }

    /// `name=<value>` as a browser would send it back: `value` encrypted
    /// with this server's key under the cookie `name`.
    pub fn cookie_holding(&self, name: &str, value: &str) -> String {
        let jar = PrivateCookieJar::new(self.key.clone())
            .add(Cookie::new(name.to_owned(), value.to_owned()));
        let response = jar.into_response();
        let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        set_cookie.split(';').next().unwrap().to_owned()
    }

    /// What is inside the player cookie the browser holds, decrypted, if it
    /// holds one this server can read.
    pub fn cookie_plaintext(&self, player: &Player) -> Option<String> {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, player.cookie.as_ref()?.parse().ok()?);
        PrivateCookieJar::from_headers(&headers, self.key.clone())
            .get(player::COOKIE_NAME)
            .map(|cookie| cookie.value().to_owned())
    }

    /// Who the browser is to the server: the ID in its cookie.
    pub fn player_id(&self, player: &Player) -> Option<PlayerId> {
        PlayerId::parse(&self.cookie_plaintext(player)?)
    }

    /// Sends a request without a cookie.
    pub async fn send(&self, request: Request<Body>) -> Reply {
        self.player().send(request).await
    }

    pub async fn get(&self, uri: &str) -> Reply {
        self.player().get(uri).await
    }

    pub async fn delete(&self, uri: &str) -> Reply {
        self.send(Request::delete(uri).body(Body::empty()).unwrap())
            .await
    }
}

/// A browser: it keeps the player cookie between requests.
pub struct Player {
    pub app: Router,
    /// `gts_player=<value>`, as last set by the server.
    pub cookie: Option<String>,
}

impl Player {
    pub async fn send(&mut self, mut request: Request<Body>) -> Reply {
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
            if pair.starts_with(player::COOKIE_NAME) {
                self.cookie = Some(pair.to_owned());
            }
        }
        Reply {
            status: parts.status,
            headers: parts.headers,
            body,
        }
    }

    pub async fn get(&mut self, uri: &str) -> Reply {
        self.send(Request::get(uri).body(Body::empty()).unwrap())
            .await
    }

    /// `POST /api/daily/guess` with a JSON body.
    pub async fn post(&mut self, body: Value) -> Reply {
        self.post_raw("application/json", body.to_string()).await
    }

    pub async fn post_raw(&mut self, content_type: &str, body: impl Into<Body>) -> Reply {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/daily/guess")
            .header(header::CONTENT_TYPE, content_type)
            .body(body.into())
            .unwrap();
        self.send(request).await
    }

    pub async fn skip(&mut self) -> Reply {
        self.post(json!({ "skip": true })).await
    }

    pub async fn guess(&mut self, track_id: u64) -> Reply {
        self.post(json!({ "trackId": track_id })).await
    }

    /// `DELETE /api/player`: Clear my data.
    pub async fn clear(&mut self) -> Reply {
        self.send(Request::delete("/api/player").body(Body::empty()).unwrap())
            .await
    }
}

/// A response, read to the end.
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: bytes::Bytes,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("not JSON ({error}): {:?}", self.body))
    }

    pub fn header(&self, name: header::HeaderName) -> &str {
        self.headers
            .get(&name)
            .unwrap_or_else(|| panic!("no {name} header"))
            .to_str()
            .unwrap()
    }

    pub fn content_length(&self) -> usize {
        self.header(header::CONTENT_LENGTH).parse().unwrap()
    }

    /// Everything a client can see of this response, as text: the headers
    /// (the cookie among them) and the body.
    pub fn visible(&self) -> String {
        format!(
            "{:?}\n{}",
            self.headers,
            String::from_utf8_lossy(&self.body)
        )
    }

    pub fn assert_no_store(&self) {
        assert_eq!(
            self.headers
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store"),
            "{}",
            self.visible()
        );
    }

    /// The error body every route uses, with its status, and never cached.
    pub fn assert_error(&self, status: StatusCode, code: &str) {
        assert_eq!(self.status, status, "{}", self.visible());
        let body = self.json();
        assert_eq!(body["error"], code);
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "{body}"
        );
        self.assert_no_store();
    }

    /// Nothing a client can see of this response contains any of `secrets`.
    pub fn assert_lacks(&self, secrets: &[&str]) {
        let visible = self.visible();
        for secret in secrets {
            assert!(
                !visible.contains(secret),
                "{secret:?} leaked in:\n{visible}"
            );
        }
    }
}

// --- a store that fails ---------------------------------------------------------

/// A store whose every operation fails, with a message no client may see
/// ("fire" and the file name are what the tests look for in a response).
pub struct BrokenStore;

const BROKEN: &str = "disk on fire at /var/lib/needledrop.db";

fn broken<T>() -> Result<T, StoreError> {
    Err(StoreError::new("pretending", BROKEN))
}

#[async_trait]
impl Store for BrokenStore {
    async fn songs(&self) -> Result<Vec<PoolSong>, StoreError> {
        broken()
    }
    async fn song(&self, _: u64) -> Result<Option<PoolSong>, StoreError> {
        broken()
    }
    async fn add_song(&self, _: NewSong, _: Genres) -> Result<PoolSong, StoreError> {
        broken()
    }
    async fn set_song_genres(&self, _: u64, _: Genres) -> Result<Option<PoolSong>, StoreError> {
        broken()
    }
    async fn remove_song(&self, _: u64) -> Result<bool, StoreError> {
        broken()
    }
    async fn set_preview_failed_on(&self, _: u64, _: Option<Date>) -> Result<bool, StoreError> {
        broken()
    }
    async fn game(
        &self,
        _: &PlayerId,
        _: Section,
        _: Date,
    ) -> Result<Option<GameState>, StoreError> {
        broken()
    }
    async fn save_game(&self, _: &PlayerId, _: Section, _: &GameState) -> Result<(), StoreError> {
        broken()
    }
    async fn games(&self, _: &PlayerId, _: Section) -> Result<Vec<GameState>, StoreError> {
        broken()
    }
    async fn delete_player(&self, _: &PlayerId) -> Result<usize, StoreError> {
        broken()
    }
}
