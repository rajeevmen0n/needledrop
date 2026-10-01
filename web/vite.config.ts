import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'
import { publicServer, publicUrlFromEnv } from './dev-server.ts'

// A reverse proxy can serve / here and /api/ to the Rust server. Vite's
// public host and HMR socket follow the same ND_PUBLIC_URL as the server.
const apiBind = process.env.ND_BIND?.trim() || '127.0.0.1:4810'

export default defineConfig({
  plugins: [svelte()],
  server: {
    host: '127.0.0.1',
    port: 4811,
    strictPort: true,
    ...publicServer(publicUrlFromEnv(process.env.ND_PUBLIC_URL)),
    proxy: { '/api': `http://${apiBind}` },
  },
})
