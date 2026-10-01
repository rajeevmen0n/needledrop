import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'
import { localConfigPath, publicServer, publicUrlFrom } from './dev-server.ts'

// The dev server is reached two ways: directly on localhost, and, when the
// game has a public address, through a reverse proxy there that terminates
// TLS and forwards `/` here and `/api/` to the Rust server. That address is
// the server's `public_url` setting, read from where the Rust server reads
// it: `GTS_PUBLIC_URL`, else `config.local.toml` (git-ignored, this machine
// only), else `config.toml`. No hostname is written in the repo.
//
// The dev server answers every path it has no file for with index.html, which
// is what lets `/pop`, `/rock`, `/hip-hop` and `/admin` reach the app.

/** The text of a config file, or `undefined` if it is not there to read. */
function read(path: string): string | undefined {
  try {
    return readFileSync(path, 'utf8')
  } catch {
    return undefined
  }
}

// The server resolves the path against its working directory, the repo root.
const configPath = resolve(import.meta.dirname, '..', process.env.GTS_CONFIG?.trim() || 'config.toml').replaceAll('\\', '/')
const publicUrl = publicUrlFrom([read(localConfigPath(configPath)), read(configPath)], process.env.GTS_PUBLIC_URL)

export default defineConfig({
  plugins: [svelte()],
  server: {
    host: '127.0.0.1',
    port: 4811,
    strictPort: true,
    ...publicServer(publicUrl),
    // Only used on plain localhost; a reverse proxy routes /api/ itself.
    proxy: { '/api': 'http://127.0.0.1:4810' },
  },
})
