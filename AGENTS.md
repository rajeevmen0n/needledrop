# Needledrop — briefing for agents

Read this before touching the repo. Update the **Progress** section as the last step of any task.

## What this is

Needledrop is a daily song-guessing game in the style of Heardle: you hear a sliver of a song, guess it, and each wrong guess or skip unlocks a longer clip. The music is Deezer's free 30-second previews. "Needledrop" is a working title and easy to rename; the repo and crate are called `guessthesong`.

Decisions already made, and why:

- **The server is authoritative.** The browser never sees the answer's Deezer ID, title or preview URL until the game is over. A client-only game would leak the answer through the network tab.
- **The server sends only the audio unlocked so far.** If the full preview reached the browser, anyone could play all 30 seconds. The server cuts the MP3 to the current clip length on every request.
- **Game state is an encrypted, HttpOnly cookie**, not an account. There are no users or database in the MVP. Accepted limit: clearing cookies or using a private window gives a fresh game.
- **Daily mode only**, one song per UTC day for everyone, drawn from curated Deezer playlists (`config.toml`). Curated lists keep the songs recognisable; random and genre modes come later.
- **Ladder: 0.1 / 0.3 / 1 / 3 / 8 / 16 / 30 seconds — seven attempts.** The first clips are deliberately tiny; that is the game.
- **A guess is correct when the normalized title and primary artist both match.** Remasters, live cuts and album variants of the same song have different Deezer IDs, so comparing IDs would reject right answers.
- **Guesses must be picked from autocomplete.** The client sends a track ID, and the server looks up the title and artist itself, so a client cannot forge them.
- **Rust server (axum), Svelte 5 + Vite client, Nix dev shell.** The machine has no global `cargo`, `rustc`, `pnpm` or `just`; the flake provides all of them.
- **TLS via rustls, no OpenSSL**, so the dev shell needs no system libraries.

## Layout and architecture

```
browser ─▶ Cloudflare ─▶ nginx gts.icyfire.dev ─┬─ /      ─▶ Vite dev server 127.0.0.1:4811 (Svelte SPA)
                                                └─ /api/  ─▶ axum server 127.0.0.1:4810 ─▶ api.deezer.com / preview CDN
                                                                 │
                                                                 └─ data/  (daily picks history, cached mp3, cookie key)
```

nginx terminates TLS, so both processes only ever see plain HTTP. Vite also proxies `/api` to 4810 itself, so `http://127.0.0.1:4811` works without nginx. Vite's HMR websocket is configured for `wss://gts.icyfire.dev:443`, so hot reload only works through the public URL. When the game ships as a production build, the server will serve `web/dist` and nginx will send everything to 4810.

```
flake.nix, flake.lock   dev shell: cargo, rustc, clippy, rustfmt, rust-analyzer, bacon, nodejs_24, pnpm, just
justfile                recipes (below)
bacon.toml              the `server` job `just dev` uses: restarts the server when server/src, Cargo.toml or config.toml change
config.toml             bind address, data dir, launch date, playlist IDs
server/                 Rust crate `guessthesong-server`
web/                    Vite + Svelte 5 + TypeScript SPA (plain Vite, not SvelteKit), pnpm
data/                   runtime state, git-ignored, created by the server
```

### Server modules (`server/src/`)

| File | Responsibility |
|---|---|
| `main.rs` | Config load, shared state, router, static file serving, tracing. |
| `config.rs` | `config.toml` + env: playlist IDs, bind address, data dir, launch date (for the day number). |
| `deezer.rs` | Typed client: `search_tracks`, `track`, `playlist_tracks` (paginated), `download_preview`; a small TTL cache so autocomplete stays under Deezer's rate limit. |
| `mp3.rs` | Pure. `Mp3::parse(bytes)` once per song: skip ID3v2, walk frame headers, keep only the audio frames (no tags, no Xing/Info frame, no partial last frame). `Mp3::prefix(ms)` → the leading frames covering the duration plus 4 padding frames (bit reservoir and encoder delay), as a zero-copy `Bytes`. |
| `daily.rs` | On the first request of a UTC day: load the playlist pool, drop unreadable or preview-less tracks and anything in the history, pick at random, persist to `data/daily.json`, fetch a fresh preview URL, cache the MP3 to `data/audio/<day>.mp3` and in memory. `GTS_DAILY_TRACK_ID` overrides the pick for testing. |
| `game.rs` | Pure logic: the ladder, `GameState` (the cookie payload) and its transitions, `day_number`, and `normalize_title` / `normalize_artist` (lowercase, strip diacritics, use `title_short`, drop bracketed / "feat." / "- remaster" suffixes, strip non-alphanumerics). Match = normalized title **and** primary artist equal (`is_match`, `TrackMeta::match_key`). It does not care how the track was chosen, so later modes can reuse it. |
| `routes.rs` | Handlers and the cookie session. A guess's metadata is resolved server-side from `trackId` (search cache, else `/track/{id}`). |

