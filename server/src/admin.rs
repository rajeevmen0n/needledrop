//! The admin API under `/api/admin`: the day clock, today's picks, the song pool, and the search that finds songs for it.
//!
//! **Nothing here is protected.** That is the owner's choice for now, and it
//! has to change before the server is reachable by anyone else: these routes
//! show the day's answers, replace them, move the day for every player, wipe
//! every player's games and edit the pool. The anti-leak rule of
//! [`crate::routes`] is about the player's routes and does not apply to
//! these.
//!
//! Adding a song never checks that it can be played. Two callers share the
//! route, the admin page and a bulk-add script, and each validates before it
//! calls; a song that slips through without a preview is skipped by the daily
//! pick. The search reports `playable` per result so the page can do its part.
//!
//! The three routes that change the day or its songs (re-roll, next day,
//! reset) do the change in [`crate::daily`], which keeps it apart from the
//! players' requests, and then answer with the state as `GET /api/admin/state`
//! would show it.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::{
    deezer::{DeezerError, Track},
    game,
    pick::PICK_ORDER,
    routes::{ApiError, AppState, SearchParams, daily_failed, find_tracks, no_store, store_failed},
    store::{Genre, Genres, NewSong, PoolSong, Section},
};

/// The admin routes, to be merged into the server's router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/state", get(state))
        .route("/api/admin/reroll", post(reroll))
        .route("/api/admin/next-day", post(next_day))
        .route("/api/admin/reset", post(reset))
        .route("/api/admin/songs", get(list_songs).post(add_song))
        .route("/api/admin/songs/{track_id}", delete(remove_song))
        .route("/api/admin/search", get(search))
}

// --- GET /api/admin/state -------------------------------------------------------

/// The body of `GET /api/admin/state`, and of the three routes that change
/// what it shows.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StateView {
    /// The real UTC date.
    real_day: Date,
    /// Days the server's day is ahead of it; negative when it is behind.
    offset: i64,
    /// The server's day: `real_day` plus `offset`.
    day: Date,
    /// 1 on the launch day.
    number: i64,
    /// The four sections, in the order the tabs are shown.
    sections: Vec<SectionState>,
}

/// What one section plays today.
#[derive(Debug, Serialize)]
struct SectionState {
    section: Section,
    status: PickStatus,
    /// The day's song. `null` when the section has none, and when the pick
    /// could not be made yet.
    pick: Option<PickView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum PickStatus {
    /// The song is picked and loaded: players can play it.
    Picked,
    /// Nothing in the pool can be played in this section today.
    None,
    /// Deezer did not answer, so the pick could not be made, or the picked
    /// song could not be loaded. It is tried again on later requests.
    Unavailable,
}

/// The answer, for the owner's eyes.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PickView {
    /// Deezer track ID.
    track_id: u64,
    title: String,
    artist: String,
}

/// The clock and today's songs.
///
/// A pick that has not been made yet is made here, exactly as a player's
/// first visit would make it: the page exists to show the day's songs, and
/// after a re-roll or a new day this is where the owner sees what was drawn.
/// A section that fails does not fail the others, or the response.
async fn state_view(app: &AppState) -> Result<StateView, ApiError> {
    let days = app.daily().days().await.map_err(daily_failed)?;
    let day = days.today;

    let mut sections = Vec::with_capacity(Section::ALL.len());
    for section in Section::ALL {
        let state = match app.daily().song(day, section).await {
            Ok(playing) => SectionState {
                section,
                status: PickStatus::Picked,
                pick: Some(PickView {
                    track_id: playing.track_id,
                    title: playing.song.answer.title.clone(),
                    artist: playing.song.answer.artist.clone(),
                }),
            },
            Err(error) => match daily_failed(error) {
                ApiError::NoSong => SectionState {
                    section,
                    status: PickStatus::None,
                    pick: None,
                },
                ApiError::Upstream(_) => SectionState {
                    section,
                    status: PickStatus::Unavailable,
                    pick: standing_pick(app, day, section).await?,
                },
                // The store failed: nothing here can be trusted.
                error => return Err(error),
            },
        };
        sections.push(state);
    }

    Ok(StateView {
        real_day: days.real,
        offset: days.offset,
        day,
        number: game::day_number(app.launch_date(), day),
        sections,
    })
}

/// The pick a section has for `day` when its song could not be loaded, named
/// from the pool. A song that has left the pool since has no title here.
async fn standing_pick(
    app: &AppState,
    day: Date,
    section: Section,
) -> Result<Option<PickView>, ApiError> {
    let picks = app.store().picks_on(day).await.map_err(store_failed)?;
    let Some(pick) = picks.iter().find(|pick| pick.section == section) else {
        return Ok(None);
    };
    let song = app
        .store()
        .song(pick.track_id)
        .await
        .map_err(store_failed)?;
    let (title, artist) = song
        .map(|song| (song.title, song.artist))
        .unwrap_or_default();
    Ok(Some(PickView {
        track_id: pick.track_id,
        title,
        artist,
    }))
}

async fn state_response(app: &AppState) -> Result<Response, ApiError> {
    Ok((no_store(), Json(state_view(app).await?)).into_response())
}

async fn state(State(app): State<AppState>) -> Result<Response, ApiError> {
    state_response(&app).await
}

// --- POST /api/admin/reroll -----------------------------------------------------

/// The sections a re-roll request names: the one in `{ "section": "pop" }`,
/// or all four for a body that is empty, `{}` or `{ "section": null }`.
/// `None` when the body is anything else.
///
/// The content type is not looked at: "re-roll everything" is a request
/// without a body, and a client that sends none has no reason to label it.
fn reroll_sections(body: &[u8]) -> Option<Vec<Section>> {
    if body.trim_ascii().is_empty() {
        return Some(PICK_ORDER.to_vec());
    }
    let body: serde_json::Value = serde_json::from_slice(body).ok()?;
    match body.as_object()?.get("section") {
        None | Some(serde_json::Value::Null) => Some(PICK_ORDER.to_vec()),
        Some(section) => Section::deserialize(section)
            .ok()
            .map(|section| vec![section]),
    }
}

/// Picks again for today, in one section or in all four.
///
/// The old pick and every player's game for that day and section are deleted
/// ([`Daily::reroll`](crate::daily::Daily::reroll)); the state that is sent
/// back then makes the new pick, as a day's first request would, with the
/// replaced song left out of the draw when there is another.
async fn reroll(State(app): State<AppState>, body: Bytes) -> Result<Response, ApiError> {
    let sections = reroll_sections(&body).ok_or(ApiError::BadRequest(
        "Send {\"section\": <\"general\", \"pop\", \"rock\" or \"hip-hop\">} as JSON, or no body to re-roll all four.",
    ))?;
    app.daily().reroll(&sections).await.map_err(daily_failed)?;
    state_response(&app).await
}

// --- POST /api/admin/next-day ---------------------------------------------------

/// Simulate next day: every player moves to the next day together.
async fn next_day(State(app): State<AppState>) -> Result<Response, ApiError> {
    app.daily().next_day().await.map_err(daily_failed)?;
    state_response(&app).await
}

// --- POST /api/admin/reset ------------------------------------------------------

/// Reset to day 1: today becomes the launch date again, and every game and
/// every pick is deleted. The song pool is kept.
async fn reset(State(app): State<AppState>) -> Result<Response, ApiError> {
    app.daily()
        .reset(app.launch_date())
        .await
        .map_err(daily_failed)?;
    state_response(&app).await
}

// --- GET /api/admin/songs -------------------------------------------------------

