//! Typed Deezer API client (search, track, preview download) with small in-memory caches.
//!
//! Deezer allows 50 requests per 5 seconds per IP, and autocomplete sends a
//! request for every pause in typing. Three things keep the server under the
//! limit: search results are cached for a few minutes, the title and artist of
//! every track that passes through are remembered so a guess rarely needs a
//! lookup of its own, and a request budget refuses to call Deezer at all once
//! the server is close to the limit. Nothing here retries.

use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use bytes::Bytes;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};

use crate::game::TrackMeta;

const API_BASE: &str = "https://api.deezer.com";

/// Sent with every request, so Deezer can tell who is calling.
const USER_AGENT: &str = concat!(
    "guessthesong/",
    env!("CARGO_PKG_VERSION"),
    " (+https://gts.icyfire.dev)"
);

/// Whole-request limit. A player is waiting on the other end of every call.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a search result list is reused.
const SEARCH_TTL: Duration = Duration::from_secs(10 * 60);
/// Distinct queries remembered at once.
const SEARCH_CACHE_SIZE: usize = 500;

/// How long a track's title and artist are remembered. They do not change;
/// the limit only keeps the cache from holding on to stale entries forever.
const TRACK_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Tracks remembered at once: a few hundred searches' worth.
const TRACK_CACHE_SIZE: usize = 10_000;

/// Deezer's limit is 50 requests in 5 seconds. Stopping at 40 leaves room for
/// requests the budget does not see, such as a second server process during a
/// restart.
const BUDGET_REQUESTS: usize = 40;
const BUDGET_WINDOW: Duration = Duration::from_secs(5);
const _: () = assert!(BUDGET_REQUESTS < 50);

/// Deezer's error code for "no such object".
const CODE_NO_DATA: i64 = 800;

/// Why a Deezer call produced nothing.
#[derive(Debug, thiserror::Error)]
pub enum DeezerError {
    /// Deezer answered that the object does not exist (error code 800).
    #[error("Deezer has no such object")]
    NotFound,
    /// Any other error Deezer reported, including the rate limit (code 4,
    /// "Quota limit exceeded").
    #[error("Deezer reported error {code}: {message}")]
    Api { code: i64, message: String },
    /// The server's own request budget is used up; Deezer was not called.
    #[error("too many Deezer requests in the last few seconds; not sending another")]
    Throttled,
    /// The request failed or timed out, or the HTTP status was not a success.
    #[error("Deezer request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// The body was not the JSON this client expects.
    #[error("unexpected Deezer response: {0}")]
    Decode(#[source] serde_json::Error),
}

/// A track as `/track/{id}` and `/search/track` describe it: the fields the
/// game uses and nothing else.
///
/// Serializing leaves out `preview`: the URL is signed and expires after about
/// 15 minutes, so a stored copy would only be a trap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    #[serde(default, deserialize_with = "null_as_default")]
    pub title: String,
    /// The title without "(Remastered …)" and the like. May be empty.
    #[serde(default, deserialize_with = "null_as_default")]
    pub title_short: String,
    /// Whether Deezer will play the track in this server's country.
    #[serde(default, deserialize_with = "null_as_default")]
    pub readable: bool,
    /// The track's page on deezer.com.
    #[serde(default, deserialize_with = "null_as_default")]
    pub link: String,
    /// Signed URL of the 30-second preview; empty when there is none.
    #[serde(default, deserialize_with = "null_as_default", skip_serializing)]
    pub preview: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub artist: Artist,
    #[serde(default, deserialize_with = "null_as_default")]
    pub album: Album,
}

/// The primary artist of a [`Track`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    #[serde(default, deserialize_with = "null_as_default")]
    pub name: String,
}

/// The album a [`Track`] is on. Deezer leaves cover URLs `null` for albums
/// without artwork; those become empty strings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
    #[serde(default, deserialize_with = "null_as_default")]
    pub title: String,
    /// 56 × 56 px.
    #[serde(default, deserialize_with = "null_as_default")]
    pub cover_small: String,
    /// 250 × 250 px.
    #[serde(default, deserialize_with = "null_as_default")]
    pub cover_medium: String,
    /// 500 × 500 px.
    #[serde(default, deserialize_with = "null_as_default")]
    pub cover_big: String,
}

impl Track {
    /// What the game needs to match this track against another.
    pub fn meta(&self) -> TrackMeta {
        TrackMeta::new(&self.title, &self.title_short, &self.artist.name)
    }