### API

| Route | Behaviour |
|---|---|
| `GET /api/health` | `{"ok":true}`. Exists today. |
| `GET /api/daily` | `{ day, number, ladder, attempts[], status, answer? }`. `answer` (title, artist, album, cover, Deezer link) is only present once status is `won` or `lost`. |
| `GET /api/daily/audio` | MP3 truncated to the clip length the cookie has unlocked (the full 30 s once finished). `Cache-Control: no-store`. |
| `GET /api/search?q=` | Proxied Deezer track search → `[{ id, title, artist, album, cover }]`, de-duplicated by normalized title + artist, TTL-cached in memory. |
| `POST /api/daily/guess` | `{ trackId }` or `{ skip: true }`. Appends an attempt and returns the same shape as `GET /api/daily`. |

Cookie contents: `game::GameState` as JSON — `{"day":"2026-10-01","attempts":[{"kind":"skip"},{"kind":"wrong","title":"…","artist":"…"}],"status":"playing"}` — encrypted with axum-extra's `PrivateCookieJar`. Stored titles and artists are cut to 80 characters and 160 bytes, so the worst case (seven wrong guesses) is under 2.7 kB of JSON. A cookie that fails to deserialize (it also rejects impossible states, such as `playing` with seven attempts) is treated as no cookie. The key comes from `GTS_SECRET`, else it is generated once into `data/secret.key`. Unlocked clip length = `ladder[attempts.len()]`. State for a previous day is discarded. The cookie is `Secure` when the request arrived over HTTPS, which the server learns from the `X-Forwarded-Proto` header. nginx sends it: the vhost uses `recommendedProxySettings`, which sets `X-Forwarded-Proto $scheme`.

### Frontend (`web/src/`)

Planned files; only `main.ts`, `App.svelte` and `lib/api.ts` exist so far.

- `lib/api.ts` — typed fetch wrappers.
- `lib/audio.ts` — Web Audio player: fetch the clip → `decodeAudioData` → `AudioBufferSourceNode` played for exactly the unlocked duration, 5 ms gain fades to avoid clicks, an `AnalyserNode` tap for the visual. Skips the MP3's leading priming silence (detected once, capped at 60 ms) so the 0.1 s clip is 0.1 s of actual sound.
- `lib/game.svelte.ts` — runes-based store: state from the server, play / skip / guess actions.
- `lib/components/Record.svelte` — the canvas hero (see Design direction).
- `lib/components/GuessInput.svelte` — ARIA combobox: 200 ms debounce, `AbortController`, keyboard navigation, rows with cover thumbnail + title + artist; submit only with a selected track.
- `lib/components/Attempts.svelte`, `Controls.svelte`, `Reveal.svelte`.
- `styles/tokens.css`, `styles/base.css`.

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
- **Public URL:** https://gts.icyfire.dev. Its nginx vhost lives in `../nix/hosts/mainframe/nixos/nginx.nix`. The owner runs `nixos-rebuild switch` himself. Edit the nix repo only when asked, and never commit there.
- **Before a commit, both must pass:** `nix develop -c just check` and `nix develop -c just test`.
- **Real-preview test fixture:** `mp3::tests::real_preview_fixture` checks the frame walker against `server/tests/fixtures/preview.mp3` when that file exists, and prints a note and passes when it does not. The file is copyrighted audio and git-ignored (`server/tests/fixtures/*.mp3`); never commit it. Any Deezer `preview` download works as the fixture. All other MP3 tests build synthetic streams.
- **This machine runs other services.** Ports 3000, 4173, 8080, 6969, 9091 and 17170 are taken, and one of those services is another Vite process. Stop only what you started, by PID; never `pkill -f vite` or similar.
- Environment variables: `GTS_BIND` (bind address), `RUST_LOG` (tracing filter, default `info,tower_http=debug`). Planned: `GTS_SECRET`, `GTS_DAILY_TRACK_ID`.

## Conventions

- Conventional-commit messages (`feat: …`, `fix: …`, `chore: …`). **No `Co-Authored-By` trailer.**
- One commit per completed task. No remote, no pushing.
- Build tasks are delegated to subagents. The orchestrating agent briefs each one, reviews the diff, runs the checks and commits; subagents do not commit.
- Every task ends by updating the Progress section below, so this file and the git history never disagree.
- The dev machine is a headless server. Nobody on it can hear audio, so anything about how a clip sounds (audible, click-free, right length) must be checked by the owner by ear at https://gts.icyfire.dev. Say so instead of claiming it works.
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
- **User playlists rot.** Old playlists point at track IDs that have since been withdrawn, so a large share of a playlist can be unplayable. Measured on 2026-10-01 from this server, which Deezer treats as US: `88551731` "All time hits" 51 of 139 playable (37%), `3564546242` "500 Greatest Hits of All Time" 193 of 494 (39%). Check a playlist's playable share before adding it.
- Playlist `title` is cut off at 50 characters by the API.
- Each track has a `rank` (Deezer popularity, higher = better known) that could filter out obscure tracks.

