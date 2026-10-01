# Needledrop — briefing for agents

Read this before touching the repo. Update the **Progress** section as the last step of any task.

## What this is

Needledrop is a daily song-guessing game in the style of Heardle: you hear a sliver of a song, guess it, and each wrong guess or skip unlocks a longer clip. The music is Deezer's free 30-second previews. "Needledrop" is a working title and easy to rename; the repo and crate are called `guessthesong`.

Decisions already made, and why:

- **The server is authoritative.** The browser never sees the answer's Deezer ID, title or preview URL until the game is over. A client-only game would leak the answer through the network tab.
- **The server sends only the audio unlocked so far.** If the full preview reached the browser, anyone could play all 30 seconds. The server cuts the MP3 to the current clip length on every request.
- **No accounts.** Everyone plays without logging in and each browser is remembered by an encrypted, HttpOnly cookie that holds a random anonymous player ID and nothing else. The games are in the server's database under that ID, and the stats are worked out from them. Accepted limit: clearing cookies or using a private window gives a fresh player.
- **Daily mode: four sections, one song each per day, the same for everyone.** General, Pop, Rock and Hip-hop are four separate seven-attempt games a day, each with its own song of the day drawn from the curated song database (`pick.rs` decides, `daily.rs` checks, stores and loads). The day is the server's day: the real UTC date plus a stored offset that the admin can move (Simulate next day, Reset to day 1). The agreed requirements are in **Roadmap** below; read it before building tasks 13–14.
- **Ladder: 0.1 / 0.3 / 1 / 3 / 8 / 16 / 30 seconds — seven attempts.** The first clips are deliberately tiny; that is the game.
- **A guess is correct when the normalized title and primary artist both match.** Remasters, live cuts and album variants of the same song have different Deezer IDs, so comparing IDs would reject right answers.
- **Guesses must be picked from autocomplete.** The client sends a track ID, and the server looks up the title and artist itself, so a client cannot forge them.
- **Rust server (axum), Svelte 5 + Vite client, Nix dev shell.** Nothing needs to be installed globally: the flake provides `cargo`, `rustc`, `pnpm`, `just` and the rest.
- **TLS via rustls, no OpenSSL**, so the dev shell needs no system libraries.

## Layout and architecture

```
browser ─▶ Vite dev server 127.0.0.1:4811 (Svelte SPA) ── /api ─▶ axum server 127.0.0.1:4810 ─▶ api.deezer.com / preview CDN
                                                                      │
                                                                      └─ data/  (cached mp3 + track JSON, cookie key, needledrop.db)
```

Both processes speak plain HTTP on loopback. Vite proxies `/api` to 4810, so `http://127.0.0.1:4811` is the whole app in development.

To serve it under a public hostname, put a TLS-terminating reverse proxy in front: `/` → 4811, `/api/` → 4810, forwarding `X-Forwarded-Proto` (see the cookie notes below). `web/vite.config.ts` pins `server.allowedHosts` and the HMR websocket (`server.hmr`) to one public hostname; set them to your own hostname, or remove them when working on plain localhost, because with them in place hot reload only connects through that hostname. When the game ships as a production build, the server will serve `web/dist` and the proxy will send everything to 4810.

```
flake.nix, flake.lock   dev shell: cargo, rustc, clippy, rustfmt, rust-analyzer, bacon, nodejs_24, pnpm, just
justfile                recipes (below)
bacon.toml              the `server` job `just dev` uses: restarts the server when server/src, Cargo.toml or config.toml change
config.toml             bind address, data dir, launch date, the store backend
server/                 Rust crate `guessthesong-server`
web/                    Vite + Svelte 5 + TypeScript SPA (plain Vite, not SvelteKit), pnpm
data/                   runtime state, git-ignored, created by the server: audio/, secret.key, needledrop.db
```

### Server modules (`server/src/`)

