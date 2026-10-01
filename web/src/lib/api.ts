// Typed fetch wrappers for the server's `/api` routes.
//
// The game state lives in an HttpOnly cookie the server sets. Same-origin
// `fetch` sends it by itself, so nothing here handles it.

export type Status = 'playing' | 'won' | 'lost'

export type Attempt = { kind: 'skip' } | { kind: 'wrong'; title: string; artist: string }

/** The song, sent only once the game is over. */
export interface Answer {
  title: string
  artist: string
  album: string
  /** Image URL. */
  cover: string
  /** deezer.com track URL. */
  link: string
}

export interface Daily {
  /** UTC date, "2026-10-01". */
  day: string
  /** 1 on launch day. */
  number: number
  /** Clip length in seconds for each turn. */
  ladder: number[]
  attempts: Attempt[]
  status: Status
  /** The clip length unlocked right now; the last ladder step once finished. */
  clipSeconds: number
  answer: Answer | null
}

/** One autocomplete result. */
export interface Track {
  id: number
  title: string
  artist: string
  album: string
  /** Small image URL. */
  cover: string
}

export type Move = { trackId: number } | { skip: true }

/**
 * A failed request. `code` is the server's `error` field (`bad_request`,
 * `unknown_track`, `finished`, `upstream`) or one made up here: `network` when
 * nothing answered, `bad_response` when the answer was not the expected JSON.
 * `message` is a sentence fit to show the player.
 */
export class ApiError extends Error {
  readonly code: string
  /** HTTP status, 0 when there was no response. */
  readonly status: number

  constructor(code: string, message: string, status: number) {
    super(message)
    this.name = 'ApiError'
    this.code = code
    this.status = status
  }
}

export function isAbort(err: unknown): boolean {
  return err instanceof DOMException && err.name === 'AbortError'
}

async function errorFrom(res: Response): Promise<ApiError> {
  try {
    const body: unknown = await res.json()
    if (typeof body === 'object' && body !== null && 'error' in body && 'message' in body) {
      const { error, message } = body
      if (typeof error === 'string' && typeof message === 'string') {
        return new ApiError(error, message, res.status)
      }
    }
  } catch {
    // Not JSON: a proxy's own error page (nginx or Vite), because the server behind it is down.
    return new ApiError('bad_response', "Couldn't reach the game server. Try again in a moment.", res.status)
  }
  return new ApiError('bad_response', 'The server had a problem. Try again in a moment.', res.status)
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response
  try {
    res = await fetch(path, init)
  } catch (err) {
    // A cancelled request is not a failure; let the caller see the abort.
    if (isAbort(err)) throw err
    throw new ApiError('network', "Couldn't reach the server. Check your connection and try again.", 0)
  }
  if (!res.ok) throw await errorFrom(res)
  try {
    return (await res.json()) as T
  } catch (err) {
    if (isAbort(err)) throw err
    throw new ApiError('bad_response', 'The server sent something unexpected. Try again.', res.status)
  }
}

/** Today's game as the cookie has it. A page reload restores the game from this alone. */
export function getDaily(): Promise<Daily> {
  return request<Daily>('/api/daily', { cache: 'no-store' })
}

/** Skip or guess. Resolves with the game after the move. */
export function postGuess(move: Move): Promise<Daily> {
  return request<Daily>('/api/daily/guess', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(move),
  })
}

/** Up to 8 songs matching the text; none for fewer than 2 characters. */
export function searchTracks(query: string, signal?: AbortSignal): Promise<Track[]> {
  return request<Track[]>(`/api/search?q=${encodeURIComponent(query)}`, { signal })
}

/**
 * Where the currently unlocked clip is. The audio changes after every move
 * while the path stays the same, so the query makes each state its own URL.
 */
export function audioUrl(daily: Daily): string {
  return `/api/daily/audio?t=${daily.attempts.length}-${daily.status}`
}