## Design direction

Working title **Needledrop** (a needle drop is literally playing a fragment of a record).

**Concept — an unlabelled record sliding out of its sleeve.** The page is a flat, fully saturated colour field, like a 60s jazz sleeve, not a dark app. One oversized black vinyl record is cropped by the viewport edge. Its surface carries **seven groove bands, one per clip length**; unlocked bands light up in the accent colour, and while a clip plays the record spins, the tonearm sweeps the unlocked band, and the grooves ripple with the live audio signal. The centre label is blank until the game ends — then the real album cover prints onto the label and the sleeve. That reveal is the one orchestrated motion moment; everything else is quiet.

- **Colour** — a different two-colour sleeve per weekday (7 curated field/accent pairs, each contrast-checked), so every day's game looks like a different pressing. Default pair:
  - Field `#2340E0` cobalt · Accent `#FF6A1F` tangerine · Paper `#EEF0FF` (text on field) · Ink `#0B1560` (text on paper) · Vinyl `#050505`
- **Type** — one family, Bricolage Grotesque variable (self-hosted via Fontsource): condensed heavy for the huge day number and song title, regular width for UI; tabular figures for clip times. No monospace, no all-caps labels.
- **Layout** — left-aligned, asymmetric:

```
┌──────────────────────────────────────────────┐
│        ╭──────╮   Needledrop        No. 12   │
│    ╭───┤      │                              │
│  ╭─┤   │  ◯   │   Play 0.3 seconds  ▶        │
│  │ │   │      │   ┌──────────────────────┐   │
│  ╰─┤   │      │   │ Search for a song…   │   │
│    ╰───┤      │   └──────────────────────┘   │
│        ╰──────╯   Guess      Skip to 1 s     │
│  record, cropped   1  skipped                │
│  off left edge     2  ✕ Queen, Under Pressure│
└──────────────────────────────────────────────┘
mobile: record cropped at top, controls stacked below
```

- **States** — wrong / skipped / correct are distinguished by shape and text, not colour alone. Visible focus rings; `prefers-reduced-motion` swaps the spin for a static progress arc.
- **Copy** — plain and literal: "Play 0.3 seconds", "Skip to 1 second", "Guess", "Listen on Deezer".

The design task loads the `frontend-design` skill and follows this direction, with screenshot self-critique passes (headless Chromium from nixpkgs).

## Progress

- [x] **0. Reverse proxy** — 2026-10-01. `gts.icyfire.dev` vhost added in the nix repo and switched by the owner; `https://gts.icyfire.dev/api/health` returns 200 while the dev servers run. The nix repo change is uncommitted there, by the owner's choice.
- [x] **1. Scaffold** — 2026-10-01. Flake dev shell, justfile, `config.toml`, compiling server skeleton with `GET /api/health`, Vite + Svelte placeholder that calls it, this file. `just check` and `just test` pass.
- [x] **2. Server core** — 2026-10-01. `mp3.rs` (frame walker, `Mp3::prefix`) and `game.rs` (ladder, `GameState`, normalization, matching), both pure, 68 unit tests. Nothing calls them yet. `just check` and `just test` pass.
- [ ] **3. Server I/O** — `deezer.rs`, `daily.rs`, `routes.rs`, config loading; curl walk-through passes.
- [ ] **4. Frontend plumbing** — API client, audio player, game store; unstyled but fully playable; dev servers left running for the owner.
- [ ] **5. Design pass** — tokens, record canvas, combobox, reveal sequence, responsive + reduced motion.
- [ ] **6. Verification + critique** — end-to-end checks, screenshots, fixes.

### Next up

- Task 3: `config.rs`, `deezer.rs`, `daily.rs`, `routes.rs`, wired into `main.rs`; then the curl walk-through from the plan. It builds on the two pure modules, whose public API is:

```rust
// mp3.rs — parse once per song, keep the `Mp3` in shared state, slice per request.
pub const PADDING_FRAMES: usize = 4;
pub enum Mp3Error { Empty, NoAudioFrames }                       // thiserror
impl Mp3 {                                                       // Clone is cheap (Bytes + Vec)
    pub fn parse(data: impl Into<bytes::Bytes>) -> Result<Mp3, Mp3Error>;
    pub fn prefix(&self, ms: u32) -> bytes::Bytes;               // the clip to send
    pub fn prefix_frames(&self, ms: u32) -> usize;
    pub fn audio(&self) -> bytes::Bytes;                         // every frame, no tags
    pub fn duration_ms(&self) -> u32;
    pub fn frame_count(&self) -> usize;
    pub fn sample_rate(&self) -> u32;
    pub fn samples_per_frame(&self) -> u32;
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
    pub fn for_day(self, today: Date) -> GameState;              // fresh game unless the cookie is for today
    pub fn day(&self) -> Date;
    pub fn attempts(&self) -> &[Attempt];
    pub fn status(&self) -> Status;
    pub fn is_finished(&self) -> bool;                           // gate for `answer` in the response
    pub fn turn(&self) -> usize;                                 // 1..=7
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
```

  What task 3 needs to know:
  - The audio handler is `mp3.prefix(state.unlocked_ms())` with `Content-Type: audio/mpeg` and `Cache-Control: no-store`. `Bytes` goes straight into an axum body. Serve only `prefix` output, never the downloaded bytes: `parse` is what strips the tags.
  - `Mp3::parse` accepts `Vec<u8>` (from `tokio::fs::read`) and `Bytes` (from reqwest). A download that is not an MP3 (an HTML error page, an expired-URL response) fails with `Mp3Error::NoAudioFrames`; treat that as a failed download and do not cache it.
  - Cookie in: `serde_json::from_str::<GameState>(…)`, any error → `GameState::new(today)`; then always `.for_day(today)`. Cookie out: `serde_json::to_string(&state)`. `attempts()` and `status()` serialize in the shape the API response wants, so the response struct can borrow them.
  - `today` is the UTC date: `jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date()`. The game module has no clock on purpose.
  - Build a `TrackMeta` for the answer and for the guessed track from Deezer's `title`, `title_short` and `artist.name`. `GameError::Finished` is the 409-style case (a move after the game ended).
  - Remove `#![allow(dead_code)]` from `mp3.rs` and `game.rs` once the routes use them, and delete whatever clippy then reports as unused rather than re-allowing it (candidates: `Mp3::samples_per_frame()`, `Mp3::audio()`, `GameState::turn()`).

### Known issues

- The server does not read `config.toml` yet; only `GTS_BIND` is honoured (task 3).
- The playlist pool is weaker than hoped. The plan's candidate `88551731` was dropped (37% playable). The two in `config.toml` are 59% and 71% playable, and `5123717724` leans towards 2014–2018 chart pop and hip-hop with some obscure tracks. Better candidates were seen but only half-checked — first 100 tracks only, playlist metadata not fetched — so they are not in the config: `8499830842` "Party Hits & All-Time Classics" (1075 tracks, 90 of the first 100 playable), `1321696237` "80s HITS | TOP 100 SONGS" (139, 92/100), `1319830927` "90s HITS | TOP 100 SONGS" (146, 93/100), `1318937087` "2000s HITS Y2K THROWBACKS" (213, 85/100), `11153461484` "10s HITS - 100 Greatest Songs of the 2010s" (100, 97/100). Whoever revisits the pool should finish checking these rather than search again.
- `mp3.rs` and `game.rs` start with `#![allow(dead_code)]` because nothing outside their tests calls them yet. Task 3 removes both lines once the routes use the modules.
- The shortest clip is bigger than "a few hundred bytes": `prefix(100)` is 8 frames (4 for 100 ms + 4 padding), 3,343 bytes on the real preview, about 209 ms of audio. The padding is what makes the cut safe to decode; the client must trim to the exact duration, and a determined player can hear ~0.2 s instead of 0.1 s on the first turn. Accepted.
- A preview is 29.988 s, so on the last ladder step the client must clamp playback to the decoded buffer's length rather than assume 30 s.
- Matching drops every `(…)` and `[…]` segment, so "(Remix)" and "(Instrumental)" variants count as the same song as the original, and titles differing only in a bracketed part ("Da Doo Ron Ron (When He Walked Me Home)") lose it. Checked offline against the 2,710 distinct tracks cached during playlist research: 37 key collisions, all the same song in another release, no false merges.
- `jiff` is the date crate, with its `serde` feature on (`civil::Date` serializes as `"YYYY-MM-DD"`). `rand` is 0.10 and `reqwest` is 0.13, whose APIs and feature names differ from older examples (`rustls`, not `rustls-tls`; `query` is its own feature).
- reqwest's rustls backend builds `aws-lc-sys` (C code), which makes the first server build take about a minute.
- No favicon yet (`index.html` uses an empty `data:` icon); the design pass adds one.

## Later

Out of MVP scope: random and genre modes, stats and streaks, a share-result grid, a Nix package and the production vhost (server serving `web/dist`, nginx pointing everything at 4810).