| File | Responsibility |
|---|---|
| `main.rs` | Tracing, config load, cookie key (`player::session_key`), the store (`store::open`, then `store::seed_if_empty`, both before the listener is bound), `Daily` over the store with the wall clock (`Clock::utc()`), shared state, router. Picks and loads today's four songs in the background (`AppState::warm_up`) so the first request does not wait for Deezer. No static file serving yet. |
| `config.rs` | `Config::load()`: `config.toml` (path from `GTS_CONFIG`) with `GTS_BIND` and `GTS_SECRET` on top. Fields `bind`, `data_dir`, `launch_date`, `store`, `secret`. `store` is a `StoreKind` (`Sqlite` or `Memory`) read from `[store] kind`: SQLite when the table or the key is left out, and any other value is an error naming `store.kind`. `Config::from_sources(text, env_lookup)` is the pure, tested part. A bad config stops the server at startup with a message naming the key or variable. Keys the server does not know are ignored, so an old file that still has `track_id` and `playlists` loads. |
| `deezer.rs` | `Deezer`, a typed client over one `reqwest::Client` (10 s timeout, user agent): `search_tracks(q, limit)`, `track(id)`, `track_meta(id)`, `download_preview(url)`. Reads Deezer's 200-with-error bodies (`DeezerError::NotFound` for code 800, `Api` for the rest). Search results are cached for 10 minutes (500 queries), and the title and artist of every track seen are kept (10,000 tracks, 24 h) so a guess picked from autocomplete needs no request. A request budget (40 per 5 s) refuses to call Deezer near its rate limit: `DeezerError::Throttled`. Nothing retries. No `playlist_tracks` yet. |
| `mp3.rs` | Pure. `Mp3::parse(bytes)` once per song: skip ID3v2, walk frame headers, keep only the audio frames (no tags, no Xing/Info frame, no partial last frame). `Mp3::prefix(ms)` → the leading frames covering the duration plus 4 padding frames (bit reservoir and encoder delay), as a zero-copy `Bytes`. |
| `pick.rs` | Pure: no I/O, no clock, randomness passed in. `PICK_ORDER` (Pop, Rock, Hip-hop, General), `candidates(section, day, pool, taken)` (the section's pool without what the other sections play that day and without songs whose preview failed that day; empty = no song today) and `choose(section, day, pool, history, taken, replaced, rng)`, which also would rather not draw the song a re-roll replaces, then yesterday's, then anything the section has played since its pool was last used up. The rules and which of them give way are listed at the top of the file. |
| `daily.rs` | The day and its songs. `Clock` (the real date; `Clock::utc()` or a test's own) and `Daily::days()` / `today()`: the real UTC date plus the offset in the store. `Daily::song(day, section)` → `Playing { day, section, track_id, song: Arc<Song> }` (`Song` has `meta` for matching, `answer` for the reveal, the parsed `mp3`): the stored pick with its song from memory when both are there; otherwise, under one lock, the sections ahead in `PICK_ORDER` get their turn, then this one is drawn (`pick::choose`), **loaded, and only then stored** (`Store::save_pick`). Loading is the preview check: memory, else `data/audio/<track_id>.mp3` + `.json`, else `/track/{id}` and the preview download, parsed before it is cached. A track Deezer has no preview for (not readable, no preview URL, a body that is not an MP3, no such track) is marked `preview_failed_on = day` and another is drawn; any other failure stores nothing, returns the error and keeps further Deezer requests away for 10 s. Up to 8 parsed songs stay in memory. `reroll(sections)`, `next_day()` and `reset(launch)` change the day or its picks under the write side of a lock; `stands(&playing)` gives a request the read side after checking the stored pick is still the track it holds. `warm(day)` settles all four. Its error messages name tracks, so they go to the log, never to a client. |
| `game.rs` | Pure logic: the ladder, `GameState` (one player's game on one day; what the store keeps) and its transitions, `day_number`, and `normalize_title` / `normalize_artist` (lowercase, strip diacritics, use `title_short`, drop bracketed / "feat." / "- remaster" suffixes, strip non-alphanumerics). Match = normalized title **and** primary artist equal (`is_match`, `TrackMeta::match_key`). It does not care how the track was chosen, who is playing or in which section, so later modes can reuse it. |
| `routes.rs` | `AppState` (the Deezer client, `Daily`, the `Arc<dyn Store>`, the move locks, the launch date and the cookie key; `deezer()`, `store()`, `move_locks()`, `daily()`, `launch_date()`, `today()`, `song(section, day)`, `warm_up()`), `router(state)` (the game's routes, the three legacy aliases, `player::clear` and `admin::routes()`), the game's handlers, `ApiError`, `store_failed` (logs a `StoreError`, answers 500 `internal`) and `daily_failed` (`DailyError` → `no_song`, `internal` or `upstream`). `today` is the overview behind the tabs; it reads and never picks. `show_game`, `play_clip` and `make_move` take the section; the per-section routes and the legacy routes (`LEGACY_SECTION`, General) both call them. `standing_song` fetches a section's song and holds `Daily::stands` while the games are read, so a response is about one song. `History` is a player's games in a section, today's apart from the rest; `DailyView::new` is the only place the answer is handed out. A move is read, applied and saved inside the player's move lock and under the standing guard. `find_tracks` is the query trimming and the Deezer search that the player's and the admin's search share. A guess's metadata is resolved server-side from `trackId` (`Deezer::track_meta`: the cache filled by searches, else `/track/{id}`), before the locks are taken. |
| `player.rs` | Who is playing. The player cookie (`known_player(jar)` → `Option<PlayerId>`, `remember(jar, player, headers)`), `MoveLocks` (64 striped async locks; `lock(&player)` is held across the read, the move and the write of a move, and by Clear my data), the `DELETE /api/player` handler `clear`, and the cookie key (`session_key`). |
| `stats.rs` | Pure, no clock. `Stats::as_of(today, games)` works a player's record in one section out of their stored games: `played`, `won`, `win_percent`, `current_streak`, `best_streak`, `guess_distribution`. Nothing is kept as a counter. The definitions are under **API**. |
| `admin.rs` | The admin API (see **Admin API** below): `routes()` and the handlers for the state (clock and today's answers), re-roll, next day and reset, the pool list, add / retag, remove and the admin search. The three that change the day or its songs call `Daily` and then answer with the state. It reaches the data only through `AppState::store()` and `AppState::daily()`. A `StoreError` is logged and answered with 500 `internal` (`routes::store_failed`). |
| `store/mod.rs` | The `Store` trait (async, usable as `dyn Store` through the `async-trait` crate), the only way the server touches persistent data, and its domain types: `Genre` (`pop` / `rock` / `hip-hop`; `slug()`, `from_slug()`, `ALL`), `Genres` (a `BTreeSet<Genre>`), `Section` (`General` or `Genre(Genre)`; slugs `general` / `pop` / `rock` / `hip-hop`), `PlayerId` (128 random bits as 32 lowercase hexadecimal characters; `generate()`, `parse()`), `NewSong`, `PoolSong`, `Pick` (`day`, `section`, `track_id`), and the opaque `StoreError` (a sentence for the log, nothing to match on). `decode_game` reads a game that a backend keeps as JSON and is where "a stored game that cannot be read counts as not there" is decided. `open(kind, data_dir)` builds the configured backend as an `Arc<dyn Store>`. The signatures are under **Next up**. |
| `store/memory.rs` | `MemoryStore`: four `Mutex`es, the pool by track ID, the games by (player, section, day), the picks by (day, section) and the day offset. The backend for handler tests and for `kind = "memory"`. |
| `store/sqlite.rs` | `SqliteStore`: `<data_dir>/needledrop.db` through rusqlite, with SQLite compiled into the server (the `bundled` feature). One connection behind a mutex; every operation runs in `spawn_blocking`. `open(path)` creates the directory and the file, switches foreign keys on and migrates inside one transaction: `MIGRATIONS` is a list of `(version, script)`, and SQLite's `user_version` records how far a file has got. All SQL and every rusqlite type stay in this file. The scripts are in `store/sqlite/`: `001_songs.sql` (the tables `songs` and `song_genres`) and `002_games.sql` (the table `games`: `player`, `section`, `day`, and `state`, the game as `GameState`'s JSON; primary key player, section, day) and `003_picks_and_clock.sql` (the tables `picks`: `day`, `section`, `track_id`, primary key day and section, with no foreign key to `songs`, so a played song can leave the pool; and `clock`: one row at most, `day_offset`). There is no table of players. |
| `store/seed.rs` | `SEED_SONGS`, the six starting songs as plain data, and `seed_if_empty(store)`, which adds them through the trait when the pool has no song at all. |
| `store/contract.rs` | Test-only. The contract of `Store` as 37 cases over `&dyn Store` (15 for the pool, 11 for the games, 11 for the picks, the day offset and the wipe), and `contract_tests!(fixture)`, which turns them into tests for one backend. `memory.rs` and `sqlite.rs` both call it; a new backend must too. |
| `testutil.rs` | Test-only (`#[cfg(test)]`): `synthetic_mp3(frames)` and `MockDeezer`, a Deezer stand-in on a loopback port (`/track/{id}`, `/search/track`, preview downloads, a switch that makes it answer with the rate-limit error), so no test touches the network. Also what the handler tests of `routes`, `player`, `admin` and `daily` share: `TODAY` and `TestClock` (the real date is pinned, so no test depends on when it runs; `harness.clock.set(day)` is midnight passing), `Harness` (the whole router over the stand-in, a store and a data directory the test can reach; `start(tracks, answer)` puts one song in the pool, `with_pool(tracks, &[(id, genres)])` several, `with_store(…)` / `over(…)` choose the store, `restart()` builds a second server over the same store and directory, `pick(day, section)`, `player()`, `player_with_id(&id)`, `player_id(&player)`, `cookie_holding(name, value)`, `get` / `post` / `delete` for requests without a cookie), `Player` (a browser that keeps the player cookie; `daily(section)`, `audio(section)`, `skip_in(section)`, `guess_in(section, id)`, and `skip()` / `guess(id)` for General), `Reply` (`assert_error`, `assert_lacks(secrets)`, …), `BrokenStore` (every operation fails), and ready-made games (`playing_game`, `won_game`, `lost_game`). |

### API

All JSON keys are camelCase. The frontend is built against exactly this; do not rename fields.

| Route | Behaviour |
|---|---|
| `GET /api/health` | `{"ok":true}`. |
| `GET /api/today` | The overview behind the tabs. 200 `{ day, number, sections }`: `day` is the server's day, `number` counts from `launch_date` (1 on that day), and `sections` has one entry per section in tab order (general, pop, rock, hip-hop): `{ section, status, attempts, song }`. `status` is the player's game there today (`playing` / `won` / `lost`; `playing` with `attempts` 0 is a game not started), `attempts` is the number of tries used, and `song` is `"picked"`, `"pending"` (not picked yet, but the pool has a candidate; the pick is made when the section's game is first asked for) or `"none"` (no song today). It reads and never picks, so it is fast, and `pending` means "probably". Nothing in it comes from a song. Sets or refreshes the player cookie. `Cache-Control: no-store`. |
| `GET /api/daily/{section}` | `section` is `general`, `pop`, `rock` or `hip-hop`. 200 `{ day, number, section, ladder, attempts, status, clipSeconds, answer, stats }`. `day` is the server's day, `number` counts from `launch_date` (1 on that day), `section` is the slug asked for, `ladder` is `[0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0]`, `attempts` is `[{"kind":"skip"} \| {"kind":"wrong","title","artist"}]`, `status` is `playing` / `won` / `lost`, `clipSeconds` is the clip length unlocked now (30 once finished). `answer` is `null` while playing and `{ title, artist, album, cover, link }` once won or lost (`cover` is the 500 px album cover, falling back to smaller ones; `link` is the deezer.com track URL). `stats` is the player's record in that section (below); it is there in every state. The first request of a day for a section makes its pick (and those of the sections ahead of it in the pick order), which can take as long as a few downloads. Sets or refreshes the player cookie; a browser without one is given a new ID. Stores no game: a game is written with its first move. `Cache-Control: no-store`. |
| `GET /api/daily/{section}/audio` | 200 `audio/mpeg`: `mp3.prefix(unlocked_ms)` of the game the player cookie leads to in that section, frames only, with `Content-Length` and `Cache-Control: no-store`. No `Accept-Ranges`; a `Range` header and any query string are ignored. Does not set the cookie. Without a cookie it is the 0.1 s clip. |
| `GET /api/search?q=` | 200 `[{ id, title, artist, album, cover }]`, at most 8. `q` is trimmed (and cut at 100 characters); fewer than 2 characters → `[]` without calling Deezer. Otherwise 25 results are fetched from Deezer `/search/track`, tracks with a blank title dropped, de-duplicated by `TrackMeta::match_key()` keeping the first of each, then cut to 8. `title` is Deezer's full `title`, `cover` is `album.cover_small` (56 px, may be empty). |
| `POST /api/daily/{section}/guess` | JSON `{ "trackId": 123 }` or `{ "skip": true }` → 200 with the body of `GET /api/daily/{section}` after the move (the `stats` include a game this move finished), and the cookie. A correct guess sets `won` and adds no attempt. The game is stored. Moves of one player are made one at a time, so several sent at once each cost a try. Works without a cookie: that is the first move of a new player. |
| `GET /api/daily`, `GET /api/daily/audio`, `POST /api/daily/guess` | **Legacy**, to be removed by task 13: the three routes of the web client from before the sections, kept as aliases of the `general` section so that client still plays. Same bodies and errors as the per-section routes. |
| `DELETE /api/player` | Clear my data. 204 with no body. Deletes every game stored for the player in the cookie, in every section and today's included, and sets the cookie to a new ID. Without a cookie nothing is deleted and an ID is still issued. The day's songs do not change. The old ID is not revoked: a copy of the old cookie is a player without games. `Cache-Control: no-store`. |

`stats`, always for one player in one section, derived from the stored games on every request (nothing is kept as a counter, so a deleted game takes its share with it):

| Field | Meaning |
|---|---|
| `played` | Finished games, won or lost. A game that was started and not finished is not counted, on any day. |
| `won` | How many of those were won. |
| `winPercent` | `won` of `played` as a whole number from 0 to 100, rounded to the nearest (a half goes up); 0 when nothing has been played. |
| `currentStreak` | Consecutive days won, ending today, or yesterday while today's game is not finished. A loss, an unfinished game on an earlier day or a day without a game ends it. |
| `bestStreak` | The longest run of consecutive won days so far; never less than `currentStreak`. |
| `guessDistribution` | Seven numbers: the wins on the first try, the second, … the seventh. A win after `n` recorded attempts was on try `n + 1`. They add up to `won`. |

"Today" is the day of the game in the response: the server's day. Games dated after it (they exist when the admin has moved the clock back without a reset, or the machine's clock went back) are left out until their day comes.

Errors are `{ "error": "<code>", "message": "<sentence>" }`. The messages are fixed text, safe to show the player.

| Status | `error` | When |
|---|---|---|
| 400 | `bad_request` | Guess body that is not JSON, not `application/json`, or names both or neither of `trackId` / `skip`. No turn is used. |
| 404 | `unknown_track` | Deezer has no track with that `trackId`. No turn is used. |
| 404 | `not_found` | No such route, or a section slug that is not one of the four. |
| 404 | `no_song` | The section has no song today: its pool is empty, or everything in it plays in another section today or failed the preview check today. Answered by the section's game, audio and guess routes; the UI shows it as "No song today". Not something a retry mends (adding a song, or a re-roll elsewhere, can). |
| 409 | `finished` | A move on a game that is already won or lost. Checked before anything is looked up, and again when the move is made: of several moves sent at once, those that come after the one that ends the game get this. |
| 409 | `changed` | The section's song was replaced (an admin re-roll or reset) while the request was under way. A move is not made and nothing is stored; the client should load the game again. |
| 502 | `upstream` | Deezer failed or this server's request budget is used up (search, guess lookup), or the section's song could not be picked or loaded because Deezer did not answer (game, audio, guess). The pick is tried again by a later request, at most every 10 s; no song is skipped for it. |
| 500 | `internal` | The store failed (reading the day offset or the picks, reading or saving a game, clearing a player's data, the admin routes); the reason is in the log. |

**Anti-leak rule:** while a section's game is `playing`, nothing sent to the client may contain that section's answer — its Deezer ID, title, artist, album, cover or preview URL — JSON, headers, cookie and error messages alike. `GET /api/today` never contains any. The tests of `routes` and `player` assert it on playing-state responses, headers included; keep those assertions when adding routes. The cookie cannot leak: all it holds is the player ID. Finishing one section reveals that section's answer only; no song is the answer in two sections on one day, so it tells nothing about the others.

Cookie: named `gts_player`, holding the anonymous player ID — 32 lowercase hexadecimal characters, 128 random bits from `rand`'s OS-seeded generator — and nothing else, encrypted with axum-extra's `PrivateCookieJar`, so a client can neither read it nor make a cookie for an ID of its choosing. Attributes: `HttpOnly; SameSite=Lax; Path=/; Max-Age=34560000` (400 days, the most a browser keeps), plus `Secure` when the request has `X-Forwarded-Proto: https`. A TLS-terminating reverse proxy in front must send that header (in nginx, `proxy_set_header X-Forwarded-Proto $scheme`). On plain HTTP (the Vite dev server on localhost) the cookie is not `Secure`, or the browser would drop it. It is set again by every `GET /api/today`, every `GET /api/daily/{section}` and every move, so it only runs out for a player who stays away for 400 days. A cookie that is missing, does not decrypt or does not hold a well-formed ID is a new player: the overview, a section's game, a move and Clear my data then issue a new ID. The audio route never sets it. The old `gts_daily` cookie (the whole game state) is not read; it expires two days after it was last set.

Games: stored per player, section and day as `game::GameState` — `{"day":"2026-10-01","attempts":[{"kind":"skip"},{"kind":"wrong","title":"…","artist":"…"}],"status":"playing"}` — written by each move and never by a visit. No stored game for today means a fresh game. Stored titles and artists are cut to 80 characters and 160 bytes, so a row is under 2.7 kB. A stored game that cannot be read (not JSON, an impossible state such as `playing` with seven attempts, a game filed under another day than its own) is logged and counts as not there, so the player gets a fresh game for that day instead of an error, and the next move replaces the row. Unlocked clip length = `ladder[attempts.len()]`. A re-roll deletes every player's game for that section and day, and a reset every game.

Picks: one stored row per day and section (`store::Pick`), written once the song has been loaded, so a pick that stands can be played. The answers of a day are in the log (`picked the section's song for the day … title=… artist=…`) and in `GET /api/admin/state`.

Cookie key: 64 random bytes written as 128 hexadecimal characters. `GTS_SECRET` if set (`openssl rand -hex 64`), else `<data_dir>/secret.key`, generated on the first start with mode 0600 and reused after that, so cookies survive restarts. A malformed `GTS_SECRET` or key file stops the server at startup; delete the file to get a new key. A new key means no browser's cookie can be read any more: everyone becomes a new player, and the games stored under the old IDs stay in the database with nobody able to reach them.

### Admin API

Complete since task 12: the clock, today's picks and the re-roll, and the song pool with its search. **These routes are not protected**, and the anti-leak rule does not apply to them: `GET /api/admin/state` shows the day's answers. The conventions are the ones above: camelCase keys, the same error body with fixed messages, and `Cache-Control: no-store` on every response, errors included.

The state is `{ realDay, offset, day, number, sections }`: `realDay` is the real UTC date, `offset` the stored day offset (negative when the server's day is behind), `day` the server's day (`realDay` plus `offset`), `number` the day number (1 on `launch_date`), and `sections` has one entry per section in tab order: `{ section, status, pick }`. `status` is `"picked"` (the song is picked and loaded), `"none"` (no song today) or `"unavailable"` (Deezer did not answer, so the pick could not be made or its song could not be loaded; later requests try again). `pick` is `{ trackId, title, artist }` or `null`; with `unavailable` it is the standing pick named from the pool (blank title and artist if the song has left the pool), or `null` when there is none yet.

A song row is `{ trackId, title, artist, album, genres, previewFailedOn }`. `title` is Deezer's full title. `genres` is an array of the slugs `pop`, `rock`, `hip-hop`, always in that order, and empty for a song that is in the General pool only. `previewFailedOn` is `"YYYY-MM-DD"` or `null`: the day the daily pick last found the track without a preview. It is cleared the next time the song is picked and plays.

| Route | Behaviour |
|---|---|
| `GET /api/admin/state` | 200 with the state. **It makes the picks that are missing**, exactly as a player's first request of the day would (so it can take as long as a few downloads): the page exists to show the day's songs. A section that fails does not fail the others or the response. |
| `POST /api/admin/reroll` | Body `{ "section": "pop" }` (any of the four slugs) for one section; no body, `{}` or `{ "section": null }` for all four. The content type is not looked at. → 200 with the new state. Each named section loses its pick for the server's day and **every player's game there that day**; the state in the response then picks again, as a day's first request would. The replaced song is left out of the draw when the section has another candidate; songs the other sections play today are never drawn. With a single candidate the same song comes back, and the games are deleted all the same. If Deezer is down the section shows `unavailable` and the pick is made by a later request. |
| `POST /api/admin/next-day` | Simulate next day: adds one to the offset. → 200 with the new state (which makes the new day's picks). Nothing is deleted; every player is on the new day from their next request. |
| `POST /api/admin/reset` | Reset to day 1: deletes every game and every pick and sets the offset so that the server's day is `launch_date`. The song pool, `previewFailedOn` included, is kept. → 200 with the new state. |
| `GET /api/admin/songs` | 200 with every song row, sorted by artist, then title, ignoring case, then track ID. |
| `POST /api/admin/songs` | JSON `{ "trackId": 123, "genres": ["pop", "hip-hop"] }` → 200 with the stored song row. `genres` may be left out, `null` or `[]` (no tags); a slug given twice counts once. A song already in the pool only has its genres replaced: no Deezer request is made, and its stored title, artist, album and `previewFailedOn` are kept. Otherwise the track is fetched from Deezer (`/track/{id}`, one request against the budget) for its title, short title, artist and album, and stored. **It never checks `readable` or `preview`:** a track without a preview is added like any other. Other fields in the body are ignored. The response does not say whether the song was new. |
| `DELETE /api/admin/songs/{trackId}` | 204 with no body. The song and its genre tags are gone; adding it again starts from nothing. |
| `GET /api/admin/search?q=` | 200 `[{ id, title, artist, album, cover, playable }]`: the rows of the player's search plus `playable` (`readable` and a non-empty `preview`, read from the search result itself, so there is no request per row). The `q` rules are those of `/api/search`, and both searches share one cached Deezer answer of 25 tracks, but here every release is listed, in Deezer's order, up to all 25. Tracks with a blank title are dropped. |

| Status | `error` | When |
|---|---|---|
| 400 | `bad_request` | Add: a body that is not JSON, is not `application/json`, has no whole-number `trackId`, or names a genre that is not one of the three slugs. Nothing is looked up or changed. Remove: a track ID in the path that is not a whole number. Re-roll: a body that is not empty and not a JSON object, or whose `section` is not one of the four slugs; nothing is re-rolled. |
| 404 | `unknown_track` | Add: Deezer has no track with that ID, or the track has a blank title. |
| 404 | `unknown_song` | Remove: the pool has no song with that track ID. |
| 502 | `upstream` | Add of a song that is not in the pool yet, or search: Deezer failed or the request budget is used up. Nothing is added. |
| 500 | `internal` | The store failed, or the day offset leads outside the calendar. |

A method a route does not have (`GET /api/admin/songs/123`) gets axum's 405 with an empty body, as on the player's routes. Adding or removing a song never changes a pick that stands: a song removed from the pool stays today's song where it was picked.

### Frontend (`web/src/`)

The midnight listening-room UI (task 6), replacing the original weekday sleeves. The durable visual contract lives in `DESIGN.md`; this is where things live.

- `lib/api.ts` — types for the API (`Daily`, `Attempt`, `Answer`, `Track`, `Move`), `ApiError { code, message, status }` (`code` is the server's `error`, or `network` / `bad_response`), `getDaily`, `postGuess`, `searchTracks(query, signal)`, `audioUrl(daily)` (`?t=<attempts>-<status>`).
- `lib/clip.ts` — pure helpers, unit-tested: `findAudibleStart` (threshold 0.002, cap 60 ms), `playableSeconds`, `clipLabel` ("0.3 seconds"), `clipShort` ("0.3 s"), `ladderFraction` (equal-width ladder steps; the needle's position across the record's bands is exactly this).
- `lib/audio.ts` — `ClipPlayer`, framework-free: one lazy `AudioContext` created in a gesture (`unlock()`), `load(url)` (decodes on a 44.1 kHz `OfflineAudioContext`, so no gesture is needed to load), `play(seconds)` (`source.start(when, offset, duration)` 30 ms ahead, 5 ms gain fades, clamped to the buffer), `stop()`, `progress()`, `onstatechange`, `analyser`. Graph: source → clip gain → output gain → analyser → destination. It skips the MP3's leading priming silence so the 0.1 s clip is 0.1 s of sound.
- `lib/game.svelte.ts` — `Game` runes store and the `game` singleton: `daily`, `loadError`, `moveError`, `clipError`, `clipLoading`, `submitting`, `playing`, `notice`, `moves`; derived `status`, `finished`, `turn`, `lastTurn`, `clipSeconds`, `nextClipSeconds`, `totalSeconds`, `slots`; `load()`, `refresh()`, `play()`, `stop()`, `toggle()`, `skip()`, `guess(track)`, `progress()`. Reloads the clip whenever `audioUrl` or the day changes. A 409 `finished` re-fetches the state instead of showing an error. The design pass did not change this file, `api.ts`, `clip.ts` or `audio.ts`.
- `lib/sleeve.ts` — retained legacy helpers, pure and unit-tested, no longer used by the UI: `sleeveForDay(day)` (the UTC weekday of the game's `day`, 0 = Sunday) and `sleeveOverride(search)` (`?sleeve=0..6`).
- `lib/sky.ts` — the night sky as numbers, pure and unit-tested (no DOM, no clock, no `Math.random`): `seededRandom`, `starCount(width, height)` (one star per 2,750 px², 60 to 280), `generateStars(count, seed)` (`SKY_SEED` fixes the sky; a larger sky is a smaller one plus more stars), `twinkle`, `approach` / `smoothstep` / `easeOutCubic`, `starPoint` (drift, scroll parallax, wrap), `calmFactor` (dimming behind text), `planMeteor` / `meteorPose` / `meteorDelay`, `makeDust` / `dustPose`, `rms`, `hexChannels`, and the `Sky` class: `resize`, `step(dt, { playing, level, avoid })`, `celebrate(x, y, radius)`, `settle()`, `busy`. Every tuning constant is at the top of the file. It is separate from the painter because the tests are type-checked without DOM types (`tsconfig.node.json`).
- `lib/starfield.ts` — `StarfieldPainter`, framework-free canvas 2D drawing of a `Sky`: `configure(geometry)`, `setPalette(palette)`, `paint(sky, view)`. Halos and diffraction spikes are pre-rendered sprites (no `shadowBlur`); it keeps nothing else between frames.
- `lib/record.ts` — `RecordPainter`, framework-free canvas 2D drawing: `configure(geometry)`, `setPalette(palette)`, `invalidate()`, `paint(view)`. `paint` draws exactly what its `RecordView` says (lit amount per band, rotation, needle position, waveform, cover, printed fraction, progress arc); the only state it keeps is three cached layers (disc and grooves, sheen, blank label). All geometry constants (band radii, label, tonearm pivot and length) are at the top of the file.
- `lib/components/`
  - `Atmosphere.svelte` — the night sky behind the page: owns the viewport-fixed `<canvas>`, its size (DPR capped at 2, 4.2 M backing pixels) and the one `requestAnimationFrame` loop that steps a `Sky` and paints it (30 fps while only the slow drift is happening, every frame while a clip plays, a shooting star falls or the page scrolls). Props: `playing`, `paused` (the footer's Pause motion: holds the frame), `status` (`loading` or the game status; a change from `playing` to `won` sets off the stardust) and `analyser` (a getter for the audio analyser; star brightness breathes with its RMS level). It finds the page through two attributes instead of props: `[data-sky-calm]` (text: stars dim behind it and shooting stars avoid it) and `[data-sky-origin]` (the record's centre and radius). The two hazes are CSS gradients on its pseudo-elements. Hidden tabs stop the loop; reduced motion draws one static frame.
  - `Record.svelte` — owns the `<canvas>`, the animation state and the one `requestAnimationFrame` loop; reads the colours, the font and the layout switches from CSS custom properties.
  - `Play.svelte` — the play button (a round disc plus the label) and the one-line readout under the label; used while playing and in the reveal. Clips shorter than 3 s never show "Stop" (pressing again restarts them).
  - `GuessInput.svelte` — ARIA combobox; takes `onguess` and `busy`, knows nothing about the game. Places its list above or below the field from `window.visualViewport`. Search debounces 300 ms, cancels stale requests, waits for IME composition, and has an explicit clear button that restores input focus. Failed submissions retain the selected track.
  - `Skip.svelte` — "Skip to …" / "Give up".
  - `Attempts.svelte` — current and used tries with clip length, a mark (SVG shape), and words. Empty future rows remain available to screen readers but are visually hidden; the seven-step clip rail is in `App.svelte`.
  - `Reveal.svelte` — result line, title in display type, artist, cover and album, Deezer link, `Play`.
- `App.svelte` — header (inline SVG mark: a record with a four-point glint), complete record stage, question and seven-step clip rail, game controls/history, footer. Responsive side/stacked layouts; no weekday theme selection. Also owns the light behind the record (`.stage::before`, positioned from `--record-x` / `--record-y` / `--record-r`), the `[data-sky-origin]` probe in the stage, the `data-sky-calm` marks on the header text, `main`, the record caption and the footer, the one-time arrival animation, the current rail step's glint and breathing glow, and the Pause motion switch (`motionPaused` → `Atmosphere`'s `paused` and `.page.still`).
- `styles/tokens.css` owns the dark palette, shared type/spacing/control tokens and record geometry per layout. `styles/base.css` owns reset, `.button`, focus ring, `.error`, global scrollbars and reduced motion. Components own their layout and presentation. The sky's decorative tokens are `--sky-warm` and `--sky-cool` (colour channels, used as `rgb(var(--sky-warm) / α)`), `--star-cool` and `--accent-bright`; `.button.solid` carries the hover glow and light sweep.
- `index.html` — `viewport-fit=cover`, constant black theme-color and a matching vinyl SVG favicon (`public/favicon.svg`).
- Font: `@fontsource-variable/bricolage-grotesque` (dev dependency), `standard.css` — one variable file per subset with the weight, width and optical-size axes, bundled by Vite and served from this origin. No external font requests.
- `web/mock/` — a stand-in API for UI work without the server: `GTS_MOCK=1 GTS_MOCK_AUDIO=<mp3> pnpm dev` (from `web/`, in the dev shell). Not imported without the flag. One game per Vite process, no cookie; `POST /api/mock/reset` restarts it; the answer is "Paper Boats". Not re-run after the design pass.
- `web/test/` — `pnpm --dir web test` (`node --test`, no framework) covers `lib/clip.ts`, `lib/sleeve.ts` and `lib/sky.ts` (45 tests). Anything a test imports is type-checked without DOM types, so it must stay free of them.

Headless Chromium for browser checks comes from the Nix cache: `nix build --inputs-from . nixpkgs#chromium --out-link <scratch dir>/chromium-root`, then drive `<link>/bin/chromium` over the DevTools protocol with Node's built-in `WebSocket` (no dependencies). Use `--out-link` (in a scratch directory, not the repo), not `--no-link`: without a GC root the store path was garbage-collected in the middle of a test run on 2026-10-01. It has no audio output, but the `AudioContext` clock and the analyser run, so playback, the needle and the ripple can be screenshotted. Emulate a phone with `Emulation.setDeviceMetricsOverride` (`mobile: true`) plus `Emulation.setTouchEmulationEnabled`, the on-screen keyboard with a short viewport (390×480), and reduced motion with `Emulation.setEmulatedMedia`.

## How to work here

Everything runs inside the dev shell: `nix develop`, or prefix single commands with `nix develop -c`. Flakes only see files git knows about, so `git add` new files before expecting Nix to see them.

| Recipe | What it does |
|---|---|
| `just dev` | Server with auto-reload (bacon) and the Vite dev server together. Ctrl-C stops both. |
| `just server` | Server only, no auto-reload. |
| `just web` | Vite dev server only. |
| `just test` | `cargo test`. |
| `just check` | `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `svelte-check` + `tsc`, `vite build`. |
| `just build` | Release build of the server and the client bundle (`web/dist`). |
| `just fmt` | `cargo fmt`. |

All recipes run from the repo root, so the server's working directory is the repo root and `config.toml` and `data/` resolve there.

- **Ports:** `4810` Rust API (`GTS_BIND` overrides), `4811` Vite. Both bind `127.0.0.1` only.
- **Before a commit, both must pass:** `nix develop -c just check` and `nix develop -c just test`.
- **Real-preview test fixture:** `mp3::tests::real_preview_fixture` checks the frame walker against `server/tests/fixtures/preview.mp3` when that file exists, and prints a note and passes when it does not. The file is copyrighted audio and git-ignored (`server/tests/fixtures/*.mp3`); never commit it. Any Deezer `preview` download works as the fixture. All other MP3 tests build synthetic streams.
- **The machine may run other services**, including other Vite processes. Stop only what you started, by PID; never `pkill -f vite` or similar.
- Environment variables: `GTS_CONFIG` (config file path, default `config.toml` in the working directory), `GTS_BIND` (overrides `bind`), `GTS_SECRET` (cookie key, 128 hex characters; see the cookie notes above), `RUST_LOG` (tracing filter, default `info,tower_http=debug`). A variable that is set but blank counts as unset. `GTS_TRACK_ID` is gone with `track_id`; setting it does nothing. The store backend has no variable: it is `[store] kind` in `config.toml`, `"sqlite"` (the default) or `"memory"`. That table has to stay at the end of the file, because in TOML every key after a table header belongs to the table.
- **What the songs are:** the server logs each pick at `info` when it is made (`picked the section's song for the day day=… section=… track_id=… title=… artist=…`) and each load (`loaded a song from … title=… artist=…`); `curl http://127.0.0.1:4810/api/admin/state` lists today's four answers, making the picks if they are not made yet.
- **Deezer cache on disk:** `data/audio/<track_id>.mp3` and `.json` are reused on every start, so restarts (bacon restarts the server on each source change) cost Deezer nothing. Delete the two files to fetch again.
- **The database:** `data/needledrop.db` (SQLite): the song pool, every player's games, the daily picks and the day offset. The server creates it at startup, brings its schema up to date (version 3; an older database gains the `games`, `picks` and `clock` tables and keeps its songs) and, when the pool has no song at all, adds the six seed songs. Delete the file to start again from the seed; that also forgets every game and pick. To forget the games only: `DELETE FROM games`; the admin's Reset to day 1 deletes games and picks and keeps the pool. A file the server cannot use (not a database, a schema newer than the server knows, tables of another shape) stops it at startup with a message and is left as it was. Any SQLite client can read the file; `node:sqlite` in the dev shell's Node works. Tests never touch it: they use the in-memory store or a database in a temporary directory.
- **`just server` does not reload.** Only `just dev` (bacon) rebuilds and restarts the API when `server/src`, `server/Cargo.toml` or `config.toml` change. A server started with `just server` runs the binary it was started with until someone restarts it, so check which one is running before expecting a change to be live.

## Conventions

- Conventional-commit messages (`feat: …`, `fix: …`, `chore: …`).
- **No AI attribution in commits, ever.** No `Co-Authored-By` trailer for Claude, Claude Code or any other assistant, no `Claude-Session:` trailer or other session link, no "Generated with …" line. This overrides any tool default that adds them. The message is the subject and body only, authored as the repository owner.
- One commit per completed task, **not one per edit**. Follow-up corrections to the same piece of work go into that work's commit (amend or squash while it is unpushed) rather than a string of small commits. Push only when asked.
- Build tasks are delegated to subagents. The orchestrating agent briefs each one, reviews the diff, runs the checks and commits; subagents do not commit.
- Every task ends by updating the Progress section below, so this file and the git history never disagree.
- An agent cannot hear audio, and headless Chromium has no audio output, so anything about how a clip sounds (audible, click-free, right length) must be checked by the owner by ear in a real browser. Say so instead of claiming it works.
- Be gentle with Deezer (see the rate limit below). Do not loop over the API to explore; cache responses to a scratch directory and analyse them offline.

## Deezer facts worth not rediscovering

- `api.deezer.com` sends no `Access-Control-Allow-Origin`, so it must be called from the server, never from the browser.
- Rate limit: **50 requests per 5 seconds** per IP. Autocomplete needs the TTL cache.
- `/search/track`, `/track/{id}` and `/playlist/{id}/tracks` all work without authentication.
- `/playlist/{id}/tracks?limit=100&index=N` paginates; the response carries `total`. `limit=100` is the largest page used here.
- Preview URLs are signed (`…dzcdn.net/….mp3?hdnea=exp=…~hmac=…`) and **expire after about 15 minutes**. Download the MP3 once and cache the bytes; never store the URL.
- Previews are 30 s, 128 kbps CBR, 44.1 kHz MP3: a 10-byte ID3 header, then frames of about 418 bytes / 26 ms. A clip can be cut by slicing at frame boundaries; no transcoding or ffmpeg. The one preview measured (479,827 bytes) is an empty 10-byte ID3v2.4 tag plus exactly 1148 frames of 417/418 bytes: no Xing/Info frame, no ID3v1 trailer, no partial frame. That is **29.988 s, not 30**, so the last ladder step is "everything" rather than a full 30 000 ms.
- Tracks carry `title_short` — the title without "(Remastered …)" and similar — next to `title` and `title_version`. Use it for matching.
- A track that cannot be played has `"readable": false` **and** `"preview": ""`; in every playlist checked the two always went together. Filter on both anyway.
- **User playlists rot.** Old playlists point at track IDs that have since been withdrawn, so a large share of a playlist can be unplayable. Measured on 2026-10-01 from a US IP address (availability varies by country): `88551731` "All time hits" 51 of 139 playable (37%), `3564546242` "500 Greatest Hits of All Time" 193 of 494 (39%). Check a playlist's playable share before adding it.
- Playlist `title` is cut off at 50 characters by the API.
- Each track has a `rank` (Deezer popularity, higher = better known) that could filter out obscure tracks.

## Design direction

**Midnight listening room**, approved on 2026-10-01. The owner requested a complete replacement of the saturated sleeve UI with an OLED-black design while retaining the spinning record and needle. Read **DESIGN.md** for the maintained visual contract and token mapping. The old weekday palettes and `?sleeve=` overrides no longer affect the page.

### Palette and type

- True-black page `#000000`, graphite surfaces `#151515`, warm ivory `#F3EFE7`, champagne accent `#D8B983`, muted silver `#96999E`.
- The sky adds decorative tokens only: `--sky-warm` (`216 185 131`, the accent as light), `--sky-cool` (`58 86 156`, one deep blue), `--star-cool` `#C7D2E8` and `--accent-bright` `#E8D2A6` (a lit champagne control). The hazes stay at low alpha (0.3 at most, at the record's edge), so the field still reads as black. None of them carries text.
- Decorative borders `#303030`; the search control uses `--control-border: #666666` for a 3.18:1 boundary against graphite. Ivory on black is 18.31:1; muted text on graphite is 6.39:1; dark ink on champagne is 9.95:1.
- Self-hosted Bricolage Grotesque: regular-width weight 600 question, condensed weight 800 revealed song title, 400/600 body and labels. Input text stays 16 px or larger. Clip times use tabular numerals.
- Champagne marks the play button, available Guess button, unlocked clips and success. Every state also has words or a distinct shape. The search popup highlights its active result with an accent edge. Controls have visible keyboard focus.

### Layout and controls

- A full-width header carries the compact wordmark and daily pressing number. Desktop places the complete record left and a focused game column right, with a quiet footer. No fixed/cropped record or generic card shell.
- Side layout starts at `(min-width: 40rem) and (min-aspect-ratio: 6/5)`. Other sizes stack the record above a centered game column. Short landscape and short portrait have compact variants; the smallest portrait hides the decorative headline and reduces record size so Play, search, Guess and Skip fit at 320×568.
- The seven clip lengths form a fine progress rail. It is decorative to screen readers because Play and Attempts convey the same information. The order remains listen → search/guess → skip → current/used attempts → rules.
- Page scrolling owns overflow. Touch targets are at least 44 px; primary controls are 56 px. Safe-area gutters, `svh` with `vh` fallback, and a visual-viewport-aware autocomplete popup support phones. Real phone keyboards still need owner verification.
- Inputs and buttons have restrained 6 px corners. The record and play control are circular. No bright background themes, neon rings, decorative pills or dashboard cards.

### The record and motion

The record hangs in a night sky (task 9), which replaced the groove trails and floating notes. One viewport-fixed canvas behind the page shows 60 to 280 stars, the same on every visit, at three depths; each twinkles on its own 3 to 8 second cycle and the whole sky drifts westward at 0.35 to 1.3 px/s. While audio plays the drift eases up to five times that pace and the stars brighten and breathe with the sound, then ease back. A thin champagne shooting star crosses the upper sky every 9 to 22 s (about 5 to 12 s while playing) for about a second. Stars dim to a fifth behind text (`data-sky-calm`) and shooting stars keep out of it. Two CSS hazes (warm behind the record, deep blue opposite) drift over 68 to 84 s, and a soft light behind the record (`.stage::before`), brightest at the disc's edge, strengthens during playback. A win made on the page releases stardust and a flare from behind the record for about 1.6 s, then one brighter shooting star. The page arrives once on load (sky, header, record, game column, footer); the current rail step glints when it unlocks and then breathes; Play and Guess glow on hover. The sky stays behind the page, ignores pointer input and is hidden from assistive technology. The footer can pause/resume it; hidden tabs suspend it and reduced motion draws one static frame. The record remains the main visual object.

Canvas 2D, 2.24 record-radii wide, with DPR capped at 2 and backing store capped at 1800 px per side. The painter keeps three cached layers: graphite disc/grooves, reflected sheen, and ivory label. The label carries Needledrop, test-pressing details and the UTC game date. The tonearm has silver counterweight, bearing, tube and headshell details with a champagne stylus.

Seven groove bands remain (outer radius 0.955 to inner 0.385, 0.01 gaps); unlocking adds a subtle champagne tint. The numeric scale was removed from the disc because the separate clip rail carries it. Label radius is 0.30; spindle radius is 0.022.

Playback drives 33⅓ rpm rotation, smooth acceleration/coasting, stylus movement through `ladderFraction`, and analyser-driven groove movement. The reflection rocks slightly with rotation. The animation loop sleeps when settled and cancels while the tab is hidden. No additional animation loop was added to the renderer.

On a fresh win/loss, the album art prints onto the rotating label, the result and song details reveal in sequence, and the disc coasts upright. Reloading a finished game shows the settled text immediately. With reduced motion, rotation, needle movement, groove response and CSS reveal motion stop; a progress arc still communicates playback. The backend, game store and audio engine are unchanged by this redesign.

## Roadmap: sections, song database, players and admin

Agreed with the owner on 2026-10-01. This is the specification for tasks 10–14 in Progress. Items marked *(assumed)* were not asked; they are the builder's defaults and the owner may overrule them.

### Sections and the daily pick

- Four sections, each a separate seven-attempt game per day: `general`, `pop`, `rock`, `hip-hop` (the slugs used in URLs, the API and the database).
- Every song in the database is in the General pool. A song may also carry **any number of genre tags** (none, one or several of pop / rock / hip-hop) and is in the pool of each genre it is tagged with.
- **No song is the answer in two sections on the same day.** Picks are made in a fixed order — Pop, Rock, Hip-hop, then General — and each excludes the songs already picked that day, so playing a genre section never spoils General.
- A section does not repeat a song until its pool is used up *(assumed)*: prefer songs not picked in that section since the pool was last exhausted, and never yesterday's when there is another choice.
- Picks are stored (day, section, track), so a restart keeps the day's songs. They are made lazily, the first time a day is asked for.
- **The daily pick checks that the preview is available.** It runs once per section per day (and again on an admin re-roll), and it has to fetch the track and its preview to play it anyway, so the check costs nothing extra. If Deezer says the track has no preview (`readable: false` or an empty `preview`), that song is skipped for the day and another is drawn. The add endpoint stays unvalidated (see Admin), so this is the safety net for songs that were added unchecked or withdrawn later.
- Only "this track has no preview" skips a song. Deezer being unreachable or over the request budget is not a verdict on the song: the pick fails with 502 `upstream` and is retried, as the song load is today, rather than burning through the pool *(assumed)*.
- A skipped song stays in the database and is tried again on later days; the day it last failed is recorded and shown in the admin pool list so the owner can remove it *(assumed)*.
- A section with no pickable song after the day's exclusions and skips answers with an error the UI shows as "No song today" *(assumed)*.
- **Seed:** six songs, two per genre, in `server/src/store/seed.rs` (`SEED_SONGS`; chosen on 2026-10-01; that day each was `readable` and its preview downloaded as a full 479,827-byte MP3): Pop — "Billie Jean" (Michael Jackson, `4603408`), "Toxic" (Britney Spears, `15391618`); Rock — "Bohemian Rhapsody" (Queen, `4091937401`), "Back In Black" (AC/DC, `92720046`); Hip-hop — "Lose Yourself" (Eminem, `1109731`), "Juicy" (The Notorious B.I.G., `3616616`). The server inserts them at startup, through the `Store` trait, when the pool has no song at all, so later removals stick (removing every song brings the six back at the next start). A database had been built by hand from the old `server/seed.sql`, with the same two tables and no schema version; the SQLite backend adopts such a file as schema version 1 and keeps its rows.
- **The database stores track IDs and display metadata only — never audio or preview URLs.** Preview URLs are signed and expire in minutes; the MP3 is downloaded when a song is picked for a day and cached under `data/audio/`, as today. With the seed alone, each genre alternates its two songs and General picks one of the three the genres did not take that day.
- `track_id` in `config.toml`, `GTS_TRACK_ID` and the `playlists` key are gone since task 12: picks come from the database.
- *Built in task 12* as written above, with these details: a pick is stored only after its song has been loaded, so the check is the load; a song found in the disk cache counts as playable without asking Deezer; "no preview" also covers a preview body that is not an MP3 and a track Deezer no longer knows; a failed check is cleared the next time the song is picked and plays; General waits behind a genre section that is stuck on a Deezer failure (they share the one Deezer); a re-roll would rather repeat yesterday's song than draw the replaced one again.

### Storage

SQLite, one file under `data_dir` *(the SQLite crate is the builder's choice; it must not need a system library or a separate service)*. It holds the songs and their genres, the picks, the players, their games and the day offset. The parsed-preview disk cache in `data/audio/` stays as it is. Built: the trait, both backends and the contract suite, on rusqlite with its `bundled` SQLite, with the song operations (task 10), the players' games (task 11) and the picks and the day offset (task 12). "The players" turned out to need no storage of their own: a player is an ID in a cookie and exists in the database only as the games stored under it.

**The database sits behind one abstraction**, so MySQL, Postgres, a document store or Firebase can replace SQLite later by writing one new implementation and nothing else (asked for by the owner on 2026-10-01):

- A `Store` trait (async) in its own module is the only way the rest of the server touches persistent data. Handlers, the pick and the stats hold an `Arc<dyn Store>`; no SQL, connection type or database error type appears outside the implementation.
- Its methods are **domain operations, not queries**: list / add / retag / remove songs; get a day's picks and the pick history of a section; save a pick; load and save a player's game; list a player's games in a section (all of them: an unfinished game on an earlier day is a missed day, which the stats need to see); delete everything a player has; delete games by day and section; get and set the day offset; wipe picks, games and players. They take and return plain domain types (`game::GameState`, track IDs, dates).
- **Logic stays in Rust, above the trait.** Choosing the pick is a pure function over the pool and the history; stats are computed from the list of games. A backend only fetches and stores, so nothing depends on joins, aggregates or SQL dialect — which is what makes a document store possible.
- The only atomicity a backend must provide is single-record: "save this pick unless the day and section already have one" (two requests at midnight must agree on the song), and "replace this game". No multi-table transactions. That two moves of one player do not both build on the same stored game is settled above the trait, by a lock in the server (`player::MoveLocks`), not by a conditional write.
- Two implementations from the start: `SqliteStore`, and an in-memory one for tests. One shared test suite runs against both and is the contract a future backend has to pass.
- The backend is chosen in `config.toml` (for example `[store] kind = "sqlite"`), defaulting to SQLite.
- The seed songs are inserted through the trait from a backend-neutral list (`store/seed.rs`), so every backend is seeded the same way. The schema belongs to the SQLite backend (`store/sqlite/001_songs.sql`).

**Stats are derived from the stored games, not kept as counters.** Re-rolling a pick or resetting the clock deletes games, and derived stats then correct themselves.

### Players, streaks and stats

- The cookie carries only a random anonymous player ID (HttpOnly, long-lived, refreshed on each visit); every game and stat is in the database. Reasons: four games do not fit in one 4 kB cookie (one worst-case game is 2.7 kB), and server-side streaks cannot be forged. The old `gts_daily` cookie is ignored. *(Built in task 11, for the one section there is; the details are under **API** near the top of this file.)*
- **One streak per section.** A streak is consecutive days **won**: a loss or a missed day resets it to zero.
- Recorded and shown per section: current streak, best streak, games played, win rate, and the guess distribution (which of the seven attempts the win came on).
- **Clear my data:** a button on the player page, behind a confirmation. It deletes that player's games and stats, today's attempts included, and issues a new anonymous ID. The day's songs do not change. *(The route is built: `DELETE /api/player`. The button is task 13.)*

### The day clock

- The effective day is the real UTC date plus a stored offset in days, **for the whole server**: every player moves together, exactly as at a real midnight. Everything that says "today" (picks, games, streaks, the day number) uses the effective day.
- **Simulate next day** adds one to the offset.
- **Reset to day 1** sets the effective day back to `launch_date` and deletes all picks, games and players. **The song database is kept.**
- *Built in task 12* (`Daily::days`, `next_day`, `reset`; the offset is one record in the store). There are no player records to delete: a player is the games stored under an ID, so after a reset every browser keeps its cookie and has no games.

### Admin

`/admin` in the web app, and `/api/admin/*` on the server. **Unprotected for now** by the owner's choice; it must be protected or disabled before any public deployment, because it can show the day's answers and wipe every player's data. The anti-leak rule applies to the player routes, not to these.

Admin API (route names are a proposal; the behaviour is the requirement). All of it is built as written here (tasks 10 and 12); the exact shapes and error codes are under **Admin API** near the top of this file:

| Route | Behaviour |
|---|---|
| `GET /api/admin/state` | Real date, offset, effective day and day number, and today's pick for each section. |
| `GET /api/admin/songs` | The whole pool: track ID, title, artist, genres, and the day its preview last failed the daily check, if it has. |
| `POST /api/admin/songs` | `{ "trackId": 123, "genres": ["pop", "hip-hop"] }` adds the song, or replaces the genres of one already there. **It never checks that a preview exists; validation is the caller's job.** Two callers share it: the admin UI, which validates first, and a later bulk-add script or AI agent, which validates each song itself before adding it. It still looks the track up on Deezer for its title and artist, so an unknown ID is an error and callers must stay under the request budget. A song that slips through without a preview is caught by the daily pick. |
| `DELETE /api/admin/songs/{trackId}` | Removes a song from the pool *(assumed)*. |
| `GET /api/admin/search?q=` | The player search plus a `playable` flag per result, without the de-duplication that hides alternative releases. |
| `POST /api/admin/reroll` | `{ "section": "pop" }`, or no section for all four. Picks again for the effective day, as a real daily refresh would, choosing a different song where the pool allows. Every player's game for that day and section is deleted. |
| `POST /api/admin/next-day` | Simulate next day. |
| `POST /api/admin/reset` | Reset to day 1. |

Admin UI:

- **Clock:** the effective day and day number, **Simulate next day**, and **Reset to day 1** behind a confirmation.
- **Today's picks:** the song of each section, with **Re-roll** per section and **Re-roll all**. There is deliberately no way to choose a specific song for a section: the owner asked for re-roll only.
- **Add a song:** search with the same drop-down as the game. All results are shown, playable or not. **The validation lives here, in the UI:** choosing a result whose `playable` flag (from the admin search) is false shows an error and adds nothing. For a playable one the owner ticks the genres and adds it through the unvalidated endpoint.
- **The pool:** the list of songs with their genres, a way to change the genres, and remove *(list and remove are assumed)*.

### Player UI

- A row of four tabs under the header — General, Pop, Rock, Hip-hop — each showing whether today's game there is unplayed, won or lost. The record, clip rail and controls show the selected section's game. Each tab has its own URL (`/`, `/pop`, `/rock`, `/hip-hop`) using the History API; no router library *(assumed)*.
- Stats for the section appear with the result once its game is finished, and the current streak is visible while playing *(placement is the designer's)*.
- The Clear my data button, in the footer *(assumed)*.
- Follow `DESIGN.md`; the admin page can be plain but uses the same tokens.

### Player API changes

The single-game routes become per-section. Proposal: `GET /api/daily/{section}`, `GET /api/daily/{section}/audio`, `POST /api/daily/{section}/guess`, the existing response fields unchanged with `section` and `stats` added; an overview route for the tabs (status of all four sections today); and a route for Clear my data. `GET /api/search` is unchanged. All built: `section`, `stats` and Clear my data (`DELETE /api/player`) in task 11, the per-section routes and the overview (`GET /api/today`) in task 12. The three routes without a section remain as legacy aliases of General until task 13 moves the client.

## Progress

- [x] **1. Scaffold** — 2026-10-01. Flake dev shell, justfile, `config.toml`, compiling server skeleton with `GET /api/health`, Vite + Svelte placeholder that calls it, this file. `just check` and `just test` pass.
- [x] **2. Server core** — 2026-10-01. `mp3.rs` (frame walker, `Mp3::prefix`) and `game.rs` (ladder, `GameState`, normalization, matching), both pure, 68 unit tests. Nothing calls them yet. `just check` and `just test` pass.
- [x] **3. Server I/O** — 2026-10-01. `config.rs`, `deezer.rs`, `daily.rs`, `routes.rs` and `testutil.rs`, wired in `main.rs`; the API table above is what was built. Shipped with **one hard-coded track** (`track_id` in `config.toml`, `GTS_TRACK_ID`); the playlist-based daily pick is deferred to its own item below. 136 unit tests (handler tests run against a local Deezer stand-in); the curl walk-through against real Deezer passed (fresh game, seven clip sizes, wrong guess, win, loss, 409 after the end, no answer in any playing-state response). `just check` and `just test` pass.
- [x] **4. Frontend plumbing** — 2026-10-01. API client, Web Audio clip player, runes game store, plain components (controls, timeline, autocomplete, tries, reveal), a flag-gated mock API and 11 unit tests for the pure helpers. Built in parallel with task 3, then run together: a full game (play, skip, wrong guess, reload, right guess, reveal) passes in headless Chromium with no console errors, HMR websocket included. Not yet confirmed by ear.
- [x] **5. Design pass** — 2026-10-01. Before it started, the owner tested the plain UI of task 4 in his browser and confirmed it "works perfectly", audio included. Then: seven weekday sleeves (contrast-checked, `?sleeve=0..6`), self-hosted Bricolage Grotesque, the record canvas (`lib/record.ts`, `Record.svelte`), side and stacked layouts with the phone as a first-class target, result list placed by the visual viewport, reveal sequence, reduced-motion mode, SVG favicon. Logic files untouched. Checked in headless Chromium against the real server at 320, 360, 390 and 430 px portrait, 844×390, 768×1024, 1024×768, 1440×900 and 1920×1080: a scripted game (play, skip, keyboard-only wrong guess, reload, right guess, reveal, full clip; a lost game; a reduced-motion game) passes with no console errors or warnings and no frames requested while idle. `just check` and `pnpm --dir web test` pass. Production bundle: JS 74.4 kB (28.3 kB gzip), CSS 13.4 kB (3.7 kB gzip), font 131.5 kB (latin; latin-ext 53.6 kB and vietnamese 22.0 kB load only when needed). **Not checked on a real phone or by ear** since the restyle.
- [x] **6. Midnight redesign + verification** — 2026-10-01. Owner approved OLED black, graphite vinyl, champagne/ivory type and a metal tonearm, replacing task 5's bright sleeves. Added `DESIGN.md`, responsive whole-record layouts, compact clip rail/history, dark autocomplete with clear/IME support, matching favicon, and global scrollbar tokens. Headless-browser checks at 320×568, 360×740, 390×844, 844×390, 768×1024, 1024×768, 1024×1366, 1440×900 and 1920×1080 passed. Isolated browser fixtures covered play/stop and animated canvas, skip, keyboard wrong guess, reload, win/loss, full clip, clear-focus, IME, no results, injected search/move/load errors and retry, loading, reduced motion, and 390×480 keyboard-height popup placement. No unexpected browser errors; expected injected 502s were recorded. Real API smoke also passed. `just check`, `just test` (136), frontend tests (14), strict design audit and official DESIGN lint passed; independent review found no blocking issues. Bundle: JS 77.28 kB (29.30 gzip), CSS 17.10 kB (4.45 gzip). **Not checked by ear or on a physical phone/Safari since this redesign.**
- [x] **7. Animated background atmosphere** — 2026-10-01. Added oversized sound trails, continuously sweeping champagne highlights and twenty independently drifting dust motes. Playback strengthens the wave; the footer offers Pause motion / Resume motion. Decoration scrolls with the record, with masks protecting the control area, hidden-tab suspension and reduced-motion support. The owner requested more visible idle animation after the initial restrained version. **Final stronger-motion revision was not tested or screenshotted at the owner's explicit request; the owner will verify it.** Existing API/audio/game logic and preview ports are unchanged.
- [x] **8. Playback-only musical notes** — 2026-10-01. Replaced dust with small authored SVG eighth notes and beamed pairs floating slowly at varied depths. Notes, champagne sweeps and the wave now move only while audio plays and pause in place between clips. Pause override, hidden-tab suspension and reduced-motion support remain. **No tests or screenshots run, as requested by the owner.**
- [x] **9. Deep-space atmosphere** — 2026-10-01. The owner disliked the floating notes and groove circles and asked for tasteful "space vibes". `Atmosphere.svelte` is now a canvas night sky (`lib/sky.ts` model, `lib/starfield.ts` painter): seeded three-depth starfield with twinkle and slow drift, playback-reactive pace and brightness, shooting stars, two CSS hazes, stars dimmed behind text, and a stardust burst on a win. `App.svelte` gained the eclipse light behind the record, a new SVG header mark (the `◎` glyph is gone), a one-time arrival sequence and a living current step on the clip rail; Play and Guess have glow / light-sweep states. Record, game store, API client and audio engine untouched; `Atmosphere` gained the `status` and `analyser` props. `just check` and `pnpm --dir web test` (45 tests, 31 new for `lib/sky.ts`) pass. Bundle: JS 88.52 kB (33.58 gzip), CSS 23.73 kB (5.77 gzip). **NOT visually verified: no browser was run and no screenshot taken, at the owner's request.** Every size, alpha and timing was chosen by reasoning and one rough offline rasterisation of star positions, so expect tuning. The owner should look at: star density and brightness at idle on a 1× desktop monitor and on a phone (constants at the top of `lib/sky.ts`); whether the idle drift is unnoticeable while typing; the light behind the record at idle and during a long clip (it must read as an eclipse, not a ring); the hazes (the page must still read as black, without banding); a shooting star; the win burst; the arrival sequence; the rail's current step; Play and Guess hover; Pause motion; and scrolling on a phone (the sky is viewport-fixed, the record's light scrolls with the record).
- [x] **10. Song database (server)** — 2026-10-01. `store/`: the async `Store` trait with the song operations (list, get, add or retag, set genres, remove, record or clear the day a preview check failed), `SqliteStore` (rusqlite 0.40 with SQLite compiled in, schema versioned by `user_version`) and `MemoryStore`, a 15-case contract suite that runs against both, the backend-neutral seed list, and `[store] kind` in `config.toml`. `admin.rs`: `GET` and `POST /api/admin/songs`, `DELETE /api/admin/songs/{trackId}` and `GET /api/admin/search` (see **Admin API**). `server/seed.sql` is gone. Nothing reads the pool: the player routes and `Daily` are unchanged and the game still plays `track_id`. 213 server tests (77 new: 51 store, 23 admin, 3 config); `just check` and `just test` pass. A copy of the hand-built `data/needledrop.db` was opened with the new backend: adopted as schema version 1, its six songs listed, nothing seeded again. **Not run on the live server or against real Deezer.** The API on 4810 had been started with `just server`, which does not reload, and this task was told not to restart it, so the admin routes are proven only by handler tests against the Deezer stand-in. After a restart the owner should check: the server starts on the existing database, `GET /api/admin/songs` lists the six seed songs, and an add, a retag, a remove and an admin search work against real Deezer.
- [x] **11. Anonymous players and stats (server)** — 2026-10-01. The cookie is now `gts_player`, an encrypted random player ID and nothing else (`player.rs`); the games are in the store per player, section and day (`Store::game`, `save_game`, `games`, `delete_player`; `002_games.sql`; both backends; the contract suite grew from 15 to 25 cases), written by a move and never by a visit; `Section` and `PlayerId` are domain types in `store/mod.rs`, and every game is stored under `general`. `stats.rs` derives the record (played, won, win percent, current and best streak, guess distribution) from the stored games, and `GET /api/daily` and the guess response carry it as `stats`, next to the new `section`. `DELETE /api/player` is Clear my data. A player's moves are made one at a time (`player::MoveLocks`), so moves sent together cannot share a try; four handler tests over a store that yields between read and write fail without the lock and pass with it. The cookie session (`load_game` / `store_game`) and the fits-in-a-cookie test are gone; the cookie key code moved from `routes.rs` to `player.rs`; the two test harnesses became one in `testutil.rs`. 293 server tests, 80 more than before: 21 for the stats, 24 in `player` (6 of them moved from `routes` with the cookie key), `routes` from 34 to 38 (17 new, 7 cookie-session tests gone), 6 for the new store types, 10 contract cases on each backend and 5 more for SQLite. `just check` and `just test` pass. `web/` is untouched: the client ignores the new fields. **Nothing was run on the live server**: the API on 4810 was an old binary started with `just server`, and this task was told not to start or restart anything, so all of it is proven by tests only (handler tests over the Deezer stand-in and the in-memory store, one over SQLite in a temporary directory). After a restart the owner should check: the server starts on the existing `data/needledrop.db` (it gains the `games` table, the six songs stay); a game in progress under the old cookie starts again, once; a reload after a move shows the move; `GET /api/daily` shows `section` and `stats`; winning or losing changes `stats`; a second browser profile is a separate player; `curl -X DELETE` on `/api/player` with the browser's cookie empties the game and the stats.

- [x] **12. Sections, daily picks and the day clock (server)** — 2026-10-01. `pick.rs` is the pick as a pure function (21 tests, with a seeded generator). `daily.rs` was reworked around it: the server's day (`Clock`, real date plus the stored offset), lazy picks in the fixed order, each stored only once its song is loaded, a song without a preview marked and skipped for the day, a Deezer failure retried without burning the pool, up to eight songs in memory, and `reroll` / `next_day` / `reset` behind a read-write lock that `stands` lets a request hold. The store gained `delete_games`, `picks_on`, `pick_history`, `save_pick` (unless one stands), `remove_pick`, `day_offset`, `set_day_offset` and `wipe_games_and_picks` (`003_picks_and_clock.sql`, schema version 3; the contract suite grew from 25 to 37 cases). Routes: `GET /api/today` (the overview), `GET /api/daily/{section}`, `…/audio`, `…/guess`, the three old routes kept as legacy aliases of General, the errors `no_song` (404) and `changed` (409); admin: `GET /api/admin/state`, `POST /api/admin/reroll`, `/next-day`, `/reset`. `track_id`, `GTS_TRACK_ID` and `playlists` are gone (an old config file still loads). The handler tests run on a pinned clock, so none depends on the time of day. 392 server tests, 99 more than before: `pick` 21, `daily` 28 (was 8), `routes` 54 (was 38), `admin` 35 (was 23), `config` 13 (was 14), 12 new contract cases on each backend and 7 more SQLite-only tests. `just check` and `just test` pass. `web/` is untouched and keeps working through the legacy routes. **Nothing was run on a live server or against real Deezer**: the API on 4810 was an old binary started with `just server`, and this task was told not to start or restart anything. The subagent building it was interrupted before its hand-over; the orchestrator read the code, ran the gates and wrote this file's update. After a restart the owner should check: the server starts on the existing database and logs four picks (Pop, Rock, Hip-hop, General) with four different songs; `GET /api/admin/state` shows them; the current UI still plays (it is the General section); `curl` on `/api/daily/pop` and `/api/today` with the browser's cookie; `POST /api/admin/next-day` gives a new day, new picks and fresh games while streaks carry; `POST /api/admin/reroll` with `{"section":"pop"}` gives Pop its other song and empties that game; `POST /api/admin/reset` returns to day 1 with no games.

The remaining tasks implement the **Roadmap** section above, in this order. Each is one subagent-sized task and ends with a report to the owner saying what he should test.

- [ ] **13. Player UI: sections, stats, clear data** — the four tabs with their URLs and per-section state, the stats display, the streak while playing, the Clear my data button with its confirmation, "No song today".
- [ ] **14. Admin UI at `/admin`** — clock, today's picks with re-roll, add a song by search with the preview check, and the pool list with genre editing and remove.

### Next up

- The owner reviews the UI on a physical phone and desktop: the night sky (see task 9), the record during a longer clip, reveal, autocomplete with the real keyboard, and first-tap audio in iOS Safari. Follow-up polish should use `DESIGN.md`.
- Tasks 13 and 14. They are both web-only apart from the removal of the legacy routes (task 13), and they share one seam: `web/src/main.ts` / `App.svelte` has to send `/admin` to the admin page and everything else to the game. What they need to know:
  - **Every route the UI needs is specified, with exact shapes and error codes, under API and Admin API near the top of this file.** The same shapes as TypeScript:

    ```ts
    type Section = 'general' | 'pop' | 'rock' | 'hip-hop'        // tab order
    type Status = 'playing' | 'won' | 'lost'
    type Stats = { played: number; won: number; winPercent: number; currentStreak: number; bestStreak: number; guessDistribution: number[] /* 7 */ }

    // GET /api/today — the tabs
    type Today = { day: string; number: number; sections: { section: Section; status: Status; attempts: number; song: 'picked' | 'pending' | 'none' }[] }

    // GET /api/daily/{section}, and the body of POST /api/daily/{section}/guess ({ trackId } | { skip: true })
    type Daily = { day: string; number: number; section: Section; ladder: number[]; attempts: Attempt[]; status: Status; clipSeconds: number; answer: Answer | null; stats: Stats }
    // GET /api/daily/{section}/audio → audio/mpeg; DELETE /api/player → 204; GET /api/search?q= → Track[] (unchanged)

    // GET /api/admin/state, and the body of POST /api/admin/reroll ({ section } or no body), /next-day, /reset
    type AdminState = { realDay: string; offset: number; day: string; number: number; sections: { section: Section; status: 'picked' | 'none' | 'unavailable'; pick: { trackId: number; title: string; artist: string } | null }[] }
    // GET /api/admin/songs → SongRow[]; POST /api/admin/songs ({ trackId, genres }) → SongRow; DELETE /api/admin/songs/{trackId} → 204
    type SongRow = { trackId: number; title: string; artist: string; album: string; genres: ('pop' | 'rock' | 'hip-hop')[]; previewFailedOn: string | null }
    // GET /api/admin/search?q= → AdminHit[]
    type AdminHit = { id: number; title: string; artist: string; album: string; cover: string; playable: boolean }
    // every error: { error: string; message: string } — the message is safe to show
    ```

  - **What the player page has to handle.** 404 `no_song` on a section's game: show "No song today" for that tab (the overview's `song: "none"` says so in advance; `"pending"` can still turn into `no_song`). 409 `changed` on a move: the song was re-rolled under the player; load the section's game and the overview again and say so. 409 `finished`: re-fetch, as today. 502 `upstream`: try again in a moment, as today. A `day` in any response that differs from the one on screen means midnight (or the admin's clock) has passed: reload the overview and the game. The first request of a day for a section can take seconds (the picks are made then), so each tab needs a loading state. After a move, the tab's status can be updated from the response without asking the overview again. After Clear my data (`DELETE /api/player`, which replaces the cookie), re-fetch the overview and the game.
  - **The audio URL carries the section** and still wants a cache-busting key the server ignores (`/api/daily/{section}/audio?t=<attempts>-<status>`). Switching tabs has to stop the clip that is playing and load the other section's.
  - **`lib/game.svelte.ts` is one game.** The `game` singleton, `api.ts` (`getDaily`, `postGuess`, `audioUrl`) and `App.svelte` assume a single daily game; they need a section. One `Game` that is pointed at a section and reloads, or one per section, is the builder's choice. `audio.ts`, `clip.ts`, the record and the sky do not care.
  - **The legacy routes go with task 13**, once `api.ts` calls the per-section ones: in `server/src/routes.rs` delete `LEGACY_SECTION`, `legacy_daily`, `legacy_audio`, `legacy_guess` and their three `.route(…)` lines, and the test `the_routes_without_a_section_are_the_general_section`; then `/api/daily` answers 404 like any unknown route (a test should say so). `testutil::Player::skip()` / `guess()` already use `/api/daily/general/guess`. Remove their row from the API table.
  - **`web/mock/`** still mirrors the old single-game API (no sections, no stats, no overview, no admin). Bring it along or retire it; nothing imports it without `GTS_MOCK=1`.
  - **The admin page.** `GET /api/admin/state` makes the day's missing picks and can take seconds; the three POSTs answer with the new state, so the page never has to ask twice. A section with `status: "unavailable"` wants a "try again" (ask for the state again after ten seconds). Add a song: search with `/api/admin/search`, refuse a result whose `playable` is false in the page itself, then `POST /api/admin/songs`; the same POST with a `trackId` already in the pool is the retag. Reset to day 1 and re-roll delete players' games, so both want a confirmation (the Roadmap asks for one on reset). After any of them an open player page is stale until it reloads.
  - The frontend is plain Vite with no router, and Vite's dev server already answers unknown paths with `index.html`, so `/admin`, `/pop`, `/rock` and `/hip-hop` reach the app; a production build served by the Rust server will need the same fallback.
  - `GuessInput.svelte` knows nothing about the game (it takes `onguess` and `busy`) but calls `searchTracks` itself; the admin search needs the same control over `/api/admin/search`, with unplayable rows shown as such.
  - **The owner runs the servers.** The API on 4810 has been an old binary (started with `just server`, which does not reload) through tasks 10–12; nothing of the server work has been seen live. A UI task can only be checked in a browser once the owner has restarted it; do not start or restart it for him.
- For server work after that, what to know about the code as it stands:
  - The store. Handlers reach it through `AppState::store()` (a `&dyn Store`) and turn a `StoreError` into 500 `internal` after logging it (`routes::store_failed`). "Not there" is `None` or `false`, never an error. The trait's signatures are in the reference block below.
  - Adding to the store takes four steps: a method on the trait in `store/mod.rs` (a domain operation over plain types, atomic for one record); its implementation in `memory.rs` and in `sqlite.rs`; when it needs a table, a new numbered script in `store/sqlite/` appended to `MIGRATIONS` (never edit an existing script, because existing databases have already run it); and cases in `store/contract.rs`, with their names added to the list inside `contract_tests!`. `BrokenStore` in `testutil.rs` implements the trait too and needs each new method, and so do the wrapping stores in `routes::tests` (stores that yield or pause in the middle of an operation, for the tests of simultaneous requests).
  - The SQLite backend has one connection, so a trait method is atomic by being one closure passed to `SqliteStore::run`. `save_pick` is an `INSERT … ON CONFLICT DO NOTHING` followed by a read, as `add_song` is.
  - **The day** comes from `app.today()` (`Daily::today`: the clock's real date plus the stored offset), read once per request and passed down. `game.rs`, `stats.rs` and `pick.rs` have no clock on purpose. Tests pin the real date (`testutil::TODAY`, `harness.clock.set(day)`); nothing in a test should call `today_utc()`.
  - **A request and the song it uses.** `app.song(section, day)` can be slow (a pick, a download) and is called before any lock. What a handler then reads or writes about the games must happen while `Daily::stands(&playing)` is held, because a re-roll or reset replaces the song and deletes its games: lookers go through `routes::standing_song` (which fetches the new song and looks again), a move takes the player's move lock first, then `stands`, and answers 409 `changed` when the song is gone. The admin's changes take the write side and never the move lock, so there is no deadlock.
  - **Picks.** Made lazily by `Daily::song`, in `pick::PICK_ORDER`, stored only after the song loaded. `Section::ALL` is the tab order (General first), not the pick order. A song that leaves the pool keeps its picks. `Daily::reroll` only deletes (games first, then the pick) and remembers the replaced song in memory; the next `song` call draws.
  - **Game storage.** A game exists in the store only once a move has been made; "no game" is a fresh game, and handlers apply `.for_day(today)` to whatever comes back. Stats need no attention after a re-roll or reset, being derived.
  - `Track::is_playable()` is the readable-and-has-a-preview check. Every Deezer API call goes through `Deezer::get_json` and counts against the 40-per-5-s request budget, admin routes and picks included; a first-of-the-day pick of four uncached songs is four API calls and four downloads.
  - `game::GameState` serializes to JSON, and that JSON is what the SQLite backend stores (`games.state`). Changing its shape needs a migration or a reader that accepts both, because `decode_game` treats what it cannot read as "no game".
  - `rand` 0.10 makes the player IDs (`rand::random::<u128>()`) and the picks (`rand::rng()`, the `RngExt` trait; its names differ from older examples). A generator must not be alive across an `await`.
  - `MockTrack::preview(Preview::None)` makes the Deezer stand-in serve a track that is not readable and has no preview.
- The public API of the pure modules, of the store, of the player module and of the day's songs, for reference:

```rust
// mp3.rs — parse once per song, keep the `Mp3` in shared state, slice per request.
pub const PADDING_FRAMES: usize = 4;
pub enum Mp3Error { Empty, NoAudioFrames }                       // thiserror
impl Mp3 {                                                       // Clone is cheap (Bytes + Vec)
    pub fn parse(data: impl Into<bytes::Bytes>) -> Result<Mp3, Mp3Error>;
    pub fn prefix(&self, ms: u32) -> bytes::Bytes;               // the clip to send
    pub fn prefix_frames(&self, ms: u32) -> usize;
    pub fn duration_ms(&self) -> u32;
    pub fn frame_count(&self) -> usize;
    pub fn sample_rate(&self) -> u32;
    // #[cfg(test)] only: audio() (every frame, no tags), samples_per_frame()
}

// game.rs
pub const LADDER_MS: [u32; 7];  pub const MAX_ATTEMPTS: usize;  pub const FULL_CLIP_MS: u32;
pub fn ladder_seconds() -> [f64; 7];                             // 0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0
pub fn day_number(launch: jiff::civil::Date, today: jiff::civil::Date) -> i64;   // launch day = 1
pub enum Status { Playing, Won, Lost }                           // serde: "playing" | "won" | "lost"
pub enum Attempt { Skip, Wrong { title: String, artist: String } }   // serde: {"kind":"skip"} | {"kind":"wrong",…}
pub enum GameError { Finished }                                  // thiserror
pub struct GameState;                                            // Serialize + Deserialize, private fields
impl GameState {
    pub fn new(day: Date) -> GameState;
    pub fn for_day(self, today: Date) -> GameState;              // fresh game unless this one is for today
    pub fn day(&self) -> Date;
    pub fn attempts(&self) -> &[Attempt];
    pub fn status(&self) -> Status;
    pub fn is_finished(&self) -> bool;                           // gate for `answer` in the response
    pub fn unlocked_ms(&self) -> u32;                            // pass straight to Mp3::prefix
    pub fn skip(&mut self) -> Result<Status, GameError>;         // Ok(status after the move)
    pub fn guess(&mut self, answer: &TrackMeta, guessed: &TrackMeta) -> Result<Status, GameError>;
}
pub struct TrackMeta { pub title: String, pub title_short: String, pub artist: String }
impl TrackMeta {
    pub fn new(title: impl Into<String>, title_short: impl Into<String>, artist: impl Into<String>) -> TrackMeta;
    pub fn match_key(&self) -> (String, String);                 // de-duplicate search results on this
}
pub fn is_match(answer: &TrackMeta, guess: &TrackMeta) -> bool;
pub fn normalize_title(title: &str) -> String;
pub fn normalize_artist(artist: &str) -> String;

// store/mod.rs — the only way to persistent data. Re-exports MemoryStore, SqliteStore, seed_if_empty.
pub enum Genre { Pop, Rock, HipHop }                             // serde and slug(): "pop" | "rock" | "hip-hop"; Ord in this order
impl Genre { pub const ALL: [Genre; 3]; pub fn slug(self) -> &'static str; pub fn from_slug(slug: &str) -> Option<Genre>; }
pub type Genres = std::collections::BTreeSet<Genre>;
pub enum Section { General, Genre(Genre) }                       // serde and slug(): "general" | "pop" | "rock" | "hip-hop"
impl Section { pub const ALL: [Section; 4]; pub fn slug(self) -> &'static str; pub fn from_slug(slug: &str) -> Option<Section>; pub fn genre(self) -> Option<Genre>; }   // also From<Genre>
pub struct PlayerId;                                             // 32 lowercase hex characters; Ord + Hash; Display is the ID
impl PlayerId { pub fn generate() -> PlayerId; pub fn parse(text: &str) -> Option<PlayerId>; pub fn as_str(&self) -> &str; }
pub struct NewSong { pub track_id: u64, pub title: String, pub title_short: String, pub artist: String, pub album: String }
pub struct PoolSong { /* the NewSong fields, then */ pub genres: Genres, pub preview_failed_on: Option<Date> }
impl PoolSong { pub fn new(song: NewSong, genres: Genres) -> PoolSong; }   // no failed check on record
pub struct Pick { pub day: Date, pub section: Section, pub track_id: u64 }   // what a section plays on a day; Copy + Ord
pub struct StoreError;                                           // opaque; Display is a sentence for the log, never for a client
impl StoreError { pub fn new(what: impl Display, cause: impl Display) -> StoreError; }
#[async_trait]
pub trait Store: Send + Sync {
    async fn songs(&self) -> Result<Vec<PoolSong>, StoreError>;                  // by ascending track ID
    async fn song(&self, track_id: u64) -> Result<Option<PoolSong>, StoreError>;
    async fn add_song(&self, song: NewSong, genres: Genres) -> Result<PoolSong, StoreError>;   // already there: genres replaced, the rest kept
    async fn set_song_genres(&self, track_id: u64, genres: Genres) -> Result<Option<PoolSong>, StoreError>;   // None: no such song
    async fn remove_song(&self, track_id: u64) -> Result<bool, StoreError>;      // false: it was not there
    async fn set_preview_failed_on(&self, track_id: u64, day: Option<Date>) -> Result<bool, StoreError>;
    async fn game(&self, player: &PlayerId, section: Section, day: Date) -> Result<Option<GameState>, StoreError>;   // None: no move made there
    async fn save_game(&self, player: &PlayerId, section: Section, game: &GameState) -> Result<(), StoreError>;       // replaces the game of game.day(); last save wins
    async fn games(&self, player: &PlayerId, section: Section) -> Result<Vec<GameState>, StoreError>;                // finished or not, by ascending day
    async fn delete_player(&self, player: &PlayerId) -> Result<usize, StoreError>;   // every section and day; how many games went
    async fn delete_games(&self, section: Section, day: Date) -> Result<usize, StoreError>;   // every player's game there that day (a re-roll)
    async fn picks_on(&self, day: Date) -> Result<Vec<Pick>, StoreError>;        // at most one per section, in Section's order
    async fn pick_history(&self, section: Section) -> Result<Vec<Pick>, StoreError>;   // every day, ascending
    async fn save_pick(&self, pick: Pick) -> Result<Pick, StoreError>;           // unless that day and section have one; returns the pick that stands
    async fn remove_pick(&self, day: Date, section: Section) -> Result<bool, StoreError>;
    async fn day_offset(&self) -> Result<i64, StoreError>;                       // 0 until set
    async fn set_day_offset(&self, days: i64) -> Result<(), StoreError>;
    async fn wipe_games_and_picks(&self) -> Result<(), StoreError>;              // games first; the pool and the offset stay
}
pub fn open(kind: config::StoreKind, data_dir: &Path) -> Result<Arc<dyn Store>, StoreError>;
pub async fn seed_if_empty(store: &dyn Store) -> Result<usize, StoreError>;      // how many songs it added: 6 or 0

// stats.rs — pure; the record of one player in one section.
pub struct Stats { pub played: u32, pub won: u32, pub win_percent: u32, pub current_streak: u32, pub best_streak: u32, pub guess_distribution: [u32; 7] }   // Serialize, camelCase; Default is the empty record
impl Stats { pub fn as_of<'a>(today: Date, games: impl IntoIterator<Item = &'a GameState>) -> Stats; }   // any order; games after `today` are left out

// player.rs — the cookie, the move lock, Clear my data, the cookie key.
pub fn known_player(jar: &PrivateCookieJar) -> Option<PlayerId>;                 // None: no cookie, or not one of ours
pub fn remember(jar: PrivateCookieJar, player: &PlayerId, headers: &HeaderMap) -> PrivateCookieJar;   // sets the cookie for 400 days
impl MoveLocks { pub fn new() -> MoveLocks; pub async fn lock(&self, player: &PlayerId) -> tokio::sync::MutexGuard<'_, ()>; }
pub async fn clear(…) -> Result<Response, ApiError>;                             // DELETE /api/player
pub fn session_key(secret: Option<&str>, data_dir: &Path) -> anyhow::Result<Key>;

// pick.rs — pure; the daily pick.
pub const PICK_ORDER: [Section; 4];                              // Pop, Rock, Hip-hop, General
pub fn candidates(section: Section, day: Date, pool: &[PoolSong], taken: &BTreeSet<u64>) -> Vec<u64>;   // empty: no song that day
pub fn choose(section: Section, day: Date, pool: &[PoolSong], history: &[Pick], taken: &BTreeSet<u64>, replaced: Option<u64>, rng: &mut (impl Rng + ?Sized)) -> Option<u64>;

// daily.rs — the day, its picks and their songs.
pub struct Clock;                                                // Clock::utc(), Clock::new(|| date), real_day()
pub struct Days { pub real: Date, pub offset: i64, pub today: Date }
pub struct Song { pub meta: TrackMeta, pub answer: Answer, pub mp3: Mp3 }
pub struct Playing { pub day: Date, pub section: Section, pub track_id: u64, pub song: Arc<Song> }
pub enum DailyError { NoSong { .. }, Lookup { .. }, Unplayable(u64), Download { .. }, BadAudio { .. }, CoolingDown(Duration), Store(StoreError), Clock(i64) }   // for the log only
impl Daily {
    pub fn new(deezer: Deezer, store: Arc<dyn Store>, data_dir: &Path, clock: Clock) -> Daily;
    pub async fn days(&self) -> Result<Days, DailyError>;
    pub async fn today(&self) -> Result<Date, DailyError>;                       // read once per request
    pub async fn song(&self, day: Date, section: Section) -> Result<Playing, DailyError>;   // picks and loads if needed
    pub async fn warm(&self, day: Date);
    pub async fn stands(&self, playing: &Playing) -> Result<Option<Standing<'_>>, StoreError>;   // None: the song was replaced
    pub async fn reroll(&self, sections: &[Section]) -> Result<Date, DailyError>;   // deletes picks and games; the next song() draws
    pub async fn next_day(&self) -> Result<Days, DailyError>;
    pub async fn reset(&self, launch: Date) -> Result<Days, DailyError>;
}
```

  Rules the routes follow, which new routes must keep:
  - Serve only `Mp3::prefix` output, never the downloaded bytes: `parse` is what strips the tags, and a tag can carry the title.
  - Who is playing comes from `player::known_player(&jar)` and from nowhere else; a request without a usable cookie is a new player (`PlayerId::generate()`), and a response that shows the game sets the cookie with `player::remember`.
  - `today` is the server's day (`app.today()`), read once per request. The game, stats and pick modules have no clock on purpose.
  - The song comes from `app.song(section, today)`, before any lock; the games are read and written only while `Daily::stands` holds for that song.
  - Game in: `Store::game` or `Store::games`; nothing stored for today → `GameState::new(today)`; then always `.for_day(today)`. A visit stores nothing.
  - A move is read, applied and saved under `app.move_locks().lock(&player)` and then the standing guard, with nothing slow inside.
  - A `StoreError` goes through `routes::store_failed` and a `DailyError` through `routes::daily_failed`: logged, a fixed message, never the reason.
  - A guess's `TrackMeta` comes from Deezer by ID (`Deezer::track_meta`), never from the request body.

### Known issues

- **Tasks 10, 11 and 12 have not been run on a live server or against real Deezer**, only in tests (see task 12 in Progress for what to check after a restart).
- With the six seed songs every answer is one of six, each genre alternates its two, and a song that was Pop's answer yesterday can be General's today: "no repeat" is per section. A real pool fixes the first two; the third is by design.
- `GET /api/admin/state` has a side effect: it makes the day's missing picks, and so does every admin route that answers with the state. Opening the admin page after midnight is what draws the day's songs if no player came first.
- A re-roll remembers the song it replaced in memory only. If the server restarts between the re-roll and the next request for that section, the new pick can be the same song again.
- A pick that stands is never replaced behind the players' backs. If its song cannot be loaded any more on the same day (the disk cache is gone and Deezer has withdrawn the track), the section answers 502 until the owner re-rolls it; the log says so.
- General cannot be picked while a genre section is stuck on a Deezer failure: it waits behind it and answers 502 too, even if its own song would be in the disk cache.
- `GET /api/today` says `pending` from the pool alone, without checking previews, so a section it calls `pending` can still turn out to have no song when its game is asked for.
- Moving the clock forward with Simulate next day and then waiting for real midnight moves it again: the offset is added to the real date. Reset to day 1 is the way back.
- The legacy routes `/api/daily`, `/api/daily/audio` and `/api/daily/guess` exist only for the old client; task 13 removes them. `web/mock/` still mirrors the old single-game API.
- The standing lock (`Daily::stands`) and the pick lock work inside one server process, like the move lock.
- A player who was in the middle of a game when the server is updated from before task 11 starts that day again: the old `gts_daily` cookie is not read. The song is also a different one, since the picks replaced `track_id`.
- Games are never pruned. Every player who makes a move leaves one row per section and day for good, and a client that sends moves without keeping the cookie makes a new player, and a row, with every request. Nothing limits that yet; the rows are small (about 100 bytes for a game of skips, 2.7 kB at worst).
- The stats are worked out from all of a player's games in the section on every `GET /api/daily/{section}` and every move: one query and a pass over the list. Fine for years of daily play by one player; if it ever is not, store a summary per game rather than counters.
- The move lock (`player::MoveLocks`) works inside one server process, which is all there is. Two server processes over one database could let two simultaneous moves of a player share a try; that would need a conditional write in the store.
- Clear my data cannot revoke the old ID: a saved copy of the old cookie still works, as a player with no games. Likewise there is no way to move a player to another browser.
- Changing the cookie key (deleting `data/secret.key`, or a new `GTS_SECRET`) turns everyone into a new player and leaves their old games in the database, unreachable. `DELETE FROM games` clears them.
- `stats.winPercent` is a whole number; the client can compute a finer rate from `won` and `played`.
- A section's game needs its song, not only the player's game: with Deezer unreachable and the song not in `data/audio/`, `GET /api/daily/{section}` answers 502 `upstream` (as do the audio and guess routes) until a pick and load succeed. They are retried on a request at most every 10 s. Once the day's songs are in memory or on disk, Deezer being down only breaks search and guesses whose track is not in the metadata cache; skips keep working.
- The request budget (40 Deezer API calls per 5 s, shared by all players) turns searches beyond it into 502 `upstream` rather than queueing them. The client should treat a failed search as "no results for now", not as a fatal error.
- Search fetches 25 Deezer results and de-duplicates afterwards, so a query whose top 25 are mostly releases of one song returns fewer than 8 rows.
- Numbers that are whole serialize with a decimal point (`"clipSeconds":1.0`, `ladder` `[…,1.0,3.0,…]`). They are the same numbers to JavaScript.
- The track metadata cached in `data/audio/<id>.json` is never refreshed; delete the file to refetch.
- The tower-http `fs` / `compression-gzip` features are in `Cargo.toml` but unused until the production static-file serving exists.
- Playlists are no longer the plan for the daily pick (the song database is; see Roadmap), but they remain a possible source for a bulk import through the admin API. What was learnt: the pool is weaker than hoped. The original candidate `88551731` was dropped (37% playable). The two that were in `config.toml` until task 12 (`11535307124` "Rolling Stone: Best Songs of All Time", `5123717724` "Most Popular Songs of All Time: Greatest Hits") are 59% and 71% playable, and `5123717724` leans towards 2014–2018 chart pop and hip-hop with some obscure tracks. Better candidates were seen but only half-checked — first 100 tracks only, playlist metadata not fetched — so they are not in the config: `8499830842` "Party Hits & All-Time Classics" (1075 tracks, 90 of the first 100 playable), `1321696237` "80s HITS | TOP 100 SONGS" (139, 92/100), `1319830927` "90s HITS | TOP 100 SONGS" (146, 93/100), `1318937087` "2000s HITS Y2K THROWBACKS" (213, 85/100), `11153461484` "10s HITS - 100 Greatest Songs of the 2010s" (100, 97/100). Whoever revisits the pool should finish checking these rather than search again.
- The shortest clip is bigger than "a few hundred bytes": `prefix(100)` is 8 frames (4 for 100 ms + 4 padding), 3,343 bytes on the real preview, about 209 ms of audio. The padding is what makes the cut safe to decode; the client must trim to the exact duration, and a determined player can hear ~0.2 s instead of 0.1 s on the first turn. Accepted.
- A preview is 29.988 s, so on the last ladder step the client must clamp playback to the decoded buffer's length rather than assume 30 s.
- Matching drops every `(…)` and `[…]` segment, so "(Remix)" and "(Instrumental)" variants count as the same song as the original, and titles differing only in a bracketed part ("Da Doo Ron Ron (When He Walked Me Home)") lose it. Checked offline against the 2,710 distinct tracks cached during playlist research: 37 key collisions, all the same song in another release, no false merges.
- `jiff` is the date crate, with its `serde` feature on (`civil::Date` serializes as `"YYYY-MM-DD"`). `rand` is 0.10 and `reqwest` is 0.13, whose APIs and feature names differ from older examples (`rustls`, not `rustls-tls`; `query` is its own feature).
- reqwest's rustls backend builds `aws-lc-sys` and rusqlite's `bundled` feature builds SQLite, both C code, so the first server build takes one to two minutes.
- **The admin API is open.** `/api/admin/*` has no authentication, and since task 10 it exists: anyone who can reach port 4810, directly or through the Vite proxy or a reverse proxy's `/api/`, can read the day's answers, re-roll them, move the day for everyone, wipe every player's games and edit the pool. Keep the server on loopback until the routes are protected.
- `POST /api/admin/reroll` does not look at the content type and treats an empty body as "all four", so a stray POST re-rolls everything and deletes the day's games.
- Removing every song from the pool brings the six seed songs back at the next start: the rule is "the pool is empty", and nothing records that a pool was seeded before.
- A song's stored title, artist and album are never refreshed from Deezer, and a retag keeps them. Remove the song and add it again to refetch them.
- `POST /api/admin/songs` answers 200 for a new song and for a retag alike.
- The admin search shows at most the 25 tracks Deezer returns for the query; there is no paging.
- `songs.added_at` is in the SQLite schema (inherited from the hand-built database) but the server neither reads it nor exposes it; the pool is listed by artist and title, not by when a song was added.
- The SQLite backend cannot hold a track ID above `i64::MAX`: adding one is a store error (500), where the in-memory backend accepts it. Deezer's IDs are around 2³².
- Audio: the owner confirmed by ear on 2026-10-01, in his own browser, that the plain UI of task 4 worked, clips included. The design pass did not touch `audio.ts`, but nobody has listened since the restyle, and which browsers he used is not recorded: Firefox, Safari / iOS remain unverified. In headless Chromium the truncated clip decodes and an offline render gives the exact clip lengths and fades.
- iOS: the silent switch mutes Web Audio. No workaround is in place.
- The record's seven bands are equally wide, not a linear 30 s scale (the first three steps would share 3% of the width), so the needle crosses the early bands quickly and the last ones slowly.
- Editing the text after picking a song searches for "Title, Artist" minus the edit, which may find nothing; use the Clear search button (or Escape twice) to start over.
- `web/test/` covers only the pure helpers; the store, the components and the canvas have no automated tests in the repo (scratch DevTools-protocol scripts were run, not kept).
- The night sky (task 9) has never been seen in a browser. Its loop runs for as long as the tab is visible (30 fps at idle, drawing at most 280 small arcs), so it costs a little battery on phones even when nothing happens; Pause motion or reduced motion stops it. `.page` uses `overflow: clip` to keep the record's light from lengthening the page, which needs Safari 16+; older Safari would get some empty scroll below the footer. A win seen through a refresh from another tab also sets off the stardust.
- The designed UI has been seen only in headless Chromium 154. Unverified: Safari and Firefox rendering (conic gradients need Safari 16.1+; `svh` has a `vh` fallback), real on-screen keyboards (emulated with a short viewport only), canvas frame rate on a mid-range phone (the tonearm shadow and two full-canvas layer blits run every frame while a clip plays).
- The old weekday sleeves are retired. Shared palette/control contrast is documented above; decorative dividers intentionally remain subdued and must not become the sole boundary of an input.
- After the full clip stops in the finished state, the record takes up to about 4 s to coast to upright; frames run until it rests.
- A very long word in a title breaks without a hyphen (`overflow-wrap: anywhere`).
- In landscape on a phone with the keyboard up there is little room: the list is capped to the space left (two rows at worst) and scrolls.
- The latin font file is 131.5 kB because it carries the optical-size axis as well as weight and width; `wdth.css` (78 kB, no optical size) is the smaller option if that matters more than the display cut of the title.

## Later

Not planned yet: protecting `/admin` and `/api/admin/*` (required before any public deployment), a script or agent that fills the song database through the admin API (possibly from Deezer playlists), choosing a specific song for a section, an overall streak across sections, a random (non-daily) mode, a share-result grid, a Nix package and a production deployment (server serving `web/dist`, a reverse proxy pointing everything at 4810).