/// A song of the pool as the admin API shows it: the body of a successful
/// add, and one row of the list.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SongRow {
    /// Deezer track ID.
    track_id: u64,
    /// Deezer's full title, version included.
    title: String,
    artist: String,
    album: String,
    /// Slugs, in the order pop, rock, hip-hop. Empty for a song that is in
    /// the General pool only.
    genres: Genres,
    /// The day the daily pick last found the track without a preview,
    /// `YYYY-MM-DD`, or `null`.
    preview_failed_on: Option<Date>,
}

impl From<PoolSong> for SongRow {
    fn from(song: PoolSong) -> Self {
        Self {
            track_id: song.track_id,
            title: song.title,
            artist: song.artist,
            album: song.album,
            genres: song.genres,
            preview_failed_on: song.preview_failed_on,
        }
    }
}

/// Puts the pool in the order it is shown in: by artist, then title, ignoring
/// case, so that a song can be found by eye. The track ID settles ties, which
/// keeps two releases of one song in the same order on every request.
fn sort_for_display(songs: &mut [PoolSong]) {
    songs.sort_by_cached_key(|song| {
        (
            song.artist.to_lowercase(),
            song.title.to_lowercase(),
            song.track_id,
        )
    });
}

async fn list_songs(State(app): State<AppState>) -> Result<Response, ApiError> {
    let mut songs = app.store().songs().await.map_err(store_failed)?;
    sort_for_display(&mut songs);
    let rows: Vec<SongRow> = songs.into_iter().map(SongRow::from).collect();
    Ok((no_store(), Json(rows)).into_response())
}

// --- POST /api/admin/songs ------------------------------------------------------

/// The body of `POST /api/admin/songs`:
/// `{ "trackId": 123, "genres": ["pop", "hip-hop"] }`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddSongRequest {
    track_id: u64,
    /// Left out, `null` or empty: no genre tags, so the General pool only.
    /// A slug given twice counts once; an unknown one fails the request.
    genres: Option<Vec<Genre>>,
}

async fn add_song(
    State(app): State<AppState>,
    body: Result<Json<AddSongRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = body.map_err(|_| {
        ApiError::BadRequest(
            "Send {\"trackId\": <number>, \"genres\": [<any of \"pop\", \"rock\", \"hip-hop\">]} as JSON.",
        )
    })?;
    let track_id = request.track_id;
    let genres: Genres = request.genres.unwrap_or_default().into_iter().collect();
    let store = app.store();

    // A song already in the pool is only retagged, which needs nothing from
    // Deezer: the page can change genres while Deezer is down or the request
    // budget is spent.
    let retagged = store
        .set_song_genres(track_id, genres.clone())
        .await
        .map_err(store_failed)?;
    if let Some(song) = retagged {
        return Ok((no_store(), Json(SongRow::from(song))).into_response());
    }

    // The title and artist come from Deezer by ID, as a guess's do.
    let track = match app.deezer().track(track_id).await {
        Ok(track) if !track.title.trim().is_empty() => track,
        Ok(_) | Err(DeezerError::NotFound) => return Err(ApiError::UnknownTrack),
        Err(error) => {
            tracing::warn!(%error, track_id, "could not look up a track to add to the pool");
            return Err(ApiError::Upstream(
                "Deezer did not answer, so the song was not added. Try again in a moment.",
            ));
        }
    };

    // No `track.is_playable()` here, on purpose: see the module comment.
    let song = NewSong {
        track_id,
        title: track.title,
        title_short: track.title_short,
        artist: track.artist.name,
        album: track.album.title,
    };
    let stored = store.add_song(song, genres).await.map_err(store_failed)?;
    tracing::info!(track_id, title = %stored.title, artist = %stored.artist, "added a song to the pool");
    Ok((no_store(), Json(SongRow::from(stored))).into_response())
}

// --- DELETE /api/admin/songs/{trackId} ------------------------------------------

async fn remove_song(
    State(app): State<AppState>,
    track_id: Result<Path<u64>, PathRejection>,
) -> Result<Response, ApiError> {
    let Path(track_id) = track_id.map_err(|_| {
        ApiError::BadRequest("The track ID in the address has to be a whole number.")
    })?;
    if !app
        .store()
        .remove_song(track_id)
        .await
        .map_err(store_failed)?
    {
        return Err(ApiError::UnknownSong);
    }
    tracing::info!(track_id, "removed a song from the pool");
    Ok((StatusCode::NO_CONTENT, no_store()).into_response())
}

// --- GET /api/admin/search ------------------------------------------------------

/// One result of the admin search: a player's autocomplete row plus whether
/// the track can be played.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AdminSearchHit {
    id: u64,
    /// Deezer's full title, version included.
    title: String,
    artist: String,
    album: String,
    /// 56 px album cover URL; empty when the album has no artwork.
    cover: String,
    /// Whether Deezer has a preview of the track for this server: the check
    /// the page makes before it adds a song.
    playable: bool,
}

/// Every track Deezer returned, in its order. Unlike the player's search this
/// keeps all the releases of a song, because which release goes into the pool
/// is the point: one may have a preview where another does not.
fn admin_hits(tracks: &[Track]) -> Vec<AdminSearchHit> {
    tracks
        .iter()
        .filter(|track| !track.title.trim().is_empty())
        .map(|track| AdminSearchHit {
            id: track.id,
            title: track.title.clone(),
            artist: track.artist.name.clone(),
            album: track.album.title.clone(),
            cover: track.album.cover_small.clone(),
            playable: track.is_playable(),
        })
        .collect()
}

