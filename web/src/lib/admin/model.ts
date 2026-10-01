// What the admin page knows about the server's day and the song pool, and
// every decision and sentence that needs no browser. Pure: no DOM and no
// imports, so `node --test` can load this file directly (see web/test/).

// --- the shapes of /api/admin/* ---------------------------------------------------

/** The genre tags a song can carry, in the order the server lists them. */
export const GENRES = ['pop', 'rock', 'hip-hop'] as const

export type Genre = (typeof GENRES)[number]

/** The four pools a day's songs are drawn from: the slugs of the four sections, in tab order. */
export const POOLS = ['general', 'pop', 'rock', 'hip-hop'] as const

export type Pool = (typeof POOLS)[number]

/** One song of the pool. */
export interface SongRow {
  /** The Deezer track ID. */
  trackId: number
  title: string
  artist: string
  album: string
  /** Empty for a song that is in the General pool only. */
  genres: Genre[]
  /** The day ("2026-10-01") the daily pick last found the track without a preview. */
  previewFailedOn: string | null
}

/** How many songs the whole pool has, whatever is being searched for. */
export interface PoolCounts {
  all: number
  pop: number
  rock: number
  hipHop: number
  /** Songs whose preview failed the last daily check: the candidates for removal. */
  previewFailed: number
}

/** The answer of `GET /api/admin/songs`: what was found, the first of it, and the counts. */
export interface SongsPage {
  /** How many songs match the search. */
  total: number
  /** The offset and the limit the server applied. */
  offset: number
  limit: number
  /** The first `limit` of them, by artist, then title, then track ID. */
  songs: SongRow[]
  counts: PoolCounts
}

/** One result of the admin search: a release on Deezer. */
export interface AdminHit {
  id: number
  title: string
  artist: string
  album: string
  /** Small image URL; may be empty. */
  cover: string
  /** Whether Deezer has a preview for it. Without one it cannot be played. */
  playable: boolean
  /** Whether the track is in the pool already, and with which genres if it is. */
  inPool: boolean
  genres: Genre[] | null
}

/** A section's song of the day, named from the pool. */
export interface PickedSong {
  trackId: number
  title: string
  artist: string
}

/** `unavailable`: Deezer did not answer, so the pick could not be made or its song could not be loaded. */
export type PickStatus = 'picked' | 'none' | 'unavailable'

/** What a section plays today. */
export interface SectionPick {
  section: Pool
  status: PickStatus
  /** With `unavailable`, the pick that stands, if there is one. */
  pick: PickedSong | null
}

/** The server's day and today's four answers. */
export interface AdminState {
  /** The real UTC date. */
  realDay: string
  /** Days added to it; negative when the server's day is behind. */
  offset: number
  /** The server's day: `realDay` plus `offset`. */
  day: string
  /** 1 on the launch date. */
  number: number
  /** One entry per section, in tab order. */
  sections: SectionPick[]
}

// --- names ------------------------------------------------------------------------

const POOL_LABELS: Record<Pool, string> = {
  general: 'General',
  pop: 'Pop',
  rock: 'Rock',
  'hip-hop': 'Hip-hop',
}

/** "Hip-hop": a pool or a genre as the page names it. */
export function poolLabel(pool: Pool): string {
  return POOL_LABELS[pool]
}

/** "1 song", "6 songs". */
export function songCount(count: number): string {
  return count === 1 ? '1 song' : `${count} songs`
}

/** “Billie Jean” by Michael Jackson. A blank title or artist (a pick whose song left the pool) is left out. */
export function songName(song: { title: string; artist: string }): string {
  const title = song.title.trim()
  const artist = song.artist.trim()
  if (title === '') return artist === '' ? 'a song that is no longer in the pool' : `a song by ${artist}`
  return artist === '' ? `“${title}”` : `“${title}” by ${artist}`
}

/** Where a track can be looked at and listened to. */
export function deezerLink(trackId: number): string {
  return `https://www.deezer.com/track/${trackId}`
}

