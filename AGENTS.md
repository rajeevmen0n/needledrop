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
| `mp3.rs` | Skip ID3v2, walk frame headers, `prefix(bytes, seconds)` → a slice covering the duration plus about 4 padding frames (bit reservoir and encoder delay). |
| `daily.rs` | On the first request of a UTC day: load the playlist pool, drop unreadable or preview-less tracks and anything in the history, pick at random, persist to `data/daily.json`, fetch a fresh preview URL, cache the MP3 to `data/audio/<day>.mp3` and in memory. `GTS_DAILY_TRACK_ID` overrides the pick for testing. |
| `game.rs` | Pure logic: the ladder, state transitions, and `normalize()` (lowercase, strip diacritics, use `title_short`, drop bracketed / "feat." / "- remaster" suffixes, strip non-alphanumerics). Match = normalized title **and** primary artist equal. It does not care how the track was chosen, so later modes can reuse it. |
| `routes.rs` | Handlers and the cookie session. A guess's metadata is resolved server-side from `trackId` (search cache, else `/track/{id}`). |

### API

| Route | Behaviour |
|---|---|
| `GET /api/health` | `{"ok":true}`. Exists today. |
| `GET /api/daily` | `{ day, number, ladder, attempts[], status, answer? }`. `answer` (title, artist, album, cover, Deezer link) is only present once status is `won` or `lost`. |
| `GET /api/daily/audio` | MP3 truncated to the clip length the cookie has unlocked (the full 30 s once finished). `Cache-Control: no-store`. |
| `GET /api/search?q=` | Proxied Deezer track search → `[{ id, title, artist, album, cover }]`, de-duplicated by normalized title + artist, TTL-cached in memory. |
| `POST /api/daily/guess` | `{ trackId }` or `{ skip: true }`. Appends an attempt and returns the same shape as `GET /api/daily`. |

Cookie contents: `{ day, attempts: [skip | wrong{title, artist}], status }`, encrypted with axum-extra's `PrivateCookieJar`. The key comes from `GTS_SECRET`, else it is generated once into `data/secret.key`. Unlocked clip length = `ladder[attempts.len()]`. State for a previous day is discarded. The cookie is `Secure` when the request arrived over HTTPS, which the server learns from the `X-Forwarded-Proto` header. nginx sends it: the vhost uses `recommendedProxySettings`, which sets `X-Forwarded-Proto $scheme`.

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
- Previews are 30 s, 128 kbps CBR, 44.1 kHz MP3: a 10-byte ID3 header, then frames of about 418 bytes / 26 ms. A clip can be cut by slicing at frame boundaries; no transcoding or ffmpeg.
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
- [ ] **2. Server core** — `mp3.rs` and `game.rs` with unit tests.
- [ ] **3. Server I/O** — `deezer.rs`, `daily.rs`, `routes.rs`, config loading; curl walk-through passes.
- [ ] **4. Frontend plumbing** — API client, audio player, game store; unstyled but fully playable; dev servers left running for the owner.
- [ ] **5. Design pass** — tokens, record canvas, combobox, reveal sequence, responsive + reduced motion.
- [ ] **6. Verification + critique** — end-to-end checks, screenshots, fixes.

### Next up

- Task 2: `mp3.rs` (`prefix(bytes, seconds)`) and `game.rs` (ladder, state transitions, `normalize()`), both pure and unit-tested. Tests named in the plan: MP3 slicing on a real preview fixture (slice lengths, frame alignment), normalization cases (remaster, feat., diacritics, live versions), state transitions (win, lose on the 7th miss, day rollover).

### Known issues

- The server does not read `config.toml` yet; only `GTS_BIND` is honoured (task 3).
- The playlist pool is weaker than hoped. The plan's candidate `88551731` was dropped (37% playable). The two in `config.toml` are 59% and 71% playable, and `5123717724` leans towards 2014–2018 chart pop and hip-hop with some obscure tracks. Better candidates were seen but only half-checked — first 100 tracks only, playlist metadata not fetched — so they are not in the config: `8499830842` "Party Hits & All-Time Classics" (1075 tracks, 90 of the first 100 playable), `1321696237` "80s HITS | TOP 100 SONGS" (139, 92/100), `1319830927` "90s HITS | TOP 100 SONGS" (146, 93/100), `1318937087` "2000s HITS Y2K THROWBACKS" (213, 85/100), `11153461484` "10s HITS - 100 Greatest Songs of the 2010s" (100, 97/100). Whoever revisits the pool should finish checking these rather than search again.
- `jiff` is the date crate; its `serde` feature is not enabled yet. `rand` is 0.10 and `reqwest` is 0.13, whose APIs and feature names differ from older examples (`rustls`, not `rustls-tls`; `query` is its own feature).
- reqwest's rustls backend builds `aws-lc-sys` (C code), which makes the first server build take about a minute.
- No favicon yet (`index.html` uses an empty `data:` icon); the design pass adds one.

## Later

Out of MVP scope: random and genre modes, stats and streaks, a share-result grid, a Nix package and the production vhost (server serving `web/dist`, nginx pointing everything at 4810).