async fn search(
    State(app): State<AppState>,
    params: Result<Query<SearchParams>, QueryRejection>,
) -> Result<Response, ApiError> {
    let tracks = find_tracks(&app, params).await?;
    Ok((no_store(), Json(admin_hits(&tracks))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        game::MAX_ATTEMPTS,
        testutil::{
            BrokenStore, Harness, LAUNCH, MockTrack, Preview, Reply, TODAY, playing_game, won_game,
        },
    };
    use axum::{
        body::Body,
        http::{Method, Request, header},
    };
    use jiff::civil::date;
    use serde_json::{Value, json};
    use std::{collections::BTreeSet, sync::Arc};

    const QUEEN: u64 = 10;
    const QUEEN_REMASTER: u64 = 11;
    /// A track Deezer knows but will not play: `readable: false`, no preview.
    const WITHDRAWN: u64 = 12;
    const BLANK_TITLE: u64 = 13;
    const UNKNOWN_ID: u64 = 555;

    const POP: Section = Section::Genre(Genre::Pop);
    const ROCK: Section = Section::Genre(Genre::Rock);

    fn tracks() -> Vec<MockTrack> {
        let mut tracks = vec![
            MockTrack::new(QUEEN, "Under Pressure", "Queen").album("Hot Space"),
            MockTrack::new(QUEEN_REMASTER, "Under Pressure (Remastered 2011)", "Queen")
                .title_short("Under Pressure")
                .album("Greatest Hits"),
            MockTrack::new(WITHDRAWN, "Under Pressure (Live)", "Queen")
                .title_short("Under Pressure")
                .album("Live Magic")
                .preview(Preview::None),
            MockTrack::new(BLANK_TITLE, " ", "Queen"),
        ];
        // Two releases each of fifteen songs: more than the player's search
        // shows, more than Deezer is asked for.
        for n in 0..15 {
            let title = format!("Filler Song {n}");
            tracks.push(MockTrack::new(100 + n, &title, "Padding"));
            tracks.push(
                MockTrack::new(200 + n, &format!("{title} (Live)"), "Padding").title_short(&title),
            );
        }
        tracks
    }

    /// The whole router over a Deezer stand-in and an in-memory store with an
    /// empty song pool.
    async fn start() -> Harness {
        Harness::with_pool(tracks(), &[]).await
    }

    /// The admin requests, on the shared harness.
    trait AdminRequests {
        /// `POST /api/admin/songs` with a JSON body.
        async fn add(&self, body: Value) -> Reply;
        async fn add_raw(&self, content_type: &str, body: impl Into<Body>) -> Reply;
        /// The pool as `GET /api/admin/songs` lists it.
        async fn pool(&self) -> Value;
    }

    impl AdminRequests for Harness {
        async fn add(&self, body: Value) -> Reply {
            self.add_raw("application/json", body.to_string()).await
        }

        async fn add_raw(&self, content_type: &str, body: impl Into<Body>) -> Reply {
            let request = Request::builder()
                .method(Method::POST)
                .uri("/api/admin/songs")
                .header(header::CONTENT_TYPE, content_type)
                .body(body.into())
                .unwrap();
            self.send(request).await
        }

        async fn pool(&self) -> Value {
            let reply = self.get("/api/admin/songs").await;
            assert_eq!(reply.status, StatusCode::OK);
            reply.json()
        }
    }

    fn new_song(track_id: u64, title: &str, artist: &str) -> NewSong {
        NewSong {
            track_id,
            title: title.to_owned(),
            title_short: title.to_owned(),
            artist: artist.to_owned(),
            album: format!("{title} (album)"),
        }
    }

    // --- GET /api/admin/songs -----------------------------------------------

    #[tokio::test]
    async fn an_empty_pool_is_an_empty_list() {
        let harness = start().await;
        let reply = harness.get("/api/admin/songs").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json(), json!([]));
        reply.assert_no_store();
    }

    #[tokio::test]
    async fn the_pool_is_listed_by_artist_then_title() {
        let harness = start().await;
        let store = &harness.store;
        for (track_id, title, artist, genres) in [
            (
                4_091_937_401,
                "Bohemian Rhapsody",
                "Queen",
                vec![Genre::Rock],
            ),
            (30, "under pressure", "queen", vec![]),
            (20, "Back In Black", "AC/DC", vec![Genre::Rock, Genre::Pop]),
            (31, "Under Pressure", "Queen", vec![Genre::HipHop]),
            (5, "Africa", "TOTO", vec![Genre::Pop]),
        ] {
            store
                .add_song(
                    new_song(track_id, title, artist),
                    genres.into_iter().collect(),
                )
                .await
                .unwrap();
        }
        store
            .set_preview_failed_on(20, Some(date(2026, 10, 3)))
            .await
            .unwrap();

        let reply = harness.get("/api/admin/songs").await;
        assert_eq!(reply.status, StatusCode::OK);
        reply.assert_no_store();
        assert_eq!(
            reply.json(),
            json!([
                {
                    "trackId": 20,
                    "title": "Back In Black",
                    "artist": "AC/DC",
                    "album": "Back In Black (album)",
                    "genres": ["pop", "rock"],
                    "previewFailedOn": "2026-10-03",
                },
                {
                    "trackId": 4_091_937_401_u64,
                    "title": "Bohemian Rhapsody",
                    "artist": "Queen",
                    "album": "Bohemian Rhapsody (album)",
                    "genres": ["rock"],
                    "previewFailedOn": null,
                },
                // Case is ignored; the track ID settles the tie.
                {
                    "trackId": 30,
                    "title": "under pressure",
                    "artist": "queen",
                    "album": "under pressure (album)",
                    "genres": [],
                    "previewFailedOn": null,
                },
                {
                    "trackId": 31,
                    "title": "Under Pressure",
                    "artist": "Queen",
                    "album": "Under Pressure (album)",
                    "genres": ["hip-hop"],
                    "previewFailedOn": null,
                },
                {
                    "trackId": 5,
                    "title": "Africa",
                    "artist": "TOTO",
                    "album": "Africa (album)",
                    "genres": ["pop"],
                    "previewFailedOn": null,
                },
            ])
        );
    }

    // --- POST /api/admin/songs ----------------------------------------------

    #[tokio::test]
    async fn an_added_song_gets_its_text_from_deezer() {
        let harness = start().await;

        // Whatever else the body claims about the track is ignored.
        let reply = harness
            .add(json!({
                "trackId": QUEEN_REMASTER,
                "genres": ["rock", "pop"],
                "title": "Forged",
                "artist": "Forger",
            }))
            .await;
        assert_eq!(reply.status, StatusCode::OK);
        reply.assert_no_store();
        let row = json!({
            "trackId": QUEEN_REMASTER,
            "title": "Under Pressure (Remastered 2011)",
            "artist": "Queen",
            "album": "Greatest Hits",
            "genres": ["pop", "rock"],
            "previewFailedOn": null,
        });
        assert_eq!(reply.json(), row);
        assert_eq!(harness.pool().await, json!([row]));
        assert_eq!(harness.deezer.api_hits(), 1);

        // The short title, which matching uses, is stored too.
        let stored = harness.store.song(QUEEN_REMASTER).await.unwrap().unwrap();
        assert_eq!(stored.title_short, "Under Pressure");
    }

    #[tokio::test]
    async fn a_track_without_a_preview_is_added_all_the_same() {
        let harness = start().await;

        let reply = harness
            .add(json!({ "trackId": WITHDRAWN, "genres": ["rock"] }))
            .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(
            reply.json(),
            json!({
                "trackId": WITHDRAWN,
                "title": "Under Pressure (Live)",
                "artist": "Queen",
                "album": "Live Magic",
                "genres": ["rock"],
                "previewFailedOn": null,
            })
        );
        assert!(harness.store.song(WITHDRAWN).await.unwrap().is_some());

        // The search is where its state shows.
        let hits = harness
            .get("/api/admin/search?q=under+pressure")
            .await
            .json();
        let withdrawn = hits
            .as_array()
            .unwrap()
            .iter()
            .find(|hit| hit["id"] == WITHDRAWN)
            .unwrap();
        assert_eq!(withdrawn["playable"], false);
    }

    #[tokio::test]
    async fn genres_may_be_left_out_empty_or_repeated() {
        let harness = start().await;

        for (body, expected) in [
            (json!({ "trackId": 100 }), json!([])),
            (json!({ "trackId": 101, "genres": null }), json!([])),
            (json!({ "trackId": 102, "genres": [] }), json!([])),
            (
                json!({ "trackId": 103, "genres": ["hip-hop"] }),
                json!(["hip-hop"]),
            ),
            (
                json!({ "trackId": 104, "genres": ["rock", "hip-hop", "rock", "pop", "pop"] }),
                json!(["pop", "rock", "hip-hop"]),
            ),
        ] {
            let reply = harness.add(body.clone()).await;
            assert_eq!(reply.status, StatusCode::OK, "{body}");
            assert_eq!(reply.json()["genres"], expected, "{body}");
        }
        assert_eq!(harness.pool().await.as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn adding_a_song_already_there_replaces_its_genres_without_asking_deezer() {
        let harness = start().await;
        harness
            .add(json!({ "trackId": QUEEN, "genres": ["rock"] }))
            .await;
        harness
            .store
            .set_preview_failed_on(QUEEN, Some(date(2026, 10, 2)))
            .await
            .unwrap();
        let lookups = harness.deezer.api_hits();

        // Deezer being down does not stop a retag.
        harness.deezer.set_failing(true);
        let reply = harness
            .add(json!({ "trackId": QUEEN, "genres": ["pop", "hip-hop"] }))
            .await;
        assert_eq!(reply.status, StatusCode::OK);
        reply.assert_no_store();
        let row = json!({
            "trackId": QUEEN,
            "title": "Under Pressure",
            "artist": "Queen",
            "album": "Hot Space",
            "genres": ["pop", "hip-hop"],
            "previewFailedOn": "2026-10-02",
        });
        assert_eq!(reply.json(), row);
        assert_eq!(harness.pool().await, json!([row]));

        // Leaving the genres out takes them all away.
        let reply = harness.add(json!({ "trackId": QUEEN })).await;
        assert_eq!(reply.json()["genres"], json!([]));
        assert_eq!(harness.pool().await.as_array().unwrap().len(), 1);
        assert_eq!(harness.deezer.api_hits(), lookups);
    }

    #[tokio::test]
    async fn a_track_deezer_does_not_have_is_not_added() {
        let harness = start().await;

        for track_id in [UNKNOWN_ID, BLANK_TITLE, 0] {
            harness
                .add(json!({ "trackId": track_id, "genres": ["pop"] }))
                .await
                .assert_error(StatusCode::NOT_FOUND, "unknown_track");
        }
        assert_eq!(harness.pool().await, json!([]));
    }

    #[tokio::test]
    async fn a_deezer_failure_adds_nothing_and_is_an_upstream_error() {
        let harness = start().await;
        harness.deezer.set_failing(true);
        harness
            .add(json!({ "trackId": QUEEN, "genres": ["rock"] }))
            .await
            .assert_error(StatusCode::BAD_GATEWAY, "upstream");
        assert_eq!(harness.pool().await, json!([]));

        // The same request works once Deezer is back.
        harness.deezer.set_failing(false);
        let reply = harness
            .add(json!({ "trackId": QUEEN, "genres": ["rock"] }))
            .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(harness.pool().await.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn malformed_adds_are_bad_requests_that_reach_neither_deezer_nor_the_pool() {
        let harness = start().await;

        for body in [
            json!({}),
            json!({ "genres": ["pop"] }),
            json!({ "trackId": "10" }),
            json!({ "trackId": -1 }),
            json!({ "trackId": 1.5 }),
            json!({ "trackId": null }),
            json!({ "track_id": QUEEN }),
            json!({ "trackId": QUEEN, "genres": "pop" }),
            json!({ "trackId": QUEEN, "genres": ["jazz"] }),
            json!({ "trackId": QUEEN, "genres": ["pop", "general"] }),
            json!({ "trackId": QUEEN, "genres": ["Pop"] }),
            json!({ "trackId": QUEEN, "genres": ["hiphop"] }),
            json!({ "trackId": QUEEN, "genres": [""] }),
            json!({ "trackId": QUEEN, "genres": [1] }),
            json!({ "trackId": QUEEN, "genres": [null] }),
            json!([QUEEN]),
            json!(null),
        ] {
            harness
                .add(body)
                .await
                .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        }
        harness
            .add_raw("application/json", "{not json")
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        harness
            .add_raw("application/json", "")
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        harness
            .add_raw("text/plain", r#"{"trackId":10}"#)
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");

        assert_eq!(harness.deezer.api_hits(), 0);
        assert_eq!(harness.pool().await, json!([]));
    }

    #[tokio::test]
    async fn a_bad_genre_does_not_retag_a_song_that_is_there() {
        let harness = start().await;
        harness
            .add(json!({ "trackId": QUEEN, "genres": ["rock"] }))
            .await;

        harness
            .add(json!({ "trackId": QUEEN, "genres": ["pop", "jazz"] }))
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        assert_eq!(harness.pool().await[0]["genres"], json!(["rock"]));
    }

    // --- DELETE /api/admin/songs/{trackId} ----------------------------------

    #[tokio::test]
    async fn a_removed_song_leaves_the_pool() {
        let harness = start().await;
        harness
            .add(json!({ "trackId": QUEEN, "genres": ["rock"] }))
            .await;
        harness.add(json!({ "trackId": QUEEN_REMASTER })).await;

        let reply = harness.delete(&format!("/api/admin/songs/{QUEEN}")).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        assert!(reply.body.is_empty());
        reply.assert_no_store();

        let pool = harness.pool().await;
        assert_eq!(pool.as_array().unwrap().len(), 1);
        assert_eq!(pool[0]["trackId"], QUEEN_REMASTER);

        // Added again, it comes back without the tags it had.
        let again = harness.add(json!({ "trackId": QUEEN })).await.json();
        assert_eq!(again["genres"], json!([]));
    }

    #[tokio::test]
    async fn removing_a_song_that_is_not_in_the_pool_is_not_found() {
        let harness = start().await;
        harness.add(json!({ "trackId": QUEEN })).await;

        // A track Deezer knows but the pool does not, and one nobody knows.
        for track_id in [QUEEN_REMASTER, UNKNOWN_ID, 0, u64::MAX] {
            harness
                .delete(&format!("/api/admin/songs/{track_id}"))
                .await
                .assert_error(StatusCode::NOT_FOUND, "unknown_song");
        }
        // The second removal of the same song as well.
        let uri = format!("/api/admin/songs/{QUEEN}");
        assert_eq!(harness.delete(&uri).await.status, StatusCode::NO_CONTENT);
        harness
            .delete(&uri)
            .await
            .assert_error(StatusCode::NOT_FOUND, "unknown_song");
        assert_eq!(harness.pool().await, json!([]));
    }

    #[tokio::test]
    async fn a_track_id_that_is_not_a_number_is_a_bad_request() {
        let harness = start().await;
        harness.add(json!({ "trackId": QUEEN })).await;

        for uri in [
            "/api/admin/songs/queen",
            "/api/admin/songs/-10",
            "/api/admin/songs/10.5",
            "/api/admin/songs/99999999999999999999999",
        ] {
            harness
                .delete(uri)
                .await
                .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        }
        assert_eq!(harness.pool().await.as_array().unwrap().len(), 1);
    }

    // --- GET /api/admin/search ----------------------------------------------

    #[tokio::test]
    async fn the_admin_search_lists_every_release_and_whether_it_plays() {
        let harness = start().await;

        let reply = harness
            .get("/api/admin/search?q=%20Under%20%20PRESSURE%20")
            .await;
        assert_eq!(reply.status, StatusCode::OK);
        reply.assert_no_store();
        let cover = |id: u64| {
            format!("https://cdn-images.dzcdn.net/images/cover/cover{id}/56x56-000000-80-0-0.jpg")
        };
        // Three releases of one song, in Deezer's order. The player's search
        // would show the first only.
        assert_eq!(
            reply.json(),
            json!([
                {
                    "id": QUEEN,
                    "title": "Under Pressure",
                    "artist": "Queen",
                    "album": "Hot Space",
                    "cover": cover(QUEEN),
                    "playable": true,
                },
                {
                    "id": QUEEN_REMASTER,
                    "title": "Under Pressure (Remastered 2011)",
                    "artist": "Queen",
                    "album": "Greatest Hits",
                    "cover": cover(QUEEN_REMASTER),
                    "playable": true,
                },
                {
                    "id": WITHDRAWN,
                    "title": "Under Pressure (Live)",
                    "artist": "Queen",
                    "album": "Live Magic",
                    "cover": cover(WITHDRAWN),
                    "playable": false,
                },
            ])
        );
        let player = harness.get("/api/search?q=under+pressure").await.json();
        assert_eq!(player.as_array().unwrap().len(), 1);
        // Both searches were answered by one Deezer request.
        assert_eq!(harness.deezer.api_hits(), 1);
    }

    #[tokio::test]
    async fn the_admin_search_is_not_cut_to_the_autocomplete_length() {
        let harness = start().await;

        // 30 tracks match; Deezer is asked for 25 and all of them are shown,
        // live versions included.
        let rows = harness.get("/api/admin/search?q=filler").await.json();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 25);
        assert!(rows.iter().all(|row| row["playable"] == true));
        let live = rows
            .iter()
            .filter(|row| row["title"].as_str().unwrap().ends_with("(Live)"))
            .count();
        assert_eq!(live, 12);

        // A track without a title is not something to add.
        let rows = harness.get("/api/admin/search?q=queen").await.json();
        let ids: Vec<u64> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, vec![QUEEN, QUEEN_REMASTER, WITHDRAWN]);
    }

    #[tokio::test]
    async fn short_admin_queries_do_not_reach_deezer() {
        let harness = start().await;
        for uri in [
            "/api/admin/search",
            "/api/admin/search?q=",
            "/api/admin/search?q=a",
            "/api/admin/search?q=%20a%20%20",
            "/api/admin/search?other=1",
        ] {
            let reply = harness.get(uri).await;
            assert_eq!(reply.status, StatusCode::OK, "{uri}");
            assert_eq!(reply.json(), json!([]), "{uri}");
            reply.assert_no_store();
        }
        assert_eq!(harness.deezer.api_hits(), 0);
    }

    #[tokio::test]
    async fn a_failed_admin_search_is_an_upstream_error() {
        let harness = start().await;
        harness.deezer.set_failing(true);
        harness
            .get("/api/admin/search?q=queen")
            .await
            .assert_error(StatusCode::BAD_GATEWAY, "upstream");
    }

    // --- the rest -----------------------------------------------------------

    #[tokio::test]
    async fn a_store_failure_is_an_internal_error_with_a_fixed_message() {
        let harness = Harness::over(tracks(), &[], Arc::new(BrokenStore)).await;

        for reply in [
            harness.get("/api/admin/songs").await,
            harness.add(json!({ "trackId": QUEEN })).await,
            harness.delete("/api/admin/songs/10").await,
            harness.get("/api/admin/state").await,
            harness.post("/api/admin/reroll", None).await,
            harness
                .post("/api/admin/reroll", Some(json!({ "section": "pop" })))
                .await,
            harness.post("/api/admin/next-day", None).await,
            harness.post("/api/admin/reset", None).await,
        ] {
            reply.assert_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
            reply.assert_lacks(&["fire", "needledrop.db"]);
        }
        // The retag is tried before Deezer is asked, so nothing was looked up.
        assert_eq!(harness.deezer.api_hits(), 0);

        // What does not need the store carries on: the search.
        assert_eq!(harness.get("/api/health").await.status, StatusCode::OK);
        assert_eq!(
            harness.get("/api/admin/search?q=queen").await.status,
            StatusCode::OK
        );
        // The day and its songs are in the store, so the game does fail.
        harness
            .get("/api/daily/general")
            .await
            .assert_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
    }

    #[tokio::test]
    async fn unknown_admin_routes_are_json_errors() {
        let harness = start().await;
        for uri in [
            "/api/admin",
            "/api/admin/nope",
            "/api/admin/songs/10/genres",
            "/api/admin/state/pop",
            "/api/admin/reroll/pop",
        ] {
            harness
                .get(uri)
                .await
                .assert_error(StatusCode::NOT_FOUND, "not_found");
        }
    }

    #[tokio::test]
    async fn a_method_a_route_does_not_have_changes_nothing() {
        let harness = start().await;
        harness.add(json!({ "trackId": QUEEN })).await;

        // A song can be removed but not fetched on its own, and the list
        // cannot be deleted. These are axum's own 405s, with no body.
        let song = format!("/api/admin/songs/{QUEEN}");
        for reply in [
            harness.get(&song).await,
            harness.delete("/api/admin/songs").await,
            harness.delete("/api/admin/search?q=queen").await,
            harness.get("/api/admin/reroll").await,
            harness.get("/api/admin/next-day").await,
            harness.get("/api/admin/reset").await,
            harness.post("/api/admin/state", None).await,
        ] {
            assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
            assert!(reply.body.is_empty());
        }
        assert_eq!(harness.pool().await.as_array().unwrap().len(), 1);
    }

    // --- GET /api/admin/state -----------------------------------------------

    /// A pool like the seed: two songs per genre and nothing untagged.
    const SIX: [(u64, &[Genre]); 6] = [
        (100, &[Genre::Pop]),
        (101, &[Genre::Pop]),
        (102, &[Genre::Rock]),
        (103, &[Genre::Rock]),
        (104, &[Genre::HipHop]),
        (105, &[Genre::HipHop]),
    ];

    /// Three songs per genre: whatever General takes, a genre has two left,
    /// so a re-roll of a genre always has another song to draw.
    const NINE: [(u64, &[Genre]); 9] = [
        (100, &[Genre::Pop]),
        (101, &[Genre::Pop]),
        (106, &[Genre::Pop]),
        (102, &[Genre::Rock]),
        (103, &[Genre::Rock]),
        (107, &[Genre::Rock]),
        (104, &[Genre::HipHop]),
        (105, &[Genre::HipHop]),
        (108, &[Genre::HipHop]),
    ];

    const SLUGS: [&str; 4] = ["general", "pop", "rock", "hip-hop"];

    /// `GET /api/admin/state`, which has to answer.
    async fn state(harness: &Harness) -> Value {
        let reply = harness.get("/api/admin/state").await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        reply.assert_no_store();
        reply.json()
    }

    /// The line of one section in a state.
    fn section<'a>(state: &'a Value, slug: &str) -> &'a Value {
        state["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|section| section["section"] == slug)
            .unwrap_or_else(|| panic!("no {slug} in {state}"))
    }

    /// The track a section plays, which it has to have.
    fn picked(state: &Value, slug: &str) -> u64 {
        let section = section(state, slug);
        assert_eq!(section["status"], "picked", "{section}");
        section["pick"]["trackId"].as_u64().unwrap()
    }

    /// `POST /api/admin/reroll` for one section, which has to work.
    async fn reroll(harness: &Harness, slug: &str) -> Value {
        let reply = harness
            .post("/api/admin/reroll", Some(json!({ "section": slug })))
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        reply.assert_no_store();
        reply.json()
    }

    /// `POST /api/admin/next-day`, which has to work.
    async fn next_day(harness: &Harness) -> Value {
        let reply = harness.post("/api/admin/next-day", None).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.visible());
        reply.assert_no_store();
        reply.json()
    }

    #[tokio::test]
    async fn the_state_shows_the_clock_and_todays_answers() {
        let harness = Harness::with_pool(tracks(), &SIX).await;

        let first = state(&harness).await;
        assert_eq!(first["realDay"], "2026-10-03");
        assert_eq!(first["offset"], 0);
        assert_eq!(first["day"], "2026-10-03");
        assert_eq!(first["number"], 3);
        let sections = first["sections"].as_array().unwrap();
        let slugs: Vec<&str> = sections
            .iter()
            .map(|section| section["section"].as_str().unwrap())
            .collect();
        assert_eq!(slugs, SLUGS);

        // Nobody had asked for today yet: the state made the picks, one per
        // section, each from its own pool and no song twice.
        let pop = picked(&first, "pop");
        assert!([100, 101].contains(&pop));
        assert!([102, 103].contains(&picked(&first, "rock")));
        assert!([104, 105].contains(&picked(&first, "hip-hop")));
        let distinct: BTreeSet<u64> = SLUGS.iter().map(|slug| picked(&first, slug)).collect();
        assert_eq!(distinct.len(), 4);
        // The answer is there in words: this page is the owner's.
        assert_eq!(
            section(&first, "pop")["pick"],
            json!({
                "trackId": pop,
                "title": format!("Filler Song {}", pop - 100),
                "artist": "Padding",
            })
        );

        // They are the day's songs: in the store, and what the players get.
        assert_eq!(harness.pick(TODAY, POP).await, Some(pop));
        let mut player = harness.player();
        assert_eq!(player.guess_in(POP, pop).await.json()["status"], "won");

        // Asking again changes nothing and costs Deezer nothing.
        let hits = harness.deezer.api_hits();
        assert_eq!(state(&harness).await, first);
        assert_eq!(harness.deezer.api_hits(), hits);
    }

    #[tokio::test]
    async fn the_state_says_which_sections_have_no_song() {
        // Pop has a song, Rock's only song has no preview, nothing is tagged
        // hip-hop, and General is left with one.
        let harness = Harness::with_pool(
            tracks(),
            &[
                (100, &[Genre::Pop]),
                (WITHDRAWN, &[Genre::Rock]),
                (QUEEN, &[]),
            ],
        )
        .await;

        assert_eq!(
            state(&harness).await,
            json!({
                "realDay": "2026-10-03",
                "offset": 0,
                "day": "2026-10-03",
                "number": 3,
                "sections": [
                    {
                        "section": "general",
                        "status": "picked",
                        "pick": { "trackId": QUEEN, "title": "Under Pressure", "artist": "Queen" },
                    },
                    {
                        "section": "pop",
                        "status": "picked",
                        "pick": { "trackId": 100, "title": "Filler Song 0", "artist": "Padding" },
                    },
                    { "section": "rock", "status": "none", "pick": null },
                    { "section": "hip-hop", "status": "none", "pick": null },
                ],
            })
        );
        // The song that failed the check is marked in the pool.
        let pool = harness.pool().await;
        let withdrawn = pool
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["trackId"] == WITHDRAWN)
            .unwrap();
        assert_eq!(withdrawn["previewFailedOn"], "2026-10-03");
    }

    #[tokio::test]
    async fn a_section_deezer_cannot_serve_is_unavailable_and_the_state_still_answers() {
        let mut harness = Harness::with_pool(tracks(), &SIX).await;
        harness.deezer.set_failing(true);

        // No pick can be made: every section waits, none is given up on.
        let down = state(&harness).await;
        assert_eq!(down["day"], "2026-10-03");
        assert_eq!(down["number"], 3);
        for slug in SLUGS {
            assert_eq!(
                section(&down, slug),
                &json!({ "section": slug, "status": "unavailable", "pick": null })
            );
        }
        assert_eq!(harness.store.picks_on(TODAY).await.unwrap(), Vec::new());
        for row in harness.pool().await.as_array().unwrap() {
            assert_eq!(row["previewFailedOn"], Value::Null);
        }

        harness.deezer.set_failing(false);
        let made = state(&harness).await;
        for slug in SLUGS {
            picked(&made, slug);
        }

        // A restart that finds the preview cache gone and Deezer down: the
        // picks stand and are named, from the pool, but cannot be played.
        std::fs::remove_dir_all(harness.data_dir().join("audio")).unwrap();
        harness.restart();
        harness.deezer.set_failing(true);
        let down = state(&harness).await;
        for slug in SLUGS {
            let line = section(&down, slug);
            assert_eq!(line["status"], "unavailable");
            assert_eq!(line["pick"], section(&made, slug)["pick"]);
        }
        harness
            .get("/api/daily/pop")
            .await
            .assert_error(StatusCode::BAD_GATEWAY, "upstream");

        harness.deezer.set_failing(false);
        assert_eq!(state(&harness).await, made);
    }

    // --- POST /api/admin/next-day ---------------------------------------------

    #[tokio::test]
    async fn the_next_day_moves_every_player_on_and_draws_new_songs() {
        let harness = Harness::with_pool(tracks(), &SIX).await;
        let before = state(&harness).await;
        let mut winner = harness.player();
        let mut loser = harness.player();
        let body = winner.guess(picked(&before, "general")).await.json();
        assert_eq!(body["status"], "won");
        for _ in 0..MAX_ATTEMPTS {
            loser.skip().await;
        }
        let winner_id = harness.player_id(&winner).unwrap();

        let after = next_day(&harness).await;
        assert_eq!(after["realDay"], "2026-10-03");
        assert_eq!(after["offset"], 1);
        assert_eq!(after["day"], "2026-10-04");
        assert_eq!(after["number"], 4);
        assert_eq!(harness.store.day_offset().await.unwrap(), 1);
        // New songs, as at a real midnight: each genre plays its other one,
        // and again no song twice.
        for slug in ["pop", "rock", "hip-hop"] {
            assert_ne!(picked(&after, slug), picked(&before, slug), "{slug}");
        }
        let distinct: BTreeSet<u64> = SLUGS.iter().map(|slug| picked(&after, slug)).collect();
        assert_eq!(distinct.len(), 4);
        // Yesterday's picks are history, not gone.
        assert_eq!(harness.store.picks_on(TODAY).await.unwrap().len(), 4);

        // Both players are on the new day, with a fresh game and the record
        // of the day before.
        let body = winner.daily(Section::General).await.json();
        assert_eq!(body["day"], "2026-10-04");
        assert_eq!(body["number"], 4);
        assert_eq!(body["status"], "playing");
        assert_eq!(body["attempts"], json!([]));
        assert_eq!(body["answer"], Value::Null);
        assert_eq!(body["stats"]["played"], 1);
        assert_eq!(body["stats"]["currentStreak"], 1);
        let body = loser.daily(Section::General).await.json();
        assert_eq!(body["status"], "playing");
        assert_eq!(body["stats"]["played"], 1);
        assert_eq!(body["stats"]["won"], 0);
        assert_eq!(body["stats"]["currentStreak"], 0);
        let today = winner.get("/api/today").await.json();
        assert_eq!(today["day"], "2026-10-04");
        assert_eq!(today["sections"][0]["status"], "playing");

        // The streak carries: a win today makes it two days.
        let body = winner.guess(picked(&after, "general")).await.json();
        assert_eq!(body["status"], "won");
        assert_eq!(body["stats"]["currentStreak"], 2);
        assert_eq!(body["stats"]["bestStreak"], 2);
        assert_eq!(body["stats"]["played"], 2);
        let games = harness
            .store
            .games(&winner_id, Section::General)
            .await
            .unwrap();
        assert_eq!(games.len(), 2);

        // Again, and then a real midnight on top of the two simulated ones.
        let again = next_day(&harness).await;
        assert_eq!(again["offset"], 2);
        assert_eq!(again["day"], "2026-10-05");
        assert_eq!(again["number"], 5);
        harness.clock.set(date(2026, 10, 4));
        let later = state(&harness).await;
        assert_eq!(later["realDay"], "2026-10-04");
        assert_eq!(later["offset"], 2);
        assert_eq!(later["day"], "2026-10-06");
        assert_eq!(later["number"], 6);
    }

    // --- POST /api/admin/reset --------------------------------------------------

    #[tokio::test]
    async fn a_reset_is_day_one_again_without_games_or_picks_but_with_the_pool() {
        let harness = Harness::with_pool(tracks(), &SIX).await;
        let first = state(&harness).await;
        let mut player = harness.player();
        player.skip_in(ROCK).await;
        player.guess(picked(&first, "general")).await;
        next_day(&harness).await;
        next_day(&harness).await;
        player.skip().await;
        let id = harness.player_id(&player).unwrap();
        let pool = harness.pool().await;
        assert_eq!(pool.as_array().unwrap().len(), 6);
        let history = harness.store.pick_history(POP).await.unwrap();
        assert_eq!(history.len(), 3);

        let reply = harness.post("/api/admin/reset", None).await;
        assert_eq!(reply.status, StatusCode::OK);
        reply.assert_no_store();
        let after = reply.json();
        // Today is the launch date again: the offset is what makes it so.
        assert_eq!(after["realDay"], "2026-10-03");
        assert_eq!(after["offset"], -2);
        assert_eq!(after["day"], "2026-10-01");
        assert_eq!(after["number"], 1);
        assert_eq!(harness.store.day_offset().await.unwrap(), -2);

        // Every game of every day is gone, and with them the record.
        for section in Section::ALL {
            let games = harness.store.games(&id, section).await.unwrap();
            assert_eq!(games, Vec::new(), "{section}");
        }
        let body = player.daily(Section::General).await.json();
        assert_eq!(body["day"], "2026-10-01");
        assert_eq!(body["number"], 1);
        assert_eq!(body["attempts"], json!([]));
        assert_eq!(body["stats"]["played"], 0);
        assert_eq!(body["stats"]["bestStreak"], 0);

        // So is every pick: all the history there is are the four the state
        // has just made for day 1.
        for section in Section::ALL {
            let history = harness.store.pick_history(section).await.unwrap();
            assert_eq!(history.len(), 1, "{section}");
            assert_eq!(history[0].day, LAUNCH);
        }
        for slug in SLUGS {
            picked(&after, slug);
        }
        // The song pool is as it was.
        assert_eq!(harness.pool().await, pool);

        // A reset on day 1 is day 1; after a real midnight it takes one more
        // day of offset to get there.
        let reply = harness.post("/api/admin/reset", None).await;
        assert_eq!(reply.json()["offset"], -2);
        harness.clock.set(date(2026, 10, 4));
        assert_eq!(state(&harness).await["day"], "2026-10-02");
        let reply = harness.post("/api/admin/reset", None).await;
        assert_eq!(reply.json()["offset"], -3);
        assert_eq!(reply.json()["day"], "2026-10-01");
        assert_eq!(reply.json()["number"], 1);
    }

    // --- POST /api/admin/reroll -------------------------------------------------

    #[tokio::test]
    async fn a_reroll_gives_one_section_another_song_and_deletes_its_games_for_the_day() {
        let harness = Harness::with_pool(tracks(), &NINE).await;
        let before = state(&harness).await;
        let old = picked(&before, "pop");
        let mut one = harness.player();
        let mut other = harness.player();
        one.skip_in(POP).await;
        one.skip_in(ROCK).await;
        assert_eq!(other.guess_in(POP, old).await.json()["status"], "won");
        other.skip().await;
        let one_id = harness.player_id(&one).unwrap();
        let other_id = harness.player_id(&other).unwrap();
        // A Pop game of the day before, which is none of the re-roll's business.
        let yesterday = TODAY.yesterday().unwrap();
        let store = &harness.store;
        store
            .save_game(&one_id, POP, &won_game(yesterday, 1))
            .await
            .unwrap();

        let after = reroll(&harness, "pop").await;
        // Another song for Pop; the clock and the other three as they were.
        let new = picked(&after, "pop");
        assert_ne!(new, old);
        assert!([100, 101, 106].contains(&new));
        for slug in ["general", "rock", "hip-hop"] {
            assert_eq!(section(&after, slug), section(&before, slug), "{slug}");
        }
        assert_ne!(new, picked(&after, "general"));
        assert_eq!(after["day"], before["day"]);
        assert_eq!(after["offset"], 0);

        // Every player's Pop game of today is gone, won or not. Nothing else.
        assert_eq!(
            store.games(&one_id, POP).await.unwrap(),
            vec![won_game(yesterday, 1)]
        );
        assert_eq!(store.games(&other_id, POP).await.unwrap(), Vec::new());
        assert_eq!(
            store.games(&one_id, ROCK).await.unwrap(),
            vec![playing_game(TODAY, 1)]
        );
        assert_eq!(
            store.games(&other_id, Section::General).await.unwrap(),
            vec![playing_game(TODAY, 1)]
        );

        // The players have a fresh Pop game of the new song. The record
        // corrects itself: the win that was deleted is not in it.
        let body = one.daily(POP).await.json();
        assert_eq!(body["attempts"], json!([]));
        assert_eq!(body["stats"]["played"], 1);
        assert_eq!(body["stats"]["currentStreak"], 1);
        let body = other.daily(POP).await.json();
        assert_eq!(body["status"], "playing");
        assert_eq!(body["answer"], Value::Null);
        assert_eq!(body["stats"]["played"], 0);
        // The old answer is a miss now, the new one wins.
        assert_eq!(other.guess_in(POP, old).await.json()["status"], "playing");
        assert_eq!(other.guess_in(POP, new).await.json()["status"], "won");
        // Rock goes on where it was.
        let body = one.daily(ROCK).await.json();
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
    }

    #[tokio::test]
    async fn a_reroll_without_a_section_draws_all_four_again() {
        for body in [None, Some(json!({})), Some(json!({ "section": null }))] {
            let harness = Harness::with_pool(tracks(), &SIX).await;
            let before = state(&harness).await;
            let mut player = harness.player();
            for section in Section::ALL {
                player.skip_in(section).await;
            }
            let id = harness.player_id(&player).unwrap();

            let reply = harness.post("/api/admin/reroll", body.clone()).await;
            assert_eq!(reply.status, StatusCode::OK, "{body:?}");
            reply.assert_no_store();
            let after = reply.json();

            // A new day's worth of picks. Each genre has two songs and plays
            // its other one; General draws from the three the genres left,
            // which are the ones they played before, so its song is new too.
            for slug in SLUGS {
                assert_ne!(picked(&after, slug), picked(&before, slug), "{slug}");
            }
            let distinct: BTreeSet<u64> = SLUGS.iter().map(|slug| picked(&after, slug)).collect();
            assert_eq!(distinct.len(), 4);
            assert_eq!(after["day"], "2026-10-03");

            for section in Section::ALL {
                let games = harness.store.games(&id, section).await.unwrap();
                assert_eq!(games, Vec::new(), "{section}");
            }
        }
    }

    #[tokio::test]
    async fn a_reroll_never_gives_a_section_a_song_another_section_plays_today() {
        // Pop's two songs are half of what General can play.
        let harness = Harness::with_pool(
            tracks(),
            &[
                (100, &[Genre::Pop]),
                (101, &[Genre::Pop]),
                (110, &[]),
                (111, &[]),
            ],
        )
        .await;
        let mut now = state(&harness).await;
        assert_ne!(picked(&now, "general"), picked(&now, "pop"));

        for round in 0..12 {
            let slug = if round % 2 == 0 { "general" } else { "pop" };
            let old = picked(&now, slug);
            now = reroll(&harness, slug).await;
            assert_ne!(
                picked(&now, "general"),
                picked(&now, "pop"),
                "round {round}"
            );
            assert!([100, 101].contains(&picked(&now, "pop")));
            // General always has another song to go to. Pop has one only
            // while General is not playing its other song.
            if slug == "general" {
                assert_ne!(picked(&now, slug), old, "round {round}");
            }
            assert_eq!(section(&now, "rock")["status"], "none");
        }
    }

    #[tokio::test]
    async fn a_reroll_of_a_section_with_one_song_draws_it_again_and_still_deletes_the_games() {
        let harness = Harness::with_pool(tracks(), &[(100, &[Genre::Pop])]).await;
        let mut player = harness.player();
        player.skip_in(POP).await;
        let id = harness.player_id(&player).unwrap();

        let after = reroll(&harness, "pop").await;
        assert_eq!(picked(&after, "pop"), 100);
        assert_eq!(harness.store.games(&id, POP).await.unwrap(), Vec::new());
        // A section without a song has nothing to re-roll, and says so again.
        let after = reroll(&harness, "rock").await;
        assert_eq!(section(&after, "rock")["status"], "none");
        assert_eq!(picked(&after, "pop"), 100);
    }

    #[tokio::test]
    async fn a_malformed_reroll_is_a_bad_request_and_rerolls_nothing() {
        let harness = Harness::with_pool(tracks(), &SIX).await;
        let before = state(&harness).await;
        let mut player = harness.player();
        player.skip_in(POP).await;
        let id = harness.player_id(&player).unwrap();

        for body in [
            json!({ "section": "jazz" }),
            json!({ "section": "Pop" }),
            json!({ "section": "" }),
            json!({ "section": 7 }),
            json!({ "section": ["pop"] }),
            json!([]),
            json!(["pop"]),
            json!("pop"),
            json!(7),
            json!(null),
        ] {
            harness
                .post("/api/admin/reroll", Some(body))
                .await
                .assert_error(StatusCode::BAD_REQUEST, "bad_request");
        }
        let request = Request::post("/api/admin/reroll")
            .body(Body::from("{not json"))
            .unwrap();
        harness
            .send(request)
            .await
            .assert_error(StatusCode::BAD_REQUEST, "bad_request");

        assert_eq!(state(&harness).await, before);
        assert_eq!(harness.store.games(&id, POP).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_reroll_while_deezer_is_down_is_finished_when_deezer_is_back() {
        let harness = Harness::with_pool(tracks(), &NINE).await;
        let before = state(&harness).await;
        let old = picked(&before, "rock");
        let mut player = harness.player();
        player.skip_in(ROCK).await;
        let id = harness.player_id(&player).unwrap();

        // The song Rock would go to has never been downloaded.
        harness.deezer.set_failing(true);
        let reply = harness
            .post("/api/admin/reroll", Some(json!({ "section": "rock" })))
            .await;
        assert_eq!(reply.status, StatusCode::OK);
        let during = reply.json();
        assert_eq!(
            section(&during, "rock"),
            &json!({ "section": "rock", "status": "unavailable", "pick": null })
        );
        for slug in ["general", "pop", "hip-hop"] {
            assert_eq!(section(&during, slug), section(&before, slug), "{slug}");
        }
        // The old song is gone all the same, and the games played against it.
        assert_eq!(harness.pick(TODAY, ROCK).await, None);
        assert_eq!(harness.store.games(&id, ROCK).await.unwrap(), Vec::new());
        // Rock waits for Deezer; the other sections play on.
        player
            .daily(ROCK)
            .await
            .assert_error(StatusCode::BAD_GATEWAY, "upstream");
        assert_eq!(player.daily(POP).await.status, StatusCode::OK);
        let today = player.get("/api/today").await.json();
        assert_eq!(today["sections"][2]["song"], "pending");

        // Deezer is back: the pick is made by whoever asks first, and it is
        // not the song that was re-rolled away.
        harness.deezer.set_failing(false);
        assert_eq!(player.daily(ROCK).await.status, StatusCode::OK);
        let after = state(&harness).await;
        assert_ne!(picked(&after, "rock"), old);
        assert!([102, 103, 107].contains(&picked(&after, "rock")));
    }

    #[tokio::test]
    async fn adding_and_removing_songs_leaves_todays_picks_alone() {
        let harness = Harness::with_pool(tracks(), &[(QUEEN, &[])]).await;
        let before = state(&harness).await;
        assert_eq!(picked(&before, "general"), QUEEN);
        assert_eq!(section(&before, "pop")["status"], "none");
        let game = harness.get("/api/daily/general").await.json();

        harness
            .add(json!({ "trackId": 100, "genres": ["pop"] }))
            .await;
        harness.add(json!({ "trackId": 101 })).await;
        let reply = harness.delete(&format!("/api/admin/songs/{QUEEN}")).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);

        // General still plays the song that has left the pool, and it is
        // still named: the day's pick is not the pool's business.
        let after = state(&harness).await;
        assert_eq!(section(&after, "general"), section(&before, "general"));
        assert_eq!(harness.get("/api/daily/general").await.json(), game);
        // A section that had no song gets one from a song added in the day.
        assert_eq!(picked(&after, "pop"), 100);

        // From the next day on the removed song is not drawn.
        let tomorrow = next_day(&harness).await;
        assert_eq!(picked(&tomorrow, "pop"), 100);
        assert_eq!(picked(&tomorrow, "general"), 101);
    }

    #[test]
    fn a_reroll_names_one_section_or_none() {
        let all = Some(PICK_ORDER.to_vec());
        for body in ["", "  \n", "{}", r#"{"section":null}"#, r#"{"other":1}"#] {
            assert_eq!(reroll_sections(body.as_bytes()), all, "{body:?}");
        }
        for section in Section::ALL {
            let body = json!({ "section": section }).to_string();
            assert_eq!(reroll_sections(body.as_bytes()), Some(vec![section]));
        }
        for body in [
            r#"{"section":"jazz"}"#,
            r#"{"section":["pop"]}"#,
            r#"["pop"]"#,
            r#""pop""#,
            "null",
            "{",
        ] {
            assert_eq!(reroll_sections(body.as_bytes()), None, "{body:?}");
        }
    }

    // --- pure pieces --------------------------------------------------------

    #[test]
    fn display_order_ignores_case_and_is_settled_by_the_track_id() {
        let mut songs: Vec<PoolSong> = [
            (3, "b", "Zed"),
            (9, "Same", "abba"),
            (2, "same", "ABBA"),
            (7, "Alpha", "abba"),
            (1, "A", "zed"),
        ]
        .into_iter()
        .map(|(id, title, artist)| PoolSong::new(new_song(id, title, artist), Genres::new()))
        .collect();

        sort_for_display(&mut songs);
        let ids: Vec<u64> = songs.iter().map(|song| song.track_id).collect();
        assert_eq!(ids, vec![7, 2, 9, 1, 3]);
    }

    #[test]
    fn a_song_row_has_camel_case_keys_and_no_short_title() {
        let mut song = PoolSong::new(
            NewSong {
                track_id: 7,
                title: "Song (Live)".to_owned(),
                title_short: "Song".to_owned(),
                artist: "Someone".to_owned(),
                album: "LP".to_owned(),
            },
            Genres::from([Genre::HipHop, Genre::Pop]),
        );
        song.preview_failed_on = Some(date(2026, 12, 31));

        assert_eq!(
            serde_json::to_value(SongRow::from(song)).unwrap(),
            json!({
                "trackId": 7,
                "title": "Song (Live)",
                "artist": "Someone",
                "album": "LP",
                "genres": ["pop", "hip-hop"],
                "previewFailedOn": "2026-12-31",
            })
        );
    }
}
