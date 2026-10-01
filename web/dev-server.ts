// What the Vite dev server needs to know about the address the game is served
// under. Pure (text in, settings out), so `vite.config.ts` stays a few lines
// and the rules can be tested; it is type-checked with the config, without
// DOM types.
//
// The address is the server's `public_url` setting. It belongs in the local
// settings file, `config.local.toml`, which is git-ignored and wins over
// `config.toml`; the `GTS_PUBLIC_URL` variable wins over both. Nothing in the
// repo names a hostname.

/** The name of the setting, as the server's config files spell it. */
const KEY = 'public_url'

/**
 * The file of local settings that goes with the config file at `configPath`:
 * `config.toml` → `config.local.toml`, in the same directory. The extension
 * is replaced, as the server does it (`Path::with_extension("local.toml")`).
 * `configPath` uses `/` as its separator.
 */
export function localConfigPath(configPath: string): string {
  const slash = configPath.lastIndexOf('/')
  const name = configPath.slice(slash + 1)
  const dot = name.lastIndexOf('.')
  const stem = dot > 0 ? name.slice(0, dot) : name
  return `${configPath.slice(0, slash + 1)}${stem}.local.toml`
}

/**
 * What a config file says about the public address: the address, `null` for
 * a key that is there but blank (which says "none", and overrides a file
 * below it), or `undefined` when the file does not mention it.
 *
 * This reads one top-level string key and nothing else of the file, which is
 * all it needs; the Rust server is the one that parses the files properly and
 * rejects a bad address. A key inside a table is another setting, so reading
 * stops at the first table header.
 */
function keyIn(configText: string): string | null | undefined {
  for (const line of configText.split('\n')) {
    const text = line.trim()
    if (text.startsWith('[')) break
    const match = /^public_url\s*=\s*(?:"([^"]*)"|'([^']*)')\s*(?:#.*)?$/.exec(text)
    if (match) {
      const value = (match[1] ?? match[2] ?? '').trim()
      return value === '' ? null : value
    }
  }
  return undefined
}

/**
 * The public address to use: `fromEnv` (`GTS_PUBLIC_URL`) when it is set and
 * not blank, otherwise the `public_url` key of the first of `configTexts`
 * that has one. The texts are the config files from the most local down:
 * `config.local.toml`, then `config.toml`; a file that does not exist is
 * `undefined`. The result is `undefined` when nothing names an address, which
 * is a server that is only reached on loopback.
 */
export function publicUrlFrom(configTexts: readonly (string | undefined)[], fromEnv: string | undefined): string | undefined {
  if (fromEnv !== undefined && fromEnv.trim() !== '') return fromEnv.trim()
  for (const text of configTexts) {
    if (text === undefined) continue
    const found = keyIn(text)
    if (found !== undefined) return found ?? undefined
  }
  return undefined
}

/** The part of Vite's `server` options that depends on the public address. */
export type PublicServer = {
  /** Hosts, besides localhost, whose requests the dev server answers. */
  allowedHosts?: string[]
  /** Where the browser opens the hot-reload websocket. */
  hmr?: { protocol: 'ws' | 'wss'; host: string; clientPort: number }
}

/**
 * The dev server options for a game served under `publicUrl`.
 *
 * Without one there is nothing to add: Vite answers localhost and the
 * hot-reload websocket goes to wherever the page came from. With one, the dev
 * server sits behind a reverse proxy at that address, so it has to accept the
 * proxy's `Host` and tell the browser to open the websocket against the
 * public origin (`wss` on 443 for an https address) rather than against the
 * loopback port the proxy hides. Hot reload then only connects for a page
 * opened through that address.
 */
export function publicServer(publicUrl: string | undefined): PublicServer {
  if (publicUrl === undefined) return {}
  let url: URL
  try {
    url = new URL(publicUrl)
  } catch {
    throw new Error(`${KEY} is ${JSON.stringify(publicUrl)}: expected an address such as https://needledrop.example`)
  }
  if (url.protocol !== 'https:' && url.protocol !== 'http:') {
    throw new Error(`${KEY} is ${JSON.stringify(publicUrl)}: the scheme has to be https or http`)
  }
  const secure = url.protocol === 'https:'
  return {
    allowedHosts: [url.hostname],
    hmr: {
      protocol: secure ? 'wss' : 'ws',
      host: url.hostname,
      // `URL` leaves the port empty when it is the scheme's default.
      clientPort: url.port === '' ? (secure ? 443 : 80) : Number(url.port),
    },
  }
}
