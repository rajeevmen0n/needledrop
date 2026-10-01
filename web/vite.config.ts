import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'

// The dev server is reached two ways: directly on localhost, and through nginx
// at https://gts.icyfire.dev, which terminates TLS and forwards `/` here and
// `/api/` to the Rust server.
//
// It answers every path it has no file for with index.html, which is what
// lets `/pop`, `/rock`, `/hip-hop` and `/admin` reach the app.
export default defineConfig({
  plugins: [svelte()],
  server: {
    host: '127.0.0.1',
    port: 4811,
    strictPort: true,
    allowedHosts: ['gts.icyfire.dev'],
    // The browser opens the HMR websocket against the public HTTPS origin.
    hmr: { protocol: 'wss', host: 'gts.icyfire.dev', clientPort: 443 },
    // Only used on plain localhost; nginx routes /api/ itself.
    proxy: { '/api': 'http://127.0.0.1:4810' },
  },
})