    /// Whether the track has a preview to play. Deezer marks a withdrawn track
    /// with both `readable: false` and an empty `preview`; check both.
    pub fn is_playable(&self) -> bool {
        self.readable && !self.preview.is_empty()
    }
}

/// Reads JSON `null` as the type's default, so one odd track does not make a
/// whole result list fail to parse.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// The body of `/search/track`.
#[derive(Deserialize)]
struct SearchPage {
    #[serde(default, deserialize_with = "null_as_default")]
    data: Vec<Track>,
}

/// The `error` object Deezer puts in an HTTP 200 body when a call fails.
#[derive(Default, Deserialize)]
struct Fault {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

/// Decodes a Deezer response body.
///
/// Deezer reports failures as HTTP 200 with a body such as
/// `{"error":{"type":"DataException","message":"no data","code":800}}`, so the
/// status says nothing and the body has to be checked first.
fn parse_body<T: DeserializeOwned>(body: &[u8]) -> Result<T, DeezerError> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(DeezerError::Decode)?;
    if let Some(error) = value.get("error") {
        let fault = Fault::deserialize(error).unwrap_or_default();
        return Err(if fault.code == CODE_NO_DATA {
            DeezerError::NotFound
        } else {
            DeezerError::Api {
                code: fault.code,
                message: fault.message,
            }
        });
    }
    serde_json::from_value(value).map_err(DeezerError::Decode)
}

/// The form of a search query the cache is keyed on: lowercase, single spaces.
/// "  Under   PRESSURE " and "under pressure" are one request.
fn normalize_query(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A map whose entries expire after `ttl` and which never holds more than
/// `capacity` of them. The clock is passed in so it can be tested.
struct TtlCache<K, V> {
    ttl: Duration,
    capacity: usize,
    entries: HashMap<K, (Instant, V)>,
}

impl<K: Eq + Hash + Clone, V: Clone> TtlCache<K, V> {
    fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            ttl,
            capacity,
            entries: HashMap::new(),
        }
    }

    fn is_fresh(&self, stored: Instant, now: Instant) -> bool {
        now.saturating_duration_since(stored) < self.ttl
    }

    /// The value for `key`, unless it is missing or older than the TTL.
    fn get(&self, key: &K, now: Instant) -> Option<V> {
        self.entries
            .get(key)
            .filter(|(stored, _)| self.is_fresh(*stored, now))
            .map(|(_, value)| value.clone())
    }

    /// Stores `value`, making room first if the cache is full: expired entries
    /// go, and if that frees nothing, the oldest one does.
    fn insert(&mut self, key: K, value: V, now: Instant) {
        if self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            let ttl = self.ttl;
            self.entries
                .retain(|_, (stored, _)| now.saturating_duration_since(*stored) < ttl);
            if self.entries.len() >= self.capacity {
                let oldest = self
                    .entries
                    .iter()
                    .min_by_key(|(_, (stored, _))| *stored)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    self.entries.remove(&oldest);
                }
            }
        }
        self.entries.insert(key, (now, value));
    }
}

/// Counts the API requests sent in the last [`BUDGET_WINDOW`] and refuses new
/// ones beyond [`BUDGET_REQUESTS`]. Without it, a client hammering the search
/// endpoint with distinct queries would get this server's IP rate-limited for
/// every player.
struct RequestBudget {
    sent: VecDeque<Instant>,
}

impl RequestBudget {
    fn new() -> Self {
        Self {
            sent: VecDeque::with_capacity(BUDGET_REQUESTS),
        }
    }

    /// Records a request at `now` and returns `true`, or returns `false` when
    /// the window is full.
    fn try_take(&mut self, now: Instant) -> bool {
        while let Some(&oldest) = self.sent.front() {
            if now.saturating_duration_since(oldest) < BUDGET_WINDOW {
                break;
            }
            self.sent.pop_front();
        }
        if self.sent.len() >= BUDGET_REQUESTS {
            return false;
        }
        self.sent.push_back(now);
        true
    }
}

/// Locks `mutex`, carrying on after a panic elsewhere: the caches hold no
/// invariant a half-finished update could break.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The Deezer client. Cheap to clone; all clones share one connection pool
/// and one set of caches.
#[derive(Clone)]
pub struct Deezer {
    inner: Arc<Inner>,
}

/// Search results by normalized query and limit.
type SearchCache = TtlCache<(String, u32), Arc<[Track]>>;

