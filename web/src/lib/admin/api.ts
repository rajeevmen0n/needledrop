// Typed fetch wrappers for the server's `/api/admin` routes. They are not
// protected: whoever can reach the page can call them. The shapes are in
// `model.ts`; failures are the player API's `ApiError`, whose message is a
// sentence the server wrote to be shown.

import { ApiError, request, send } from '../api'
import { isSongsPage, songsQuery } from './model'
import type { AdminHit, AdminState, Find, Genre, Pool, SongRow, SongsPage } from './model'

const JSON_BODY = { 'Content-Type': 'application/json' }

/**
 * The server's day and today's four answers. It makes the picks that are
 * missing, so it can take as long as a few downloads.
 */
export function getState(): Promise<AdminState> {
  return request<AdminState>('/api/admin/state', { cache: 'no-store' })
}

/**
 * Draws another song for one section, or for all four with `null`, and
 * deletes every player's game there today. Resolves with the new state.
 */
export function reroll(section: Pool | null): Promise<AdminState> {
  return request<AdminState>('/api/admin/reroll', {
    method: 'POST',
    headers: JSON_BODY,
    body: JSON.stringify(section === null ? {} : { section }),
  })
}

/** Simulate next day: one more day of offset, for everyone. Resolves with the new state. */
export function nextDay(): Promise<AdminState> {
  return request<AdminState>('/api/admin/next-day', { method: 'POST' })
}

/** Reset to day 1: deletes every game and every pick; the pool is kept. Resolves with the new state. */
export function resetDays(): Promise<AdminState> {
  return request<AdminState>('/api/admin/reset', { method: 'POST' })
}

/**
 * Searches the pool: the first songs that match, how many match in all, and
 * the counts of the whole pool. The server does the searching; an idle search
 * (no text, no filter) asks for the counts alone.
 */
export async function findSongs(find: Find, signal?: AbortSignal): Promise<SongsPage> {
  const answer = await request<unknown>(`/api/admin/songs?${songsQuery(find)}`, { cache: 'no-store', signal })
  if (!isSongsPage(answer)) {
    // A server from before the searchable route answers with the whole pool as a list.
    throw new ApiError(
      'bad_response',
      'The server answered the pool search in a way this page does not know. It may need a restart to match the page.',
      200,
    )
  }
  return answer
}

/**
 * Adds a song, or replaces the genres of one that is in the pool already.
 * The server never checks that the track has a preview: the page does, before
 * calling this. Resolves with the stored row.
 */
export function saveSong(trackId: number, genres: Genre[]): Promise<SongRow> {
  return request<SongRow>('/api/admin/songs', {
    method: 'POST',
    headers: JSON_BODY,
    body: JSON.stringify({ trackId, genres }),
  })
}

/** Takes a song out of the pool, with its genre tags. */
export async function deleteSong(trackId: number): Promise<void> {
  await send(`/api/admin/songs/${trackId}`, { method: 'DELETE' })
}

/**
 * Up to 25 releases matching the text, each with whether it can be played and
 * whether it is in the pool already; none for fewer than 2 characters.
 */
export function searchReleases(query: string, signal?: AbortSignal): Promise<AdminHit[]> {
  return request<AdminHit[]>(`/api/admin/search?q=${encodeURIComponent(query)}`, { signal })
}

/** A failure as the sentence to show: the server's own message when it sent one. */
export function sentence(err: unknown): string {
  return err instanceof ApiError ? err.message : 'Something went wrong. Try again.'
}