function list(words: string[]): string {
  if (words.length <= 1) return words.join('')
  return `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`
}

// --- genres -----------------------------------------------------------------------

/** The same tags once each, in the server's order. */
export function orderGenres(genres: readonly Genre[]): Genre[] {
  return GENRES.filter((genre) => genres.includes(genre))
}

/** The tags with one of them switched: added if it was missing, taken away if it was there. */
export function toggleGenre(genres: readonly Genre[], genre: Genre): Genre[] {
  return GENRES.filter((each) => (each === genre ? !genres.includes(each) : genres.includes(each)))
}

/** Whether two lists name the same tags, in any order. */
export function sameGenres(a: readonly Genre[], b: readonly Genre[]): boolean {
  return GENRES.every((genre) => a.includes(genre) === b.includes(genre))
}

/** The pools a song with these tags is in: "General only", "General and Pop", "General, Pop and Rock". */
export function poolsText(genres: readonly Genre[]): string {
  const tagged = orderGenres(genres)
  if (tagged.length === 0) return 'General only'
  return list(['General', ...tagged.map(poolLabel)])
}

// --- the counts -------------------------------------------------------------------

/** How many songs a section can draw on: General is every song, a genre its tagged ones. */
export function countOf(counts: PoolCounts, pool: Pool): number {
  switch (pool) {
    case 'general':
      return counts.all
    case 'pop':
      return counts.pop
    case 'rock':
      return counts.rock
    case 'hip-hop':
      return counts.hipHop
  }
}

/**
 * What the counts mean for the daily picks, where they are a problem. A genre
 * with one song plays it every day; one with none has no song. General draws
 * from what the genre sections leave, so it needs more songs than they take.
 */
export function poolNotes(counts: PoolCounts): string[] {
  if (counts.all === 0) return ['The pool is empty: no section has a song.']
  const notes: string[] = []
  for (const genre of GENRES) {
    const count = countOf(counts, genre)
    if (count === 0) {
      notes.push(`${poolLabel(genre)} has no song: it shows “No song today”.`)
    } else if (count === 1) {
      notes.push(`${poolLabel(genre)} has one song, so it plays that song every day.`)
    }
  }
  const taken = GENRES.filter((genre) => countOf(counts, genre) > 0).length
  if (counts.all <= taken) {
    notes.push(
      `General may have no song: the pool has ${songCount(counts.all)} and the genre sections take up to ${taken} of them each day.`,
    )
  }
  return notes
}

/** The totals as one line to read: "7 songs · Pop 2 · Rock 3 · Hip-hop 2". */
export function totalsText(counts: PoolCounts): string {
  return [songCount(counts.all), ...GENRES.map((genre) => `${poolLabel(genre)} ${countOf(counts, genre)}`)].join(' · ')
}

/** "1 failed the preview check", "3 failed the preview check"; empty when none did. */
export function failedText(counts: PoolCounts): string {
  return counts.previewFailed > 0 ? `${counts.previewFailed} failed the preview check` : ''
}

// --- finding a song in the pool ---------------------------------------------------
//
// The pool is meant to hold hundreds of songs, so the page neither holds nor
// lists it: the server searches and counts, and the page shows the first
// screenful of what a typed text finds. To see other songs, the text is refined.

/** What "Find a song in the pool" asks for. */
export interface Find {
  /** Words that must all be in the title, the artist, the album or the track ID. */
  text: string
  /**
   * Instead of a text: the songs whose preview failed the last daily check.
   * They are the candidates for removal, and nobody would know what to type for them.
   */
  failed: boolean
}

/** The most rows the page asks for and shows. What else matches is counted, never listed. */
export const FIND_LIMIT = 25

/** Nothing typed. */
export const FIND_NOTHING: Find = { text: '', failed: false }

/** Whether a search asks for nothing: then no song is requested or listed, only the counts. */
export function isIdle(find: Find): boolean {
  return find.text.trim() === '' && !find.failed
}