struct Inner {
    http: reqwest::Client,
    /// `https://api.deezer.com`, or a local stand-in under test.
    base: String,
    searches: Mutex<SearchCache>,
    /// Title and artist by track ID, from every search result and lookup.
    tracks: Mutex<TtlCache<u64, TrackMeta>>,
    budget: Mutex<RequestBudget>,
}

impl Deezer {
    /// A client for the real Deezer API.
    pub fn new() -> Result<Self, DeezerError> {
        Self::with_base(API_BASE)
    }

    /// A client for a Deezer stand-in at `base` (no trailing slash).
    #[cfg(test)]
    pub fn for_tests(base: &str) -> Self {
        Self::with_base(base).expect("building the HTTP client")
    }

    fn with_base(base: &str) -> Result<Self, DeezerError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                base: base.to_owned(),
                searches: Mutex::new(TtlCache::new(SEARCH_TTL, SEARCH_CACHE_SIZE)),
                tracks: Mutex::new(TtlCache::new(TRACK_TTL, TRACK_CACHE_SIZE)),
                budget: Mutex::new(RequestBudget::new()),
            }),
        })
    }

    /// Up to `limit` tracks matching `query`, best match first, as Deezer
    /// ranks them. Repeated queries within a few minutes are answered from
    /// memory.
    pub async fn search_tracks(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Arc<[Track]>, DeezerError> {
        let key = (normalize_query(query), limit);
        if let Some(tracks) = lock(&self.inner.searches).get(&key, Instant::now()) {
            return Ok(tracks);
        }

        let limit_text = limit.to_string();
        let page: SearchPage = self
            .get_json(
                "/search/track",
                &[("q", key.0.as_str()), ("limit", limit_text.as_str())],
            )
            .await?;
        let tracks: Arc<[Track]> = page.data.into();

        let now = Instant::now();
        {
            let mut known = lock(&self.inner.tracks);
            for track in tracks.iter() {
                known.insert(track.id, track.meta(), now);
            }
        }
        lock(&self.inner.searches).insert(key, Arc::clone(&tracks), now);
        Ok(tracks)
    }

    /// The track with this ID, fetched from Deezer every time: the `preview`
    /// URL in it is only good for about 15 minutes.
    pub async fn track(&self, id: u64) -> Result<Track, DeezerError> {
        let track: Track = self.get_json(&format!("/track/{id}"), &[]).await?;
        lock(&self.inner.tracks).insert(track.id, track.meta(), Instant::now());
        Ok(track)
    }

    /// Title and artist of the track with this ID: from memory when a search
    /// or lookup has already seen it, which is the normal case for a guess
    /// picked from autocomplete, and from Deezer otherwise.
    pub async fn track_meta(&self, id: u64) -> Result<TrackMeta, DeezerError> {
        if let Some(meta) = lock(&self.inner.tracks).get(&id, Instant::now()) {
            return Ok(meta);
        }
        Ok(self.track(id).await?.meta())
    }

    /// Downloads a preview MP3. The caller checks that it is one.
    pub async fn download_preview(&self, url: &str) -> Result<Bytes, DeezerError> {
        let download = async {
            let response = self.inner.http.get(url).send().await?.error_for_status()?;
            response.bytes().await
        };
        // The URL carries a signature and identifies the song; keep it out of
        // error messages.
        download
            .await
            .map_err(|error| DeezerError::Http(error.without_url()))
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, DeezerError> {
        if !lock(&self.inner.budget).try_take(Instant::now()) {
            return Err(DeezerError::Throttled);
        }
        let url = format!("{}{path}", self.inner.base);
        let response = self
            .inner
            .http
            .get(url)
            .query(query)
            .send()
            .await?
            .error_for_status()?;
        parse_body(&response.bytes().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed `/track/{id}` body, with the escaped slashes Deezer sends.
    const TRACK_JSON: &str = r#"{
        "id": 1234567,
        "readable": true,
        "title": "Under Pressure (Remastered 2011)",
        "title_short": "Under Pressure",
        "title_version": "(Remastered 2011)",
        "isrc": "GBUM71029605",
        "link": "https:\/\/www.deezer.com\/track\/1234567",
        "duration": 248,
        "rank": 912345,
        "preview": "https:\/\/cdnt-preview.dzcdn.net\/api\/1\/1\/a\/b\/c\/0\/abc.mp3?hdnea=exp=1~hmac=ff",
        "artist": { "id": 412, "name": "Queen", "type": "artist" },
        "album": {
            "id": 99,
            "title": "Hot Space",
            "cover": "https:\/\/api.deezer.com\/album\/99\/image",
            "cover_small": "https:\/\/cdn-images.dzcdn.net\/images\/cover\/abc\/56x56-000000-80-0-0.jpg",
            "cover_medium": "https:\/\/cdn-images.dzcdn.net\/images\/cover\/abc\/250x250-000000-80-0-0.jpg",
            "cover_big": "https:\/\/cdn-images.dzcdn.net\/images\/cover\/abc\/500x500-000000-80-0-0.jpg",
            "type": "album"
        },
        "type": "track"
    }"#;

    fn t0() -> Instant {
        Instant::now()
    }

    // --- JSON -------------------------------------------------------------

    #[test]
    fn track_json_maps_to_a_track() {
        let track: Track = parse_body(TRACK_JSON.as_bytes()).unwrap();
        assert_eq!(track.id, 1_234_567);
        assert_eq!(track.title, "Under Pressure (Remastered 2011)");
        assert_eq!(track.title_short, "Under Pressure");
        assert_eq!(track.artist.name, "Queen");
        assert_eq!(track.album.title, "Hot Space");
        assert_eq!(track.link, "https://www.deezer.com/track/1234567");
        assert!(track.album.cover_small.ends_with("56x56-000000-80-0-0.jpg"));
        assert!(track.album.cover_big.ends_with("500x500-000000-80-0-0.jpg"));
        assert!(track.preview.starts_with("https://cdnt-preview.dzcdn.net/"));
        assert!(track.is_playable());
        assert_eq!(
            track.meta(),
            TrackMeta::new(
                "Under Pressure (Remastered 2011)",
                "Under Pressure",
                "Queen"
            )
        );
    }

    #[test]
    fn search_json_maps_to_tracks() {
        let body =
            format!(r#"{{ "data": [{TRACK_JSON}, {TRACK_JSON}], "total": 2, "next": "x" }}"#);
        let page: SearchPage = parse_body(body.as_bytes()).unwrap();
        assert_eq!(page.data.len(), 2);
        assert_eq!(page.data[1].artist.name, "Queen");

        let empty: SearchPage = parse_body(br#"{"data":[],"total":0}"#).unwrap();
        assert!(empty.data.is_empty());
    }

    #[test]
    fn null_and_missing_fields_become_defaults() {
        let body = br#"{
            "id": 5, "title": "Song", "readable": false, "preview": "",
            "artist": { "name": "Someone" },
            "album": { "title": "LP", "cover_small": null, "cover_medium": null, "cover_big": null }
        }"#;
        let track: Track = parse_body(body).unwrap();
        assert_eq!(track.title_short, "");
        assert_eq!(track.link, "");
        assert_eq!(track.album.cover_small, "");
        assert!(!track.is_playable());

        let bare: Track = parse_body(br#"{ "id": 6, "title": null, "artist": null }"#).unwrap();
        assert_eq!(bare.title, "");
        assert_eq!(bare.artist, Artist::default());
        assert_eq!(bare.album, Album::default());
    }

    #[test]
    fn a_track_needs_both_readable_and_a_preview_to_be_playable() {
        let mut track: Track = parse_body(TRACK_JSON.as_bytes()).unwrap();
        track.readable = false;
        assert!(!track.is_playable());
        track.readable = true;
        track.preview.clear();
        assert!(!track.is_playable());
    }

    #[test]
    fn an_error_body_with_code_800_is_not_found() {
        let body = br#"{"error":{"type":"DataException","message":"no data","code":800}}"#;
        assert!(matches!(
            parse_body::<Track>(body),
            Err(DeezerError::NotFound)
        ));
        // The search shape must not swallow it as an empty result either.
        assert!(matches!(
            parse_body::<SearchPage>(body),
            Err(DeezerError::NotFound)
        ));
    }

    #[test]
    fn the_rate_limit_body_is_an_api_error() {
        let body = br#"{"error":{"type":"Exception","message":"Quota limit exceeded","code":4}}"#;
        match parse_body::<SearchPage>(body) {
            Err(DeezerError::Api { code, message }) => {
                assert_eq!(code, 4);
                assert_eq!(message, "Quota limit exceeded");
            }
            other => panic!(
                "expected an API error, got {:?}",
                other.map(|page| page.data)
            ),
        }
    }

    #[test]
    fn an_error_body_of_unknown_shape_is_still_an_error() {
        assert!(matches!(
            parse_body::<Track>(br#"{"error":"nope"}"#),
            Err(DeezerError::Api { code: 0, .. })
        ));
    }

    #[test]
    fn a_body_that_is_not_the_expected_json_is_a_decode_error() {
        assert!(matches!(
            parse_body::<Track>(b"<html>Bad gateway</html>"),
            Err(DeezerError::Decode(_))
        ));
        assert!(matches!(
            parse_body::<Track>(br#"{"title":"no id"}"#),
            Err(DeezerError::Decode(_))
        ));
    }

    #[test]
    fn a_stored_track_has_no_preview_url() {
        let track: Track = parse_body(TRACK_JSON.as_bytes()).unwrap();
        let stored = serde_json::to_string(&track).unwrap();
        assert!(!stored.contains("preview"), "{stored}");
        assert!(!stored.contains("hmac"), "{stored}");

        let reloaded: Track = serde_json::from_str(&stored).unwrap();
        assert_eq!(reloaded.preview, "");
        assert_eq!(reloaded.meta(), track.meta());
        assert_eq!(reloaded.album, track.album);
    }

    // --- query normalization -------------------------------------------------

    #[test]
    fn queries_are_normalized_for_the_cache() {
        assert_eq!(normalize_query("  Under   PRESSURE "), "under pressure");
        assert_eq!(normalize_query("under pressure"), "under pressure");
        assert_eq!(normalize_query("Beyoncé\tHALO"), "beyoncé halo");
        assert_eq!(normalize_query("   "), "");
    }

    // --- TTL cache ----------------------------------------------------------

    #[test]
    fn cache_entries_expire_after_the_ttl() {
        let start = t0();
        let mut cache = TtlCache::new(Duration::from_secs(60), 10);
        cache.insert("a", 1, start);
        assert_eq!(cache.get(&"a", start), Some(1));
        assert_eq!(cache.get(&"a", start + Duration::from_secs(59)), Some(1));
        assert_eq!(cache.get(&"a", start + Duration::from_secs(60)), None);
        assert_eq!(cache.get(&"missing", start), None);
    }

    #[test]
    fn a_full_cache_drops_expired_entries_first() {
        let start = t0();
        let mut cache = TtlCache::new(Duration::from_secs(60), 2);
        cache.insert("old", 1, start);
        cache.insert("recent", 2, start + Duration::from_secs(50));

        let later = start + Duration::from_secs(70);
        cache.insert("new", 3, later);
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.get(&"recent", later), Some(2));
        assert_eq!(cache.get(&"new", later), Some(3));
        assert!(!cache.entries.contains_key("old"));
    }

    #[test]
    fn a_full_cache_of_fresh_entries_drops_the_oldest() {
        let start = t0();
        let mut cache = TtlCache::new(Duration::from_secs(60), 3);
        for (age, key) in ["a", "b", "c"].into_iter().enumerate() {
            cache.insert(key, age, start + Duration::from_secs(age as u64));
        }
        let now = start + Duration::from_secs(10);
        cache.insert("d", 9, now);

        assert_eq!(cache.entries.len(), 3);
        assert_eq!(cache.get(&"a", now), None);
        assert_eq!(cache.get(&"b", now), Some(1));
        assert_eq!(cache.get(&"d", now), Some(9));
    }

    #[test]
    fn rewriting_a_key_in_a_full_cache_evicts_nothing() {
        let start = t0();
        let mut cache = TtlCache::new(Duration::from_secs(60), 2);
        cache.insert("a", 1, start);
        cache.insert("b", 2, start);
        cache.insert("a", 3, start + Duration::from_secs(1));
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.get(&"a", start + Duration::from_secs(1)), Some(3));
        assert_eq!(cache.get(&"b", start + Duration::from_secs(1)), Some(2));
    }

    // --- request budget -----------------------------------------------------

    #[test]
    fn the_budget_stops_before_deezers_limit_and_refills() {
        let start = t0();
        let mut budget = RequestBudget::new();
        for _ in 0..BUDGET_REQUESTS {
            assert!(budget.try_take(start));
        }
        assert!(!budget.try_take(start));
        assert!(!budget.try_take(start + BUDGET_WINDOW - Duration::from_millis(1)));
        // Once the window has passed, the whole budget is back.
        for _ in 0..BUDGET_REQUESTS {
            assert!(budget.try_take(start + BUDGET_WINDOW));
        }
        assert!(!budget.try_take(start + BUDGET_WINDOW));
    }
}
