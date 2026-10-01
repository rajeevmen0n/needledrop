# Needledrop — briefing for agents

Read this before touching the repo. Update the **Progress** section as the last step of any task.

## What this is

Needledrop is a daily song-guessing game in the style of Heardle: you hear a sliver of a song, guess it, and each wrong guess or skip unlocks a longer clip. The music is Deezer's free 30-second previews. "Needledrop" is a working title and easy to rename; the repo and crate are called `guessthesong`.

Decisions already made, and why:

- **The server is authoritative.** The browser never sees the answer's Deezer ID, title or preview URL until the game is over. A client-only game would leak the answer through the network tab.
- **The server sends only the audio unlocked so far.** If the full preview reached the browser, anyone could play all 30 seconds. The server cuts the MP3 to the current clip length on every request.
- **Game state is an encrypted, HttpOnly cookie**, not an account. There are no users or database in the MVP. Accepted limit: clearing cookies or using a private window gives a fresh game.
- **Daily mode only**, one song per UTC day for everyone, drawn from curated Deezer playlists (`config.toml`). Curated lists keep the songs recognisable; random and genre modes come later. **Not built yet:** to get a playable UI sooner, the server currently plays one hard-coded track (`track_id` in `config.toml`) every day. The game is still keyed to the UTC date, so the cookie resets each day even though the song does not change.
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
                                                                 └─ data/  (cached mp3 + track JSON, cookie key)
```

nginx terminates TLS, so both processes only ever see plain HTTP. Vite also proxies `/api` to 4810 itself, so `http://127.0.0.1:4811` works without nginx. Vite's HMR websocket is configured for `wss://gts.icyfire.dev:443`, so hot reload only works through the public URL. When the game ships as a production build, the server will serve `web/dist` and nginx will send everything to 4810.

```
flake.nix, flake.lock   dev shell: cargo, rustc, clippy, rustfmt, rust-analyzer, bacon, nodejs_24, pnpm, just
justfile                recipes (below)
bacon.toml              the `server` job `just dev` uses: restarts the server when server/src, Cargo.toml or config.toml change
config.toml             bind address, data dir, launch date, the track to play, playlist IDs (not read yet)
server/                 Rust crate `guessthesong-server`
web/                    Vite + Svelte 5 + TypeScript SPA (plain Vite, not SvelteKit), pnpm
data/                   runtime state, git-ignored, created by the server
```

### Server modules (`server/src/`)

| File | Responsibility |
|---|---|
| `main.rs` | Tracing, config load, cookie key, shared state, router. Starts loading the song in the background so the first request does not wait for Deezer. No static file serving yet. |
| `config.rs` | `Config::load()`: `config.toml` (path from `GTS_CONFIG`) with `GTS_BIND`, `GTS_TRACK_ID` and `GTS_SECRET` on top. Fields `bind`, `data_dir`, `launch_date`, `track_id`, `secret`. `Config::from_sources(text, env_lookup)` is the pure, tested part. A bad config stops the server at startup with a message naming the key or variable. `playlists` is not parsed. |
| `deezer.rs` | `Deezer`, a typed client over one `reqwest::Client` (10 s timeout, user agent): `search_tracks(q, limit)`, `track(id)`, `track_meta(id)`, `download_preview(url)`. Reads Deezer's 200-with-error bodies (`DeezerError::NotFound` for code 800, `Api` for the rest). Search results are cached for 10 minutes (500 queries), and the title and artist of every track seen are kept (10,000 tracks, 24 h) so a guess picked from autocomplete needs no request. A request budget (40 per 5 s) refuses to call Deezer near its rate limit: `DeezerError::Throttled`. Nothing retries. No `playlist_tracks` yet. |
| `mp3.rs` | Pure. `Mp3::parse(bytes)` once per song: skip ID3v2, walk frame headers, keep only the audio frames (no tags, no Xing/Info frame, no partial last frame). `Mp3::prefix(ms)` → the leading frames covering the duration plus 4 padding frames (bit reservoir and encoder delay), as a zero-copy `Bytes`. |
| `daily.rs` | `Daily::song_for(day)` → `Arc<Song>` (`meta` for matching, `answer` for the reveal, the parsed `mp3`). Today every day is the configured track: `Daily::pick(day)` is the one place the real daily pick will replace. Loads lazily and keeps the song in memory: from `data/audio/<track_id>.mp3` + `<track_id>.json` when both are there and usable, otherwise `/track/{id}` then the preview download, parsed before it is cached (a body that is not an MP3 is a failed download and is not written). A failed load is logged and returned, and the next request after 10 s tries again. `today_utc()` lives here. Its error messages name the track, so they go to the log, never to a client. |
| `game.rs` | Pure logic: the ladder, `GameState` (the cookie payload) and its transitions, `day_number`, and `normalize_title` / `normalize_artist` (lowercase, strip diacritics, use `title_short`, drop bracketed / "feat." / "- remaster" suffixes, strip non-alphanumerics). Match = normalized title **and** primary artist equal (`is_match`, `TrackMeta::match_key`). It does not care how the track was chosen, so later modes can reuse it. |
| `routes.rs` | `AppState`, `router(state)`, the handlers, `ApiError`, the cookie session (`load_game` / `store_game`) and `session_key`. `DailyView::new` is the only place the answer is handed out. A guess's metadata is resolved server-side from `trackId` (`Deezer::track_meta`: the cache filled by searches, else `/track/{id}`). |
| `testutil.rs` | Test-only (`#[cfg(test)]`): `synthetic_mp3(frames)` and `MockDeezer`, a Deezer stand-in on a loopback port (`/track/{id}`, `/search/track`, preview downloads, a switch that makes it answer with the rate-limit error). Handler and loader tests run against it, so no test touches the network. |

