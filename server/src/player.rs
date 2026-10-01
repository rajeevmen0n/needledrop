//! Who is playing: the anonymous player cookie, the key it is encrypted with, the lock that keeps one player's moves in order, and Clear my data.
//!
//! There are no accounts. A browser is remembered by a cookie that holds a
//! random [`PlayerId`] and nothing else; the games played under that ID are
//! in the store ([`crate::store`]), and the stats are worked out from them
//! ([`crate::stats`]). Clearing the cookies, or a private window, is a new
//! player. That is the accepted limit of playing without an account.

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

use anyhow::Context;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::{Cookie, Key, PrivateCookieJar, SameSite};
use tokio::sync::{Mutex, MutexGuard};

use crate::{
    routes::{ApiError, AppState, no_store, store_failed},
    store::PlayerId,
};

/// The player cookie: a [`PlayerId`], encrypted.
///
/// The cookie of the first version of the game, `gts_daily`, held the whole
/// game state. It is not read any more, and it expires by itself two days
/// after it was last set.
pub(crate) const COOKIE_NAME: &str = "gts_player";

/// How long a browser keeps the cookie after a visit. Every visit sets it
/// again, so it only runs out for a player who stays away this long. 400 days
/// is the most a browser accepts; a longer life would be cut down to it.
const COOKIE_MAX_AGE: time::Duration = time::Duration::days(400);

/// The cookie key on disk, under the data directory.
const KEY_FILE: &str = "secret.key";

/// Length of the cookie key: 32 bytes to sign with and 32 to encrypt with.
const KEY_BYTES: usize = 64;

/// Locks in [`MoveLocks`]. Two players share one only by chance, and then
/// wait for each other for the length of two store operations.
const MOVE_LOCK_STRIPES: usize = 64;

// --- the player cookie ------------------------------------------------------------

/// The player the request's cookie names, or `None` for a browser the server
/// has not given an ID yet.
///
/// A cookie that is missing, does not decrypt (the jar then reports it as
/// absent) or holds something that is not an ID is no player, and the caller
/// issues a new one. Because the cookie is encrypted and authenticated with
/// the server's key, an ID that comes out of it is one this server put in: a
/// client cannot name another player, or invent one.
pub fn known_player(jar: &PrivateCookieJar) -> Option<PlayerId> {
    jar.get(COOKIE_NAME)
        .and_then(|cookie| PlayerId::parse(cookie.value()))
}

/// Puts `player` into the response's cookie, for another [`COOKIE_MAX_AGE`].
///
/// `HttpOnly`, so no script can read it, and `SameSite=Lax`, so another site
/// cannot make moves or clear the data in the player's name. `headers` are
/// the request's: the cookie is `Secure` when the browser came over HTTPS
/// (see [`is_https`]).
pub fn remember(jar: PrivateCookieJar, player: &PlayerId, headers: &HeaderMap) -> PrivateCookieJar {
    let cookie = Cookie::build((COOKIE_NAME, player.as_str().to_owned()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(COOKIE_MAX_AGE)
        .secure(is_https(headers))
        .build();
    jar.add(cookie)
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

// --- one move at a time -----------------------------------------------------------

/// Makes a player's moves happen one after another.
///
/// A move reads the stored game, applies the guess or the skip and stores the
/// result. Two requests doing that at the same moment would both read the
/// same game and both store "one more attempt": two guesses for the price of
/// one try, and with enough requests at once the whole ladder for free. So
/// the read, the move and the write are a critical section, entered through
/// [`lock`](Self::lock). The slow part of a guess, asking Deezer what the
/// guessed track is, is done before it.
///
/// The alternative was a conditional write in the store ("replace this game
/// if it still has `n` attempts"). The lock was chosen because it keeps
/// [`Store`](crate::store::Store) at "replace this game", which every backend
/// can do, and keeps the handler free of a retry loop. What it costs: it
/// holds within one server process only. That is all there is today (one
/// process owns the SQLite file); several processes over a shared database
/// would need the conditional write after all.
///
/// The locks are striped: a fixed number of them, and a player always gets
/// the same one. That needs no bookkeeping and no cleaning up, at the price
/// that two players now and then wait for each other. Players cannot arrange
/// that, since they do not choose their IDs.
pub struct MoveLocks {
    stripes: [Mutex<()>; MOVE_LOCK_STRIPES],
}

impl MoveLocks {
    pub fn new() -> Self {
        Self {
            stripes: std::array::from_fn(|_| Mutex::new(())),
        }
    }

    /// Waits for the player's turn. The turn lasts until the guard is
    /// dropped; keep it across the read, the move and the write, and across
    /// nothing slower.
    pub async fn lock(&self, player: &PlayerId) -> MutexGuard<'_, ()> {
        self.stripes[Self::stripe(player)].lock().await
    }

    /// Which lock is the player's: the same one every time.
    fn stripe(player: &PlayerId) -> usize {
        // `DefaultHasher::new()` has fixed keys, unlike a `HashMap`'s.
        let mut hasher = DefaultHasher::new();
        player.hash(&mut hasher);
        // The remainder is below the stripe count, so it fits a `usize`.
        (hasher.finish() % MOVE_LOCK_STRIPES as u64) as usize
    }
}

// --- DELETE /api/player -----------------------------------------------------------

/// Clear my data: deletes every game stored for the player, today's attempts
/// included, and with them the stats, which are derived from the games. The
/// response's cookie carries a new ID, so nothing connects what the browser
/// plays next to what it played before. The day's song is not touched.
///
/// Without a cookie there is nothing to delete; the browser still gets an ID.
/// The old ID is not revoked (there is no list of players to strike it from):
/// a copy of the old cookie is a player without games, like any new one.
pub async fn clear(
    State(app): State<AppState>,
    jar: PrivateCookieJar,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(player) = known_player(&jar) {
        // In turn with the player's moves: one that is being stored right now
        // is deleted with the rest instead of landing after the delete.
        let _turn = app.move_locks().lock(&player).await;
        let games = app
            .store()
            .delete_player(&player)
            .await
            .map_err(store_failed)?;
        tracing::info!(%player, games, "cleared a player's data");
    }
    let jar = remember(jar, &PlayerId::generate(), &headers);
    Ok((StatusCode::NO_CONTENT, jar, no_store()).into_response())
}

// --- the cookie key ---------------------------------------------------------------

/// The cookie key could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "expected 128 hexadecimal characters (64 random bytes, for example from `openssl rand -hex 64`)"
)]
pub struct KeyFormatError;

