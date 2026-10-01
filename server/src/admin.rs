//! The admin API under `/api/admin`: the song pool, and the search that finds songs for it.
//!
//! **Nothing here is protected.** That is the owner's choice for now, and it
//! has to change before the server is reachable by anyone else: these routes
//! edit the pool, and the ones still to come show the day's answers and wipe
//! every player's data. The anti-leak rule of [`crate::routes`] is about the
//! player's routes and does not apply to these.
//!
//! Adding a song never checks that it can be played. Two callers share the
//! route, the admin page and a bulk-add script, and each validates before it
//! calls; a song that slips through without a preview is skipped by the daily
//! pick. The search reports `playable` per result so the page can do its part.

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get},
};
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::{
    deezer::{DeezerError, Track},
    routes::{ApiError, AppState, SearchParams, find_tracks, no_store, store_failed},
    store::{Genre, Genres, NewSong, PoolSong},
};

/// The admin routes, to be merged into the server's router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/songs", get(list_songs).post(add_song))
        .route("/api/admin/songs/{track_id}", delete(remove_song))
        .route("/api/admin/search", get(search))
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
        store::PlayerId,
        testutil::{BrokenStore, Harness, MockTrack, Preview, Reply},
    };
    use axum::{
        body::Body,
        http::{Method, Request, header},
    };
    use jiff::civil::date;
    use serde_json::{Value, json};
    use std::sync::Arc;

    const QUEEN: u64 = 10;
    const QUEEN_REMASTER: u64 = 11;
    /// A track Deezer knows but will not play: `readable: false`, no preview.
    const WITHDRAWN: u64 = 12;
    const BLANK_TITLE: u64 = 13;
    const UNKNOWN_ID: u64 = 555;
    /// The track the game plays; the admin routes have nothing to do with it.
    const CONFIGURED_TRACK: u64 = 1;

    fn tracks() -> Vec<MockTrack> {
        let mut tracks = vec![
            MockTrack::new(CONFIGURED_TRACK, "Zanzibar Nights", "The Answers"),
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

    /// The whole router over a Deezer stand-in and an empty in-memory store.
    async fn start() -> Harness {
        Harness::start(tracks(), CONFIGURED_TRACK).await
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
        let harness = Harness::with_store(tracks(), CONFIGURED_TRACK, Arc::new(BrokenStore)).await;

        for reply in [
            harness.get("/api/admin/songs").await,
            harness.add(json!({ "trackId": QUEEN })).await,
            harness.delete("/api/admin/songs/10").await,
        ] {
            reply.assert_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
            reply.assert_lacks(&["fire", "needledrop.db"]);
        }
        // The retag is tried before Deezer is asked, so nothing was looked up.
        assert_eq!(harness.deezer.api_hits(), 0);

        // What does not need the store carries on: the search, and the game
        // of a browser without a cookie, which has no games to read.
        assert_eq!(harness.get("/api/health").await.status, StatusCode::OK);
        assert_eq!(harness.get("/api/daily").await.status, StatusCode::OK);
        assert_eq!(
            harness.get("/api/admin/search?q=queen").await.status,
            StatusCode::OK
        );
        // A known player's game is in the store, so that does fail.
        harness
            .player_with_id(&PlayerId::generate())
            .get("/api/daily")
            .await
            .assert_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
    }

    #[tokio::test]
    async fn the_pool_does_not_change_what_the_game_plays() {
        let harness = start().await;
        let before = harness.get("/api/daily").await.json();

        harness
            .add(json!({ "trackId": QUEEN, "genres": ["rock"] }))
            .await;
        harness
            .add(json!({ "trackId": CONFIGURED_TRACK, "genres": ["pop"] }))
            .await;
        harness
            .delete(&format!("/api/admin/songs/{CONFIGURED_TRACK}"))
            .await;

        assert_eq!(harness.get("/api/daily").await.json(), before);
    }

    #[tokio::test]
    async fn unknown_admin_routes_are_json_errors() {
        let harness = start().await;
        for uri in [
            "/api/admin",
            "/api/admin/nope",
            "/api/admin/songs/10/genres",
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
        ] {
            assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
            assert!(reply.body.is_empty());
        }
        assert_eq!(harness.pool().await.as_array().unwrap().len(), 1);
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