### API

All JSON keys are camelCase. The frontend is built against exactly this; do not rename fields.

| Route | Behaviour |
|---|---|
| `GET /api/health` | `{"ok":true}`. |
| `GET /api/daily` | 200 `{ day, number, ladder, attempts, status, clipSeconds, answer }`. `day` is the UTC date, `number` counts from `launch_date` (1 on that day), `ladder` is `[0.1, 0.3, 1.0, 3.0, 8.0, 16.0, 30.0]`, `attempts` is `[{"kind":"skip"} \| {"kind":"wrong","title","artist"}]`, `status` is `playing` / `won` / `lost`, `clipSeconds` is the clip length unlocked now (30 once finished). `answer` is `null` while playing and `{ title, artist, album, cover, link }` once won or lost (`cover` is the 500 px album cover, falling back to smaller ones; `link` is the deezer.com track URL). Sets or refreshes the cookie. `Cache-Control: no-store`. |
| `GET /api/daily/audio` | 200 `audio/mpeg`: `mp3.prefix(unlocked_ms)`, frames only, with `Content-Length` and `Cache-Control: no-store`. No `Accept-Ranges`; a `Range` header and any query string are ignored. Does not set the cookie. Without a cookie it is the 0.1 s clip. |
| `GET /api/search?q=` | 200 `[{ id, title, artist, album, cover }]`, at most 8. `q` is trimmed (and cut at 100 characters); fewer than 2 characters → `[]` without calling Deezer. Otherwise 25 results are fetched from Deezer `/search/track`, tracks with a blank title dropped, de-duplicated by `TrackMeta::match_key()` keeping the first of each, then cut to 8. `title` is Deezer's full `title`, `cover` is `album.cover_small` (56 px, may be empty). |
| `POST /api/daily/guess` | JSON `{ "trackId": 123 }` or `{ "skip": true }` → 200 with the body of `GET /api/daily` after the move, and the cookie. A correct guess sets `won` and adds no attempt. |

Errors are `{ "error": "<code>", "message": "<sentence>" }`. The messages are fixed text, safe to show the player.

| Status | `error` | When |
|---|---|---|
| 400 | `bad_request` | Guess body that is not JSON, not `application/json`, or names both or neither of `trackId` / `skip`. No turn is used. |
| 404 | `unknown_track` | Deezer has no track with that `trackId`. No turn is used. |
| 404 | `not_found` | No such route. |
| 409 | `finished` | A move on a game that is already won or lost. Checked before anything is looked up. |
| 502 | `upstream` | Deezer failed or this server's request budget is used up (search, guess lookup), or the song could not be loaded (`/api/daily`, audio, guess). |
| 500 | `internal` | Should not happen. |

**Anti-leak rule:** while `status` is `playing`, nothing sent to the client may contain the answer's Deezer ID, title, artist, album, cover or preview URL — JSON, headers, cookie and error messages alike. `routes::tests` assert it on every playing-state response; keep those assertions when adding routes.

Cookie: named `gts_daily`, holding `game::GameState` as JSON — `{"day":"2026-10-01","attempts":[{"kind":"skip"},{"kind":"wrong","title":"…","artist":"…"}],"status":"playing"}` — encrypted with axum-extra's `PrivateCookieJar`. Attributes: `HttpOnly; SameSite=Lax; Path=/; Max-Age=172800` (2 days), plus `Secure` when the request has `X-Forwarded-Proto: https`. nginx sends that header: the vhost uses `recommendedProxySettings`, which sets `X-Forwarded-Proto $scheme`. On plain HTTP (the Vite dev server on localhost) the cookie is not `Secure`, or the browser would drop it. Stored titles and artists are cut to 80 characters and 160 bytes, so the worst case (seven wrong guesses) is under 2.7 kB of JSON; a test checks that the whole `Set-Cookie` header for that case stays under 4096 bytes (axum-extra percent-encodes the base64, which adds about 6%). A cookie that is missing, does not decrypt or does not deserialize (impossible states, such as `playing` with seven attempts, are rejected too) is a fresh game, and so is one for another day (`.for_day(today)` is always applied). Unlocked clip length = `ladder[attempts.len()]`.

