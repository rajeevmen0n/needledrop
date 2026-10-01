# Needledrop

A daily song-guessing game in the style of Heardle. You hear the first 0.1 seconds of a song and guess it; every wrong guess or skip unlocks a longer clip (0.1 / 0.3 / 1 / 3 / 8 / 16 / 30 s, seven attempts). Music comes from Deezer's 30-second previews.

- `server/` — Rust (axum) API. It picks the daily song, cuts the clips and judges guesses, so the browser never learns the answer early.
- `web/` — Vite + Svelte 5 + TypeScript single-page app.

## Running it

Everything comes from the Nix dev shell; nothing needs to be installed globally.

```sh
nix develop          # or `direnv allow` once, if you use direnv
just dev             # server with auto-reload + Vite dev server; Ctrl-C stops both
```

| What | Where |
|---|---|
| Rust API | http://127.0.0.1:4810 (`ND_BIND` overrides it) |
| Vite dev server | http://127.0.0.1:4811 (proxies `/api` to the API) — open this one |

Both bind `127.0.0.1` only. To reach the game under a public hostname, put a TLS-terminating reverse proxy in front (`/` → 4811, `/api/` → 4810) and set `ND_PUBLIC_URL=https://needledrop.example` for both processes. Vite uses it to accept the public host and connect hot reload. `AGENTS.md` has the details.

Other recipes: `just server`, `just web`, `just test`, `just check`, `just build`, `just fmt`. Run `just` to list them.

Runtime settings use environment variables. `ND_BIND` defaults to `127.0.0.1:4810`, `ND_DATA_DIR` defaults to `data`, and `ND_PUBLIC_URL` is unset by default. The launch date is fixed at 2026-10-01 and the store is SQLite. `AGENTS.md` has the architecture, conventions and current progress.

For a deployment, set `ND_DATA_DIR=/var/lib/needledrop` in the server environment. The server stores cached previews, its cookie key, and `needledrop.db` there. Copy existing data before changing the directory to retain the song pool and game records. The renamed `nd_player` cookie gives existing browsers a fresh player ID on their first visit after this release.