/**
 * The query string of `GET /api/admin/songs` for a search. An idle search asks
 * for the counts alone (`limit=0`); no search ever asks for a second page.
 */
export function songsQuery(find: Find, limit: number = FIND_LIMIT): string {
  if (isIdle(find)) return 'limit=0'
  const parts: string[] = []
  const text = find.text.trim()
  if (text !== '') parts.push(`q=${encodeURIComponent(text)}`)
  if (find.failed) parts.push('failed=true')
  parts.push(`limit=${Math.max(0, Math.floor(limit))}`)
  return parts.join('&')
}

/** Whether an answer is a page of songs. A server from before the searchable route sends a plain list. */
export function isSongsPage(value: unknown): value is SongsPage {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return false
  const page = value as Record<string, unknown>
  return (
    typeof page.total === 'number' &&
    typeof page.offset === 'number' &&
    typeof page.limit === 'number' &&
    Array.isArray(page.songs) &&
    typeof page.counts === 'object' &&
    page.counts !== null
  )
}

/** The answer with one row replaced by what the server just stored for that song. */
export function withRow(page: SongsPage, row: SongRow): SongsPage {
  if (!page.songs.some((song) => song.trackId === row.trackId)) return page
  return { ...page, songs: page.songs.map((song) => (song.trackId === row.trackId ? row : song)) }
}

/** What was found, as the line above the rows. Empty for an idle search. */
export function foundText(find: Find, total: number): string {
  if (isIdle(find)) return ''
  const text = find.text.trim()
  const some = total === 0 ? 'No song' : songCount(total)
  if (text !== '') {
    const failed = find.failed ? ' without a preview at the last daily check' : ''
    return `${some}${failed} ${total <= 1 ? 'matches' : 'match'} “${text}”.`
  }
  return total === 0 ? 'No song failed its last daily preview check.' : `${some} had no preview at the last daily check.`
}

/** "Showing the first 25 of 312. Refine the search to see the rest." Empty when every match is shown. */
export function moreText(shown: number, total: number): string {
  if (shown <= 0 || shown >= total) return ''
  return `Showing the first ${shown} of ${total}. Refine the search to see the rest.`
}

// --- adding a song ----------------------------------------------------------------

/** The mark on a search result: why it cannot be added, or that it already is. */
export function hitNote(hit: AdminHit): string | null {
  if (!hit.playable) return 'No preview'
  return hit.inPool ? 'In the pool' : null
}

/**
 * Why a chosen result is not added, as a sentence, or `null` when it may be.
 * This is the page's own check: the server's add never looks at the preview.
 */
export function refusal(hit: AdminHit): string | null {
  if (hit.playable) return null
  const release = hit.album.trim() === '' ? '' : ` (${hit.album.trim()})`
  const inPool = hit.inPool
    ? ' It is in the pool already; find it below to remove it.'
    : ' Nothing was added. Choose another release.'
  return `${songName(hit)}${release} has no preview on Deezer, so it cannot be played.${inPool}`
}

/**
 * What the add did, as a sentence: the row the server stored, and the genres
 * the song had if it was in the pool before (`null` for a new song).
 */
export function savedText(row: SongRow, was: readonly Genre[] | null): string {
  if (was === null) return `Added ${songName(row)} to the pool: ${poolsText(row.genres)}.`
  if (sameGenres(was, row.genres)) {
    return `${songName(row)} was in the pool already; its genres are unchanged: ${poolsText(row.genres)}.`
  }
  return `${songName(row)} was in the pool already; its genres are now: ${poolsText(row.genres)}.`
}

// --- the clock and the picks ------------------------------------------------------

/** The offset in words: "None", "1 day ahead", "3 days behind". */
export function offsetText(offset: number): string {
  if (offset === 0) return 'None'
  const days = Math.abs(offset) === 1 ? '1 day' : `${Math.abs(offset)} days`
  return offset > 0 ? `${days} ahead` : `${days} behind`
}

