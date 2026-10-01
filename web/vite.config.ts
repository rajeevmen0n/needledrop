import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'
import type { PluginOption } from 'vite'

// GTS_MOCK=1 answers /api from web/mock instead of the Rust server, for
// working on the UI alone. Without the flag the mock is not even imported.
async function mockPlugins(): Promise<PluginOption[]> {
  if (process.env.GTS_MOCK !== '1') return []
  const { mockApi } = await import('./mock/api.ts')
  return [mockApi()]
}

// The dev server is reached two ways: directly on localhost, and through nginx
// at https://gts.icyfire.dev, which terminates TLS and forwards `/` here and
// `/api/` to the Rust server.
export default defineConfig(async () => ({
  plugins: [svelte(), ...(await mockPlugins())],
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
}))