/// The key the player cookie is encrypted with.
///
/// `secret` is `GTS_SECRET`: 128 hexadecimal characters, that is 64 random
/// bytes (`openssl rand -hex 64`). Without it the key lives in
/// `<data_dir>/secret.key`, in the same format, and is generated on the first
/// start. The file is created readable by its owner only.
///
/// Changing the key makes every existing cookie undecryptable, which the game
/// treats as "no cookie": each browser becomes a new player. The games of the
/// old ones stay in the database, out of anyone's reach.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        game::{GameState, MAX_ATTEMPTS},
        store::{Genre, Section},
        testutil::{BrokenStore, Harness, MockTrack, TODAY, lost_game, won_game},
    };
    use axum::{
        body::Body,
        http::{HeaderValue, Request, header},
    };
    use serde_json::json;
    use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

    /// The song being played. The ID is long so that it cannot turn up by
    /// chance in the cookie's ciphertext.
    const ANSWER_ID: u64 = 987_654_321;

    /// Strings that identify the answer. None may appear while playing.
    const SECRETS: [&str; 4] = ["987654321", "Zanzibar", "The Answers", "Night Album"];

    async fn start() -> Harness {
        let tracks =
            vec![MockTrack::new(ANSWER_ID, "Zanzibar Nights", "The Answers").album("Night Album")];
        Harness::start(tracks, ANSWER_ID).await
    }

    // --- the player cookie --------------------------------------------------

    #[tokio::test]
    async fn a_first_visit_is_given_a_player_cookie() {
        let harness = start().await;
        let mut player = harness.player();
        let reply = player.get("/api/daily/general").await;
        assert_eq!(reply.status, StatusCode::OK);

        let cookies: Vec<_> = reply.headers.get_all(header::SET_COOKIE).iter().collect();
        assert_eq!(cookies.len(), 1, "{cookies:?}");
        let cookie = reply.header(header::SET_COOKIE);
        assert!(cookie.starts_with("gts_player="), "{cookie}");
        // 400 days, the longest a browser keeps a cookie.
        for attribute in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=34560000"] {
            assert!(cookie.contains(attribute), "{attribute} missing: {cookie}");
        }
        // Plain HTTP (the dev server): a `Secure` cookie would be dropped.
        assert!(!cookie.contains("Secure"), "{cookie}");

        // Encrypted: the ID is in it, and cannot be read off it.
        let id = harness.player_id(&player).unwrap();
        assert!(!cookie.contains(id.as_str()), "{cookie}");
        reply.assert_lacks(&SECRETS);
    }

    #[tokio::test]
    async fn the_cookie_is_secure_behind_https() {
        let harness = start().await;
        let mut player = harness.player();
        for uri in ["/api/daily/general", "/api/player"] {
            let request = Request::builder()
                .method(if uri == "/api/player" {
                    "DELETE"
                } else {
                    "GET"
                })
                .uri(uri)
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
    }

    #[tokio::test]
    async fn the_cookie_holds_the_player_id_and_nothing_else() {
        let harness = start().await;
        let mut player = harness.player();
        player.get("/api/daily/general").await;
        let id = harness.player_id(&player).unwrap();

        // Whatever happens in the game, what is inside stays those 32
        // characters: no state, and nothing about the song.
        for _ in 0..MAX_ATTEMPTS {
            let reply = player.skip().await;
            assert_eq!(reply.status, StatusCode::OK);
            let inside = harness.cookie_plaintext(&player).unwrap();
            assert_eq!(inside, id.as_str());
            assert_eq!(inside.len(), 32);
            // The header is the same size for an empty game and a lost one.
            assert!(reply.header(header::SET_COOKIE).len() < 200);
        }
    }

    #[tokio::test]
    async fn every_visit_and_every_move_refreshes_the_cookie_for_the_same_player() {
        let harness = start().await;
        let mut player = harness.player();
        player.get("/api/daily/general").await;
        let id = harness.player_id(&player).unwrap();
        let first = player.cookie.clone();

        for reply in [player.get("/api/daily/general").await, player.skip().await] {
            let cookie = reply.header(header::SET_COOKIE);
            assert!(cookie.contains("Max-Age=34560000"), "{cookie}");
            assert_eq!(harness.player_id(&player), Some(id.clone()));
        }
        // Encrypted afresh each time, so the text differs while the ID stays.
        assert_ne!(player.cookie, first);
    }

    #[tokio::test]
    async fn listening_and_searching_set_no_cookie() {
        let harness = start().await;
        let mut player = harness.player();
        for uri in [
            "/api/daily/general/audio",
            "/api/search?q=zanzibar",
            "/api/health",
        ] {
            let reply = player.get(uri).await;
            assert_eq!(reply.status, StatusCode::OK, "{uri}");
            assert!(reply.headers.get(header::SET_COOKIE).is_none(), "{uri}");
        }
        assert_eq!(player.cookie, None);
    }

    #[tokio::test]
    async fn a_cookie_this_server_did_not_make_is_a_new_player() {
        let harness = start().await;
        let mut player = harness.player();
        player.skip().await;
        let real = player.cookie.clone().unwrap();
        let id = harness.player_id(&player).unwrap();

        for garbage in [
            // Not a cookie this server made.
            "gts_player=not-even-base64!!".to_owned(),
            "gts_player=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            // A bare ID: the client names a player without the key.
            format!("gts_player={id}"),
            // A real cookie with its end cut off.
            real[..real.len() - 6].to_owned(),
            // Properly encrypted, but what is inside is not an ID.
            harness.cookie_holding("gts_player", "not-an-id"),
            harness.cookie_holding("gts_player", &id.as_str().to_uppercase()),
            harness.cookie_holding("gts_player", ""),
        ] {
            player.cookie = Some(garbage.clone());
            let reply = player.get("/api/daily/general").await;
            assert_eq!(reply.status, StatusCode::OK, "{garbage}");
            let body = reply.json();
            assert_eq!(body["attempts"], json!([]), "{garbage}");
            assert_eq!(body["status"], "playing");
            // And it is replaced by a good one, for somebody else.
            assert_ne!(player.cookie.as_deref(), Some(garbage.as_str()));
            let issued = harness.player_id(&player).unwrap();
            assert_ne!(issued, id, "{garbage}");
        }

        // The untouched cookie still finds the game with the skip.
        player.cookie = Some(real);
        let body = player.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
    }

    #[tokio::test]
    async fn another_servers_key_does_not_decrypt_the_cookie() {
        let first = start().await;
        let second = start().await;
        let mut player = first.player();
        player.skip().await;
        let id = first.player_id(&player).unwrap();
        // Even if the other server had a game under that very ID.
        second
            .store
            .save_game(&id, Section::General, &lost_game(TODAY))
            .await
            .unwrap();

        // The same cookie, sent to a server with a different key.
        player.app = second.app.clone();
        let body = player.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"], json!([]));
        assert_eq!(body["status"], "playing");
        assert!(second.player_id(&player).is_some_and(|new| new != id));
    }

    #[tokio::test]
    async fn the_old_game_cookie_is_ignored() {
        let harness = start().await;
        let mut player = harness.player();
        // What the first version of the game kept in the browser: a whole
        // game, here one that is already won.
        let old = serde_json::to_string(&won_game(TODAY, 0)).unwrap();
        player.cookie = Some(harness.cookie_holding("gts_daily", &old));

        let reply = player.get("/api/daily/general").await;
        let body = reply.json();
        assert_eq!(body["status"], "playing");
        assert_eq!(body["attempts"], json!([]));
        assert_eq!(body["answer"], json!(null));
        reply.assert_lacks(&SECRETS);
        // The only cookie set is the player's; the old one is left to expire.
        let cookies: Vec<_> = reply.headers.get_all(header::SET_COOKIE).iter().collect();
        assert_eq!(cookies.len(), 1, "{cookies:?}");
        assert!(reply.header(header::SET_COOKIE).starts_with("gts_player="));
    }

    // --- DELETE /api/player -------------------------------------------------

    #[tokio::test]
    async fn clearing_deletes_the_players_games_and_issues_a_new_id() {
        let harness = start().await;
        let today = TODAY;
        let mut player = harness.player();
        player.skip().await;
        player.skip().await;
        let old_id = harness.player_id(&player).unwrap();
        let old_cookie = player.cookie.clone();
        // An earlier day, and another section.
        let store = &harness.store;
        for (section, game) in [
            (Section::General, won_game(today.yesterday().unwrap(), 2)),
            (Section::Genre(Genre::Rock), lost_game(today)),
        ] {
            store.save_game(&old_id, section, &game).await.unwrap();
        }
        let before = player.get("/api/daily/general").await.json();
        assert_eq!(before["attempts"].as_array().unwrap().len(), 2);
        assert_eq!(before["stats"]["played"], 1);

        let reply = player.clear().await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        assert!(reply.body.is_empty());
        reply.assert_no_store();
        reply.assert_lacks(&SECRETS);
        let cookie = reply.header(header::SET_COOKIE);
        for attribute in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=34560000"] {
            assert!(cookie.contains(attribute), "{attribute} missing: {cookie}");
        }

        // Everything stored under the old ID is gone, in every section.
        for section in Section::ALL {
            assert_eq!(store.games(&old_id, section).await.unwrap(), Vec::new());
        }
        // The browser is somebody new, with a fresh game and an empty record.
        let new_id = harness.player_id(&player).unwrap();
        assert_ne!(new_id, old_id);
        let after = player.get("/api/daily/general").await.json();
        assert_eq!(after["attempts"], json!([]));
        assert_eq!(after["status"], "playing");
        assert_eq!(after["clipSeconds"], 0.1);
        assert_eq!(after["stats"]["played"], 0);
        assert_eq!(after["stats"]["currentStreak"], 0);
        assert_eq!(after["stats"]["bestStreak"], 0);

        // A copy of the old cookie is not a way back: it is a player with no
        // games, who starts the day again.
        let mut stale = harness.player();
        stale.cookie = old_cookie;
        let body = stale.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"], json!([]));
        assert_eq!(body["stats"]["played"], 0);
        assert_eq!(harness.player_id(&stale), Some(old_id));
    }

    #[tokio::test]
    async fn clearing_does_not_change_the_days_song() {
        let harness = start().await;
        let mut player = harness.player();
        for _ in 0..MAX_ATTEMPTS {
            player.skip().await;
        }
        let before = player.get("/api/daily/general").await.json();
        assert_eq!(before["answer"]["title"], "Zanzibar Nights");

        player.clear().await;
        // While playing again, nothing about the song comes back.
        let fresh = player.get("/api/daily/general").await;
        assert_eq!(fresh.json()["answer"], json!(null));
        fresh.assert_lacks(&SECRETS);
        for _ in 0..MAX_ATTEMPTS {
            player.skip().await;
        }
        let after = player.get("/api/daily/general").await.json();
        assert_eq!(after["answer"], before["answer"]);
        assert_eq!(after["day"], before["day"]);
        assert_eq!(after["number"], before["number"]);
    }

    #[tokio::test]
    async fn clearing_leaves_other_players_alone() {
        let harness = start().await;
        let mut leaving = harness.player();
        let mut staying = harness.player();
        leaving.skip().await;
        staying.skip().await;
        staying.skip().await;
        let staying_id = harness.player_id(&staying).unwrap();

        assert_eq!(leaving.clear().await.status, StatusCode::NO_CONTENT);

        let body = staying.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"].as_array().unwrap().len(), 2);
        assert_eq!(harness.player_id(&staying), Some(staying_id));
    }

    #[tokio::test]
    async fn clearing_without_a_cookie_deletes_nothing_and_still_issues_an_id() {
        let harness = start().await;
        let mut other = harness.player();
        other.skip().await;

        let mut player = harness.player();
        for cookie in [None, Some("gts_player=garbage".to_owned())] {
            player.cookie = cookie;
            let reply = player.clear().await;
            assert_eq!(reply.status, StatusCode::NO_CONTENT);
            assert!(harness.player_id(&player).is_some());
        }
        // Twice in a row is fine too, and each time it is a new ID.
        let first = harness.player_id(&player).unwrap();
        assert_eq!(player.clear().await.status, StatusCode::NO_CONTENT);
        assert_ne!(harness.player_id(&player), Some(first));

        let body = other.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
    }

    #[tokio::test]
    async fn a_store_failure_while_clearing_is_an_internal_error_and_keeps_the_id() {
        let harness = Harness::with_store(Vec::new(), ANSWER_ID, Arc::new(BrokenStore)).await;
        let id = PlayerId::generate();
        let mut player = harness.player_with_id(&id);

        let reply = player.clear().await;
        reply.assert_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        reply.assert_lacks(&["fire", "needledrop.db"]);
        // Nothing was deleted, so the browser is not given a new identity
        // that would hide the data it asked to have removed.
        assert!(reply.headers.get(header::SET_COOKIE).is_none());
        assert_eq!(harness.player_id(&player), Some(id));

        // With no cookie there is nothing to delete and nothing to fail.
        let reply = harness.player().clear().await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn the_player_route_only_deletes() {
        let harness = start().await;
        let mut player = harness.player();
        player.skip().await;

        // axum's own 405, with no body, and nothing is cleared by it.
        let reply = player.get("/api/player").await;
        assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
        assert!(reply.body.is_empty());
        let body = player.get("/api/daily/general").await.json();
        assert_eq!(body["attempts"], json!([{ "kind": "skip" }]));
    }

    // --- the move lock ------------------------------------------------------

    #[tokio::test]
    async fn a_players_lock_is_held_by_one_at_a_time() {
        let locks = MoveLocks::new();
        let player = PlayerId::generate();

        let turn = locks.lock(&player).await;
        // A second move of the same player has to wait...
        let waiting = tokio::time::timeout(Duration::from_millis(20), locks.lock(&player)).await;
        assert!(waiting.is_err());
        // ...until the first is done.
        drop(turn);
        let next = tokio::time::timeout(Duration::from_secs(5), locks.lock(&player)).await;
        assert!(next.is_ok());
    }

    #[test]
    fn a_player_always_gets_the_same_lock_and_players_are_spread_over_all_of_them() {
        let mut used = [false; MOVE_LOCK_STRIPES];
        for _ in 0..5000 {
            let player = PlayerId::generate();
            let stripe = MoveLocks::stripe(&player);
            assert!(stripe < MOVE_LOCK_STRIPES);
            assert_eq!(MoveLocks::stripe(&player), stripe);
            let same = PlayerId::parse(player.as_str()).unwrap();
            assert_eq!(MoveLocks::stripe(&same), stripe);
            used[stripe] = true;
        }
        assert!(used.iter().all(|&used| used), "{used:?}");
    }

    // --- pure pieces --------------------------------------------------------

    #[test]
    fn a_remembered_player_is_known_again() {
        let key = Key::generate();
        let player = PlayerId::generate();
        let jar = remember(
            PrivateCookieJar::new(key.clone()),
            &player,
            &HeaderMap::new(),
        );
        // In the response's own jar.
        assert_eq!(known_player(&jar), Some(player.clone()));

        // And in the jar of the request that brings the cookie back.
        let response = jar.into_response();
        let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        let mut headers = HeaderMap::new();
        let pair = set_cookie.split(';').next().unwrap();
        headers.insert(header::COOKIE, pair.parse().unwrap());
        let back = PrivateCookieJar::from_headers(&headers, key);
        assert_eq!(known_player(&back), Some(player));

        // No cookie at all is nobody.
        let empty = PrivateCookieJar::new(Key::generate());
        assert_eq!(known_player(&empty), None);
    }

    #[test]
    fn a_game_is_not_a_player_id() {
        // What the old cookie held, put into the new cookie's name: still
        // nobody, because it is not an ID.
        let key = Key::generate();
        let state = serde_json::to_string(&GameState::new(TODAY)).unwrap();
        let jar = PrivateCookieJar::new(key).add(Cookie::new(COOKIE_NAME, state));
        assert_eq!(known_player(&jar), None);
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