/** The clock in one sentence, for a screen reader. */
export function dayText(state: { day: string; number: number }): string {
  return `Day ${state.number}, ${state.day}`
}

/** What a section's entry says when it has no song to show. Empty for a section with its song. */
export function pickProblem(entry: SectionPick): string {
  if (entry.status === 'none') return 'No song today'
  if (entry.status === 'unavailable') return 'Deezer did not answer'
  return ''
}

/** The second line under a problem: what it means and what stands. */
export function pickDetail(entry: SectionPick): string {
  if (entry.status === 'none') {
    return 'Nothing in its pool can be played today. Add a song to it, then check again.'
  }
  if (entry.status === 'unavailable') {
    if (!entry.pick) return 'No song could be picked yet. The server tries again at most every 10 seconds.'
    return `The pick stands, but its song could not be loaded: ${songName(entry.pick)}, track ${entry.pick.trackId}.`
  }
  return ''
}

/** The sections whose song today is this track, in tab order. */
export function sectionsPlaying(state: AdminState | null, trackId: number): Pool[] {
  if (!state) return []
  return state.sections.filter((entry) => entry.pick?.trackId === trackId).map((entry) => entry.section)
}

/** What removing a song means, in the sentences of the question before it. */
export function removalText(song: SongRow, playing: readonly Pool[]): string[] {
  const lines = [
    `Track ${song.trackId}${song.album.trim() === '' ? '' : `, from ${song.album.trim()}`}, leaves the pool with its genre tags. It can be added again by searching Deezer for it.`,
  ]
  if (playing.length > 0) {
    lines.push(
      `It is today’s song in ${list(playing.map(poolLabel))}. That stays as it is: a pick that stands is not changed by removing its song.`,
    )
  }
  return lines
}

/** The question before a re-roll, and what it deletes. `null` is all four sections. */
export function rerollQuestion(section: Pool | null): { title: string; body: string; confirm: string } {
  if (section === null) {
    return {
      title: 'Re-roll all four sections?',
      body: 'Each section draws another song for today, where its pool has one. Every player’s game today is deleted, in all four sections.',
      confirm: 'Re-roll all',
    }
  }
  const label = poolLabel(section)
  return {
    title: `Re-roll ${label}?`,
    body: `${label} draws another song for today, where its pool has one. Every player’s game in ${label} today is deleted.`,
    confirm: `Re-roll ${label}`,
  }
}

function entryOf(state: AdminState, section: Pool): SectionPick | null {
  return state.sections.find((entry) => entry.section === section) ?? null
}

/** What a re-roll did, as a sentence, read off the state before it and the state it answered with. */
export function rerollNews(section: Pool | null, before: AdminState | null, after: AdminState): string {
  if (section === null) {
    const waiting = after.sections.filter((entry) => entry.status === 'unavailable').length
    if (waiting > 0) {
      return `All four sections were re-rolled, but Deezer did not answer for ${waiting === 1 ? 'one' : waiting} of them.`
    }
    return 'All four sections were re-rolled.'
  }
  const label = poolLabel(section)
  const now = entryOf(after, section)
  if (!now || now.status === 'none') return `${label} was re-rolled and has no song today.`
  if (now.status === 'unavailable' || !now.pick) {
    return `${label} was re-rolled, but Deezer did not answer: its song is not picked yet.`
  }
  const was = before ? entryOf(before, section)?.pick : null
  if (was && was.trackId === now.pick.trackId) {
    return `${label} drew ${songName(now.pick)} again: its pool has no other song for today.`
  }
  return `${label} now plays ${songName(now.pick)}.`
}

/** What a move of the clock did, as a sentence. */
export function clockNews(work: 'next-day' | 'reset', after: AdminState): string {
  return work === 'reset'
    ? `Back on day ${after.number}, ${after.day}. Every game and every pick was deleted; the pool is as it was.`
    : `Now on day ${after.number}, ${after.day}.`
}
