// The song pool as the admin page sees it: a search the server answers. The
// page never holds the pool, only the answer to what was last asked: the
// counts, how many songs match, and the first screenful of them.
//
// A change made here (a song's genres, a removal) asks the same search again,
// so the text, the rows and the counts stay in step without the search being reset.

import { ApiError, isAbort } from '../api'
import { deleteSong, findSongs, saveSong, sentence } from './api'
import { FIND_NOTHING, isIdle, withRow } from './model'
import type { Find, Genre, SongRow, SongsPage } from './model'

/** How long the typing has to pause before the server is asked. */
const DEBOUNCE_MS = 250

class Finder {
  /** What is asked for: the text in the box, or, instead of a text, the songs whose preview check failed. */
  text = $state('')
  failed = $state(false)

  /** The last answer, and the search it answers: the rows on screen belong to `asked`, not to what is being typed. */
  page = $state.raw<SongsPage | null>(null)
  asked = $state.raw<Find>(FIND_NOTHING)
  /** True while an answer the page shows as "Searching…" is on its way. */
  searching = $state(false)
  /** Why the last search failed. The answer before it stays on screen. */
  error = $state<string | null>(null)
  /** The song the last removal took out of the pool. */
  removed = $state<number | null>(null)

  #timer: ReturnType<typeof setTimeout> | undefined
  #request: AbortController | undefined

  /** Page load: the counts alone. No song is asked for until something is typed. */
  open(): void {
    void this.#ask()
  }

  /** The text in the box changed: ask once the typing pauses. Typing always searches the whole pool. */
  type(text: string): void {
    this.text = text
    this.failed = false
    clearTimeout(this.#timer)
    this.#request?.abort()
    this.#request = undefined
    this.searching = true
    this.#timer = setTimeout(() => void this.#ask(), DEBOUNCE_MS)
  }

  /** Lists the songs whose preview failed the last daily check, in place of a text; or puts them away again. */
  showFailed(on: boolean): void {
    this.failed = on
    this.text = ''
    void this.#ask()
  }

  /** The same search once more: the retry after a failure, and Refresh. */
  again(): Promise<void> {
    return this.#ask()
  }

  /** Shows one song: what "Add a song" just saved, so its genres can be corrected at once. */
  show(trackId: number): void {
    this.text = String(trackId)
    this.failed = false
    this.removed = null
    void this.#ask()
  }

  /**
   * Asks the server for what the box says now. An answer that
   * a newer question has overtaken is dropped. `quiet` is the asking again
   * after a change: the rows stay as they are until the answer replaces them.
   */
  async #ask(quiet = false): Promise<void> {
    clearTimeout(this.#timer)
    this.#request?.abort()
    const controller = new AbortController()
    this.#request = controller
    const find: Find = { text: this.text, failed: this.failed }
    if (!quiet) this.searching = true
    try {
      const page = await findSongs(find, controller.signal)
      if (this.#request !== controller) return
      this.page = page
      this.asked = find
      this.error = null
    } catch (err) {
      if (this.#request !== controller || isAbort(err)) return
      this.error = sentence(err)
    } finally {
      if (this.#request === controller) {
        this.#request = undefined
        this.searching = false
      }
    }
  }

  /**
   * Replaces a song's genres. The stored row goes into the rows at once; the
   * search is then asked again, because the counts changed. Resolves to the
   * row, or to why it failed.
   */
  async retag(trackId: number, genres: Genre[]): Promise<SongRow | string> {
    let row: SongRow
    try {
      row = await saveSong(trackId, genres)
    } catch (err) {
      return sentence(err)
    }
    if (this.page) this.page = withRow(this.page, row)
    void this.#ask(true)
    return row
  }

  /** Takes a song out of the pool, then asks the same search again. */
  async remove(trackId: number): Promise<string | null> {
    try {
      await deleteSong(trackId)
    } catch (err) {
      // Someone else removed it already: the rows here are old.
      if (err instanceof ApiError && err.code === 'unknown_song') void this.#ask(true)
      return sentence(err)
    }
    this.removed = trackId
    await this.#ask(true)
    return null
  }

  /** Whether nothing is asked for: then no song is listed. */
  get idle(): boolean {
    return isIdle(this.asked)
  }
}

export const finder = new Finder()
