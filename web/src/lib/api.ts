// Typed fetch wrappers for the server's `/api` routes.
//
// Who is playing is an HttpOnly cookie the server sets. Same-origin `fetch`
// sends it by itself, so nothing here handles it.

import type { Section, TabInfo } from './sections'
import type { Stats } from './stats'

export type { Section, Stats }

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

/** One section's game today. */
export interface Daily {
  /** The server's day, "2026-10-01". */
  day: string
  /** 1 on launch day. */
  number: number
  section: Section
  /** Clip length in seconds for each turn. */
  ladder: number[]
  attempts: Attempt[]
  status: Status
  /** The clip length unlocked right now; the last ladder step once finished. */
  clipSeconds: number
  answer: Answer | null
  /** The player's record in this section, in every state. */
  stats: Stats
}

/** One section in the overview: where the player stands there today, and whether it has a song. */
export interface TodaySection extends TabInfo {
  section: Section
}

/** The overview behind the tabs. Nothing in it comes from a song. */
export interface Today {
  day: string
  number: number
  /** One entry per section, in tab order. */
  sections: TodaySection[]
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
 * `unknown_track`, `not_found`, `no_song`, `finished`, `changed`, `upstream`,
 * `internal`) or one made up here: `network` when nothing answered,
 * `bad_response` when the answer was not the expected JSON. `message` is a
 * sentence fit to show the player.
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

/** Sends a request and hands back the response if the server said yes. */
export async function send(path: string, init?: RequestInit): Promise<Response> {
  let res: Response
  try {
    res = await fetch(path, init)
  } catch (err) {
    // A cancelled request is not a failure; let the caller see the abort.
    if (isAbort(err)) throw err
    throw new ApiError('network', "Couldn't reach the server. Check your connection and try again.", 0)
  }
  if (!res.ok) throw await errorFrom(res)
  return res
}

/** The same, read as JSON. The admin page's calls (lib/admin/api.ts) are built on these two as well. */
export async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await send(path, init)
  try {
    return (await res.json()) as T
  } catch (err) {
    if (isAbort(err)) throw err
    throw new ApiError('bad_response', 'The server sent something unexpected. Try again.', res.status)
  }
}

/** Where the player stands in each of today's four games. Fast: it never waits for a song. */
export function getToday(): Promise<Today> {
  return request<Today>('/api/today', { cache: 'no-store' })
}

/**
 * A section's game today. A page reload restores the game from this alone.
 * The first request of a day can take seconds: that is when the song is picked.
 */
export function getDaily(section: Section): Promise<Daily> {
  return request<Daily>(`/api/daily/${section}`, { cache: 'no-store' })
}

/** Skip or guess in a section. Resolves with the game after the move. */
export function postGuess(section: Section, move: Move): Promise<Daily> {
  return request<Daily>(`/api/daily/${section}/guess`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(move),
  })
}

/**
 * Clear my data: deletes every game stored for this browser and makes it a
 * new player. The day's songs stay the same.
 */
export async function clearPlayer(): Promise<void> {
  await send('/api/player', { method: 'DELETE' })
}

/** Up to 8 songs matching the text; none for fewer than 2 characters. */
export function searchTracks(query: string, signal?: AbortSignal): Promise<Track[]> {
  return request<Track[]>(`/api/search?q=${encodeURIComponent(query)}`, { signal })
}

/**
 * Where a game's currently unlocked clip is. The audio changes after every
 * move and every midnight while the path stays the same, so the query, which
 * the server ignores, makes each state its own URL.
 */
export function audioUrl(daily: Daily): string {
  return `/api/daily/${daily.section}/audio?t=${daily.day}-${daily.attempts.length}-${daily.status}`
}
