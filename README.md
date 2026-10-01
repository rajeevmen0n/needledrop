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
| Rust API | http://127.0.0.1:4810 (`GTS_BIND` overrides it) |
| Vite dev server | http://127.0.0.1:4811 (proxies `/api` to the API) — open this one |

Both bind `127.0.0.1` only. To reach the game under a public hostname, put a TLS-terminating reverse proxy in front (`/` → 4811, `/api/` → 4810) and set `public_url` to that address in `config.local.toml` (git-ignored; a copy of `config.toml` with the blank `public_url` filled in) or in `GTS_PUBLIC_URL`. It is the only place the hostname is written, both servers read it, and it is never committed. `AGENTS.md` has the details.

Other recipes: `just server`, `just web`, `just test`, `just check`, `just build`, `just fmt`. Run `just` to list them.

Settings live in `config.toml`; one machine's own go in `config.local.toml` next to it, which wins and is git-ignored. `AGENTS.md` has the architecture, conventions and current progress.
