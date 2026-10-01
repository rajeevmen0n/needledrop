// Vite's public origin comes from the same ND_PUBLIC_URL used by the server.

/** A blank or unset value means the dev server is reached on localhost. */
export function publicUrlFromEnv(value: string | undefined): string | undefined {
  return value?.trim() || undefined
}

/** The part of Vite's server options that depends on the public address. */
export type PublicServer = {
  allowedHosts?: string[]
  hmr?: { protocol: 'ws' | 'wss'; host: string; clientPort: number }
}

export function publicServer(publicUrl: string | undefined): PublicServer {
  if (publicUrl === undefined) return {}
  let url: URL
  try {
    url = new URL(publicUrl)
  } catch {
    throw new Error(`ND_PUBLIC_URL is ${JSON.stringify(publicUrl)}: expected an address such as https://needledrop.example`)
  }
  if (url.protocol !== 'https:' && url.protocol !== 'http:') {
    throw new Error(`ND_PUBLIC_URL is ${JSON.stringify(publicUrl)}: the scheme has to be https or http`)
  }
  if (url.username || url.password || url.pathname !== '/' || url.search || url.hash) {
    throw new Error(`ND_PUBLIC_URL is ${JSON.stringify(publicUrl)}: expected an origin without credentials, path, query or fragment`)
  }
  const secure = url.protocol === 'https:'
  return {
    allowedHosts: [url.hostname],
    hmr: {
      protocol: secure ? 'wss' : 'ws',
      host: url.hostname,
      clientPort: url.port === '' ? (secure ? 443 : 80) : Number(url.port),
    },
  }
}