Cookie key: 64 random bytes written as 128 hexadecimal characters. `GTS_SECRET` if set (`openssl rand -hex 64`), else `<data_dir>/secret.key`, generated on the first start with mode 0600 and reused after that, so cookies survive restarts. A malformed `GTS_SECRET` or key file stops the server at startup; delete the file to get a new key. A new key means every player's game for the day starts again.

### Frontend (`web/src/`)

The designed UI (task 5). The look is described under **Design direction**; this is where things live.

- `lib/api.ts` — types for the API (`Daily`, `Attempt`, `Answer`, `Track`, `Move`), `ApiError { code, message, status }` (`code` is the server's `error`, or `network` / `bad_response`), `getDaily`, `postGuess`, `searchTracks(query, signal)`, `audioUrl(daily)` (`?t=<attempts>-<status>`).
- `lib/clip.ts` — pure helpers, unit-tested: `findAudibleStart` (threshold 0.002, cap 60 ms), `playableSeconds`, `clipLabel` ("0.3 seconds"), `clipShort` ("0.3 s"), `ladderFraction` (equal-width ladder steps; the needle's position across the record's bands is exactly this).
- `lib/audio.ts` — `ClipPlayer`, framework-free: one lazy `AudioContext` created in a gesture (`unlock()`), `load(url)` (decodes on a 44.1 kHz `OfflineAudioContext`, so no gesture is needed to load), `play(seconds)` (`source.start(when, offset, duration)` 30 ms ahead, 5 ms gain fades, clamped to the buffer), `stop()`, `progress()`, `onstatechange`, `analyser`. Graph: source → clip gain → output gain → analyser → destination. It skips the MP3's leading priming silence so the 0.1 s clip is 0.1 s of sound.
- `lib/game.svelte.ts` — `Game` runes store and the `game` singleton: `daily`, `loadError`, `moveError`, `clipError`, `clipLoading`, `submitting`, `playing`, `notice`, `moves`; derived `status`, `finished`, `turn`, `lastTurn`, `clipSeconds`, `nextClipSeconds`, `totalSeconds`, `slots`; `load()`, `refresh()`, `play()`, `stop()`, `toggle()`, `skip()`, `guess(track)`, `progress()`. Reloads the clip whenever `audioUrl` or the day changes. A 409 `finished` re-fetches the state instead of showing an error. The design pass did not change this file, `api.ts`, `clip.ts` or `audio.ts`.
- `lib/sleeve.ts` — pure, unit-tested: `sleeveForDay(day)` (the UTC weekday of the game's `day`, 0 = Sunday) and `sleeveOverride(search)` (`?sleeve=0..6`).
- `lib/record.ts` — `RecordPainter`, framework-free canvas 2D drawing: `configure(geometry)`, `setPalette(palette)`, `invalidate()`, `paint(view)`. `paint` draws exactly what its `RecordView` says (lit amount per band, rotation, needle position, waveform, cover, printed fraction, progress arc); the only state it keeps is three cached layers (disc and grooves, sheen, blank label). All geometry constants (band radii, label, tonearm pivot and length) are at the top of the file.
- `lib/components/`
  - `Record.svelte` — owns the `<canvas>`, the animation state and the one `requestAnimationFrame` loop; reads the colours, the font and the layout switches from CSS custom properties.
  - `Play.svelte` — the play button (a round disc plus the label) and the one-line readout under the label; used while playing and in the reveal. Clips shorter than 3 s never show "Stop" (pressing again restarts them).
  - `GuessInput.svelte` — ARIA combobox; takes `onguess` and `busy`, knows nothing about the game. Places its list above or below the field from `window.visualViewport`.
  - `Skip.svelte` — "Skip to …" / "Give up".
  - `Attempts.svelte` — the seven tries, one row per band: clip length, a mark (SVG shape), words.
  - `Reveal.svelte` — result line, title in display type, artist, cover and album, Deezer link, `Play`.
- `App.svelte` — layout (header, stage with the record, game column), picks the sleeve and sets `data-sleeve` on `<html>` and the `theme-color` meta.
- `styles/tokens.css` (every colour, size and space: the seven sleeves, the type scale, the record's size and position per layout), `styles/base.css` (reset, `.button`, focus ring, `.error`, reduced motion). Components carry layout only.
- `index.html` — `viewport-fit=cover`, the SVG favicon (`public/favicon.svg`), and a one-line inline script that sets `data-sleeve` from the browser's UTC weekday before first paint (the app then corrects it from the game's day).
- Font: `@fontsource-variable/bricolage-grotesque` (dev dependency), `standard.css` — one variable file per subset with the weight, width and optical-size axes, bundled by Vite and served from this origin. No external font requests.
- `web/mock/` — a stand-in API for UI work without the server: `GTS_MOCK=1 GTS_MOCK_AUDIO=<mp3> pnpm dev` (from `web/`, in the dev shell). Not imported without the flag. One game per Vite process, no cookie; `POST /api/mock/reset` restarts it; the answer is "Paper Boats". Not re-run after the design pass.
- `web/test/` — `pnpm --dir web test` (`node --test`, no framework) covers `lib/clip.ts` and `lib/sleeve.ts` (14 tests).

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
- **Public URL:** https://gts.icyfire.dev. Its nginx vhost lives in `../nix/hosts/mainframe/nixos/nginx.nix`. The owner runs `nixos-rebuild switch` himself. Edit the nix repo only when asked, and never commit there.
- **Before a commit, both must pass:** `nix develop -c just check` and `nix develop -c just test`.
- **Real-preview test fixture:** `mp3::tests::real_preview_fixture` checks the frame walker against `server/tests/fixtures/preview.mp3` when that file exists, and prints a note and passes when it does not. The file is copyrighted audio and git-ignored (`server/tests/fixtures/*.mp3`); never commit it. Any Deezer `preview` download works as the fixture. All other MP3 tests build synthetic streams.
- **This machine runs other services.** Ports 3000, 4173, 8080, 6969, 9091 and 17170 are taken, and one of those services is another Vite process. Stop only what you started, by PID; never `pkill -f vite` or similar.
- Environment variables: `GTS_CONFIG` (config file path, default `config.toml` in the working directory), `GTS_BIND` (overrides `bind`), `GTS_TRACK_ID` (overrides `track_id`, the Deezer track being played), `GTS_SECRET` (cookie key, 128 hex characters; see the cookie notes above), `RUST_LOG` (tracing filter, default `info,tower_http=debug`). A variable that is set but blank counts as unset.
- **What the song is:** the server logs it at `info` on startup (`loaded the song from … title=… artist=…`). That log line is the only place to find the answer without playing.
- **Deezer cache on disk:** `data/audio/<track_id>.mp3` and `.json` are reused on every start, so restarts (bacon restarts the server on each source change) cost Deezer nothing. Delete the two files to fetch again.

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

Working title **Needledrop** (a needle drop is literally playing a fragment of a record). This section describes what is built; later UI work should stay in this language.

**Concept — an unlabelled record sliding out of its sleeve.** The page is one flat, fully saturated colour field, like a 1960s jazz sleeve, not a dark app. One oversized black record is cropped by the edge of the window (or, on a phone, hangs from under the header). The record is the only bold thing on the page and it is the progress display: **seven groove bands, one per clip length**, outermost first; unlocked bands are filled with the accent colour. Everything else is quiet type on the field.

Principles:

- **Two inks and the paper.** Field, accent, paper, ink, plus the black of the vinyl. No greys, no opacity-dimmed text, no gradients outside the record. Hierarchy comes from size, weight and width.
- **Shape language.** Paper things are rectangles with square corners (the guess field, the result list, buttons, the cover); record things are circles (the play disc, the marks in the tries list). Paper surfaces that float (the result list, the cover) get a hard offset ink shadow (`--lift`), never a soft one.
- **The accent means "unlocked / do this next"**: lit bands, the play disc, the enabled Guess button, the current try's dot, the correct mark. Accent on field is below 3:1 on two sleeves, so the accent never carries meaning on the field by itself: it always has ink text on it, a paper edge round it, or words beside it.
- **States are shapes and words**, never colour alone: skipped `»` "Skipped", wrong `✕` title "by" artist, correct `✓` in a filled disc, current a filled dot and "This try" in a heavier weight, unused a hollow ring. The highlighted row in the result list has an ink bar on its left; a disabled button is dashed.
- **Motion answers actions** (a band lights when it unlocks, the record turns while a clip sounds). The only orchestrated sequence is the reveal.
- **Copy is plain and literal**, sentence case: "Play 0.3 seconds", "Skip to 1 second", "Give up", "Guess", "Play all 30 seconds", "Listen on Deezer", "You got it on try 3 of 7.", "No tries left. The song was". A picked song reads "Title, Artist" in the field and "Title by Artist" in the tries.

### Sleeves

One two-colour sleeve per UTC weekday (`data-sleeve` on `<html>`, numbered like `Date.getUTCDay()`), so each day's game is a different pressing. The weekday comes from the game's `day`, not the local date. `?sleeve=0..6` previews any of them. Vinyl is `#050505`; figures on locked bands are `#9a9a9a` (7.24:1 on vinyl).

| # | Day | Field | Accent | Paper | Ink | paper / field | ink / paper | ink / accent | accent / vinyl | accent / field (not relied on) |
|---|---|---|---|---|---|---|---|---|---|---|
| 0 | Sunday: petrol, coral | `#00677f` | `#ff7a66` | `#e8fbff` | `#00303c` | 6.06 | 13.22 | 5.53 | 7.99 | 2.53 |
| 1 | Monday: signal red, sky | `#cc121c` | `#a9e2ff` | `#fff0ee` | `#4a0408` | 5.16 | 14.44 | 11.44 | 14.57 | 4.09 |
| 2 | Tuesday: violet, lemon | `#6420d4` | `#ffe03a` | `#f3edff` | `#24085a` | 6.81 | 14.61 | 12.71 | 15.50 | 5.92 |
| 3 | Wednesday: leaf green, pink | `#1b7a2b` | `#ffb3cf` | `#effbea` | `#06330f` | 5.09 | 13.21 | 8.47 | 12.23 | 3.26 |
| 4 | Thursday: cobalt, tangerine (the agreed default) | `#2340e0` | `#ff6a1f` | `#eef0ff` | `#0b1560` | 6.44 | 14.37 | 5.68 | 7.12 | 2.55 |
| 5 | Friday: magenta, lime | `#c4166e` | `#c6f53c` | `#fff0f7` | `#4a0428` | 5.17 | 14.23 | 12.34 | 16.04 | 4.48 |
| 6 | Saturday: burnt orange, mint | `#bb410a` | `#9ff5d0` | `#fff3ea` | `#461600` | 4.97 | 14.01 | 11.96 | 15.96 | 4.25 |

Rules a new or changed sleeve must meet (WCAG ratios, computed, not eyeballed): paper on field, ink on paper and ink on accent ≥ 4.5:1; accent on vinyl and paper on vinyl ≥ 3:1. Paper on vinyl is 17.8:1 or more on all seven. The focus ring is a 3 px paper outline with a 3 px ink gap, which shows on field, paper and vinyl on every sleeve; the guess field draws its ring inside itself in ink.

### Type

One family, **Bricolage Grotesque** variable, self-hosted (weight 200–800, width 75–100, optical size). Regular width for everything except the two display roles, which are condensed (width 75) and heavy (800): the day number in the header and the song title at the reveal. Tabular figures for clip times. No monospace, no all-caps, no letter-spaced labels.

| Token | Size | Used for |
|---|---|---|
| `--text-small` | 14 px | help line, readout, rules line |
| `--text-body` | 17 px | body, the guess field (never under 16 px, or iOS zooms on focus), tries |
| `--text-large` | 21 px | wordmark on phones, play label, result line |
| `--text-title` | 28 px | artist, wordmark in the side layout |
| `--text-number` | `clamp(2.75rem, 1.75rem + 4.4vw, 5.5rem)` | "No. 12" |
| `--text-display` | `clamp(3.25rem, 1rem + 12vw, 7rem)` | song title; titles over 22 characters are set at 66% |

Weights 400 / 600 / 800 only. Display line-height 0.88, body 1.4.

### Layout

Left-aligned and asymmetric. Two layouts, switched by `@media (min-width: 40rem) and (min-aspect-ratio: 6/5)` (the same query is in `tokens.css` and `App.svelte`):

```
side (desktop, tablets and phones in landscape)        stacked (phones, portrait tablets)
┌──────────────────────────────────────────────┐       ┌──────────────────────┐
│      ╭────╮ ◎       Needledrop       No. 12  │       │ Needledrop    No. 12 │
│   ╭──┤    │ ┃                                │       │╲ ╭──────────────╮  ╱ │
│ ╭─┤  │ ◯  │ ┃       (▶) Play 0.3 seconds     │       │ ╲│      ◯       │ ╱  │
│ │ │  │    │ ┛       [Search for a song][Guess]│      │  ╲╰────────────╯╱ ━◎ │
│ ╰─┤  │    │         [Skip to 1 second]       │       │ (▶) Play 0.3 seconds │
│   ╰──┤    │         0.1 s » Skipped          │       │ [Search…     ][Guess]│
│      ╰────╯         0.3 s ● This try         │       │ [Skip to 1 second]   │
│ record fixed, cropped left/top/bottom        │       │ 0.1 s » Skipped      │
└──────────────────────────────────────────────┘       └──────────────────────┘
```

- **Side:** the record is `position: fixed`, radius `min(56svh, 36vw)`, centre half a radius in from the left edge and at 54% of the height, so the label and the full ladder of bands are always visible; the game column (max 34rem) scrolls past it. Tonearm pivot above the record's right shoulder.
- **Stacked:** header row, then a stage of height `1.33 × radius` where the record (radius `min(52vw, 19rem, 28svh)`) hangs with its centre a third of a radius below the header, then the game. The tonearm is turned a quarter (pivot at the lower right) and the clip lengths are printed down the lower left, so both sit in the visible half. Sized so play, guess field, Guess and Skip are in the first screenful at 320×568 and up.
- The order is always play → guess → skip → tries → one line of rules. There is no intro paragraph, no heading above the tries, no step bar: the record and the tries list carry that.
- Phones: `svh` units (with a `vh` fallback through `--screen`), `viewport-fit=cover` with `env(safe-area-inset-*)` in the header, gutters and bottom padding, `theme-color` set to the sleeve's field, targets ≥ 44 px (main controls 52–56 px), `touch-action: manipulation` and no tap highlight or text selection on controls. The result list opens under the field when there are 232 px of room there in the **visual** viewport, otherwise on whichever side has more, and its height is capped to that room, so with the on-screen keyboard up both the field and the list stay visible.
- No layout shift: the header reserves the number's height while loading, the result list is absolutely positioned, the tries are always seven rows, the play button has a fixed height whatever its label, and the visible status line was removed (the live region is kept, visually hidden).

### The record (`lib/record.ts`, `Record.svelte`)

Canvas 2D on a square canvas 2.24 record-radii wide (room for the tonearm). The backing store uses the device pixel ratio capped at 2 and at 1800 px a side. Geometry, as fractions of the radius: bands from 0.955 in to 0.385 in seven equal steps with a 0.01 smooth gap between them, label 0.30, spindle hole 0.022 (it shows the field colour: the hole goes through to the sleeve).

Drawn back to front each frame:

1. **Disc layer (cached):** soft ink shadow on the sleeve, black disc, about 250 hairline grooves of seeded-random brightness inside the bands, a bright rim, one scribed ring in the run-out.
2. **Lit bands:** each unlocked band is an accent ring (alpha = its lit amount, so unlocking fades in over 0.32 s, 0.09 s apart when several light) with 3–8 ink groove lines. While a clip plays those lines are polylines pushed in and out by the analyser's time-domain samples (`getFloatTimeDomainData`), tapered to zero at a seam on the cropped side.
3. **Scale:** the clip length of each band ("0.1" … "30") printed along one radius (`--record-scale-angle`), ink on lit bands, grey on locked ones. It does not turn.
4. **Sheen layer (cached):** two opposite conic-gradient wedges of white, carved into hairlines by `destination-out` strokes where the grooves are (the smooth gaps and the run-out keep the full highlight), plus two soft dark wedges a quarter turn away. The light is fixed; the layer rocks ±0.03 rad with the rotation, standing in for the warp of a real disc. Browsers without `createConicGradient` get a matt record.
5. **Label:** a cached blank paper label with a pressed ring and the game's date stamped under the hole ("1 Oct 2026", like a test pressing), rotated with the record. Once the game is over, the album cover is drawn over it, clipped to the label (this taints the canvas; nothing reads pixels back).
6. **Tonearm:** paper tube, counterweight, bearing, headshell, ink cartridge and an accent stylus point, with one blurred shadow. The arm swings about its pivot so the stylus sits on the groove at the needle's radius.

What drives it (`Record.svelte`): `game.clipSeconds` and `game.ladder` → which bands are lit; `game.playing` → spin (33⅓ rpm, eased in over about 0.12 s); `game.progress().elapsed` through `ladderFraction` → the needle, so it crosses band *n* during the *n*-th stretch of the clip, on the audio clock; `game.player.analyser` → the ripple; `game.answer.cover` → the label. At rest the needle sits on the lead-in groove. A blank record coasts to a stop anywhere; a record with a cover slows evenly and always stops upright. `requestAnimationFrame` runs only while something moves (playing, coasting, a band lighting, the needle returning, the reveal) and is cancelled when the tab is hidden; an idle page requests no frames.

### The reveal

The one orchestrated moment, run only for the player who just made the last move (`game.moves > 0`); a reload shows the settled state at once.

1. The remaining bands light, outermost first, 0.09 s apart, and the record spins up.
2. **The cover prints onto the label over exactly one turn**: the label turns under a fixed print head, and the part that has passed it shows the cover (a growing sector). If the image is slow, the record keeps turning for up to 6 s.
3. In the column the lines are uncovered left to right (`clip-path`, 140 ms steps): result line at once, then the title in display type, the artist, the cover and album ("the sleeve"), then the play button.
4. The record coasts and stops with the cover upright (up to about 4 s).

### Reduced motion

`prefers-reduced-motion: reduce`: the record never turns, the needle stays on the lead-in, grooves do not ripple, bands and the cover appear at once, the CSS reveal has no duration or delay. While a clip plays, a paper progress ring round the label fills on the audio clock; that is the only thing that moves.

## Progress

- [x] **0. Reverse proxy** — 2026-10-01. `gts.icyfire.dev` vhost added in the nix repo and switched by the owner; `https://gts.icyfire.dev/api/health` returns 200 while the dev servers run. The nix repo change is uncommitted there, by the owner's choice.
- [x] **1. Scaffold** — 2026-10-01. Flake dev shell, justfile, `config.toml`, compiling server skeleton with `GET /api/health`, Vite + Svelte placeholder that calls it, this file. `just check` and `just test` pass.
- [x] **2. Server core** — 2026-10-01. `mp3.rs` (frame walker, `Mp3::prefix`) and `game.rs` (ladder, `GameState`, normalization, matching), both pure, 68 unit tests. Nothing calls them yet. `just check` and `just test` pass.
- [x] **3. Server I/O** — 2026-10-01. `config.rs`, `deezer.rs`, `daily.rs`, `routes.rs` and `testutil.rs`, wired in `main.rs`; the API table above is what was built. Shipped with **one hard-coded track** (`track_id` in `config.toml`, `GTS_TRACK_ID`); the playlist-based daily pick is deferred to its own item below. 136 unit tests (handler tests run against a local Deezer stand-in); the curl walk-through against real Deezer passed (fresh game, seven clip sizes, wrong guess, win, loss, 409 after the end, no answer in any playing-state response). `just check` and `just test` pass.
- [x] **4. Frontend plumbing** — 2026-10-01. API client, Web Audio clip player, runes game store, plain components (controls, timeline, autocomplete, tries, reveal), a flag-gated mock API and 11 unit tests for the pure helpers. Built in parallel with task 3, then run together: a full game (play, skip, wrong guess, reload, right guess, reveal) passes in headless Chromium through `https://gts.icyfire.dev` with no console errors, HMR websocket included. Not yet confirmed by ear.
- [x] **5. Design pass** — 2026-10-01. Before it started, the owner tested the plain UI of task 4 in his browser and confirmed it "works perfectly", audio included. Then: seven weekday sleeves (contrast-checked, `?sleeve=0..6`), self-hosted Bricolage Grotesque, the record canvas (`lib/record.ts`, `Record.svelte`), side and stacked layouts with the phone as a first-class target, result list placed by the visual viewport, reveal sequence, reduced-motion mode, SVG favicon. Logic files untouched. Checked in headless Chromium against the real server at 320, 360, 390 and 430 px portrait, 844×390, 768×1024, 1024×768, 1440×900 and 1920×1080: a scripted game (play, skip, keyboard-only wrong guess, reload, right guess, reveal, full clip; a lost game; a reduced-motion game) passes with no console errors or warnings and no frames requested while idle. `just check` and `pnpm --dir web test` pass. Production bundle: JS 74.4 kB (28.3 kB gzip), CSS 13.4 kB (3.7 kB gzip), font 131.5 kB (latin; latin-ext 53.6 kB and vietnamese 22.0 kB load only when needed). **Not checked on a real phone or by ear** since the restyle.
- [ ] **6. Verification + critique** — end-to-end checks, screenshots, fixes.
- [ ] **Daily pick from playlists** — replace the hard-coded track: load the playlist pool from `config.toml`, drop unplayable tracks and past picks, pick one per UTC day, keep a history.

### Next up

- The owner looks at the designed UI at https://gts.icyfire.dev on a real phone and a desktop: the record while a long clip plays (spin, needle, ripple), the reveal, the result list with the real on-screen keyboard, iOS Safari audio on the first tap, all seven sleeves (`?sleeve=0` … `?sleeve=6`).
- Task 6 (verification + critique) picks up what he finds.
- Daily pick from playlists, when it is taken up. What it needs to know:
  - `Daily::pick(day) -> u64` in `daily.rs` is the only thing that decides the track. Everything else (`song_for`, the disk cache keyed by track ID, the routes) already works per day and per track; `song_for` reloads when the picked ID changes.
  - `pick` is synchronous and infallible today. A real pick needs the pool (network) and a history file, so it will become async and fallible; `song_for` already holds a lock for the whole load and already has the fail / pause 10 s / retry path to hang that on.
  - `Deezer` has no `playlist_tracks` yet. `config.rs` does not parse `playlists`; add the field when something reads it. `Track::is_playable()` is the readable-and-has-a-preview check.
  - Playlist pages go through `Deezer::get_json`, so they count against the 40-per-5-s request budget; a 500-track playlist is 5 requests.
  - `rand` 0.10 is already a dependency (unused until then).
  - Old `data/audio/<id>.mp3` files are never deleted; with a new song every day that needs a clean-up.
- The two pure modules' public API, for reference:

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
    pub fn for_day(self, today: Date) -> GameState;              // fresh game unless the cookie is for today
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
```

  Rules the routes follow, which new routes must keep:
  - Serve only `Mp3::prefix` output, never the downloaded bytes: `parse` is what strips the tags, and a tag can carry the title.
  - Cookie in: `serde_json::from_str::<GameState>(…)`, any error → `GameState::new(today)`; then always `.for_day(today)`.
  - `today` is the UTC date (`daily::today_utc()`), read once per request. The game module has no clock on purpose.
  - A guess's `TrackMeta` comes from Deezer by ID (`Deezer::track_meta`), never from the request body.

### Known issues

- **The song never changes.** Every day plays `track_id` from `config.toml` until the daily pick is built, so anyone who has finished one game knows every later answer. The day number and the daily cookie reset work as designed.
- `GET /api/daily` needs the song, not only the cookie: with Deezer unreachable and nothing in `data/audio/`, it answers 502 `upstream` (as do the audio and guess routes) until a load succeeds. Loads are retried on a request at most every 10 s. Once the song is in memory or on disk, Deezer being down only breaks search and guesses whose track is not in the metadata cache; skips keep working.
- The request budget (40 Deezer API calls per 5 s, shared by all players) turns searches beyond it into 502 `upstream` rather than queueing them. The client should treat a failed search as "no results for now", not as a fatal error.
- Search fetches 25 Deezer results and de-duplicates afterwards, so a query whose top 25 are mostly releases of one song returns fewer than 8 rows.
- Numbers that are whole serialize with a decimal point (`"clipSeconds":1.0`, `ladder` `[…,1.0,3.0,…]`). They are the same numbers to JavaScript.
- The track metadata cached in `data/audio/<id>.json` is never refreshed; delete the file to refetch.
- `rand` and the tower-http `fs` / `compression-gzip` features are in `Cargo.toml` but unused until the daily pick and the production static-file serving exist.
- The playlist pool is weaker than hoped. The plan's candidate `88551731` was dropped (37% playable). The two in `config.toml` are 59% and 71% playable, and `5123717724` leans towards 2014–2018 chart pop and hip-hop with some obscure tracks. Better candidates were seen but only half-checked — first 100 tracks only, playlist metadata not fetched — so they are not in the config: `8499830842` "Party Hits & All-Time Classics" (1075 tracks, 90 of the first 100 playable), `1321696237` "80s HITS | TOP 100 SONGS" (139, 92/100), `1319830927` "90s HITS | TOP 100 SONGS" (146, 93/100), `1318937087` "2000s HITS Y2K THROWBACKS" (213, 85/100), `11153461484` "10s HITS - 100 Greatest Songs of the 2010s" (100, 97/100). Whoever revisits the pool should finish checking these rather than search again.
- The shortest clip is bigger than "a few hundred bytes": `prefix(100)` is 8 frames (4 for 100 ms + 4 padding), 3,343 bytes on the real preview, about 209 ms of audio. The padding is what makes the cut safe to decode; the client must trim to the exact duration, and a determined player can hear ~0.2 s instead of 0.1 s on the first turn. Accepted.
- A preview is 29.988 s, so on the last ladder step the client must clamp playback to the decoded buffer's length rather than assume 30 s.
- Matching drops every `(…)` and `[…]` segment, so "(Remix)" and "(Instrumental)" variants count as the same song as the original, and titles differing only in a bracketed part ("Da Doo Ron Ron (When He Walked Me Home)") lose it. Checked offline against the 2,710 distinct tracks cached during playlist research: 37 key collisions, all the same song in another release, no false merges.
- `jiff` is the date crate, with its `serde` feature on (`civil::Date` serializes as `"YYYY-MM-DD"`). `rand` is 0.10 and `reqwest` is 0.13, whose APIs and feature names differ from older examples (`rustls`, not `rustls-tls`; `query` is its own feature).
- reqwest's rustls backend builds `aws-lc-sys` (C code), which makes the first server build take about a minute.
- Audio: the owner confirmed by ear on 2026-10-01, in his own browser, that the plain UI of task 4 worked, clips included. The design pass did not touch `audio.ts`, but nobody has listened since the restyle, and which browsers he used is not recorded: Firefox, Safari / iOS remain unverified. In headless Chromium the truncated clip decodes and an offline render gives the exact clip lengths and fades.
- iOS: the silent switch mutes Web Audio. No workaround is in place.
- The record's seven bands are equally wide, not a linear 30 s scale (the first three steps would share 3% of the width), so the needle crosses the early bands quickly and the last ones slowly.
- Editing the text after picking a song searches for "Title, Artist" minus the edit, which may find nothing; clear the field (Escape twice) to start over.
- `web/test/` covers only the pure helpers; the store, the components and the canvas have no automated tests in the repo (scratch DevTools-protocol scripts were run, not kept).
- The designed UI has been seen only in headless Chromium 154. Unverified: Safari and Firefox rendering (conic gradients need Safari 16.1+; `svh` has a `vh` fallback), real on-screen keyboards (emulated with a short viewport only), canvas frame rate on a mid-range phone (the tonearm shadow and two full-canvas layer blits run every frame while a clip plays).
- Accent on field is 2.53:1 (Sunday) and 2.55:1 (Thursday, the agreed default pair). Nothing depends on it — the play disc has a paper edge and an ink glyph, the current-try dot has a paper edge and words — but keep it that way when adding accent-coloured elements.
- After the full clip stops in the finished state, the record takes up to about 4 s to coast to upright; frames run until it rests.
- A very long word in a title breaks without a hyphen (`overflow-wrap: anywhere`).
- In landscape on a phone with the keyboard up there is little room: the list is capped to the space left (two rows at worst) and scrolls.
- The latin font file is 131.5 kB because it carries the optical-size axis as well as weight and width; `wdth.css` (78 kB, no optical size) is the smaller option if that matters more than the display cut of the title.

## Later

Out of MVP scope: random and genre modes, stats and streaks, a share-result grid, a Nix package and the production vhost (server serving `web/dist`, nginx pointing everything at 4810).
