//! Test support: synthetic MP3 data and a local stand-in for the Deezer API,
//! so handler and loader tests never touch the network.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::header::CONTENT_TYPE,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::deezer::Deezer;

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
