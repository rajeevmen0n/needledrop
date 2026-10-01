// What the admin page has heard from the server about the day, and the calls
// that change it: the clock and the four picks. One object for the page, as
// `game` is for the player's. The song pool has its own (finder.svelte.ts).
//
// Every call that can fail resolves to the failure as a sentence (or `null`),
// so the control that asked can show it next to itself; nothing on screen is
// dropped because a request failed.

import { getState, nextDay, reroll, resetDays, sentence } from './api'
import { clockNews, rerollNews } from './model'
import type { AdminState, Pool } from './model'

/** What is being asked about the day. One thing at a time: each answer is the whole state. */
export type DayWork = 'load' | 'next-day' | 'reset' | 'reroll'

class Desk {
  /** The server's day and today's four answers; `null` until the first answer. */
  state = $state.raw<AdminState | null>(null)
  /** What is on its way, if anything. The clock's and the picks' buttons wait for it. */
  dayWork = $state<DayWork | null>(null)
  /** Why the last load of the state failed. The state it had stays on screen. */
  stateError = $state<string | null>(null)
  /** What the last change of the clock did, and what the last re-roll did. */
  clockNews = $state('')
  picksNews = $state('')

  /** Asks for the state again. The server makes any pick that is missing, so this can take seconds. */
  async loadState(): Promise<void> {
    if (this.dayWork) return
    this.dayWork = 'load'
    try {
      this.state = await getState()
      this.stateError = null
    } catch (err) {
      this.stateError = sentence(err)
    } finally {
      this.dayWork = null
    }
  }

  async #change(work: DayWork, call: () => Promise<AdminState>): Promise<AdminState | string> {
    if (this.dayWork) return 'Something else is still on its way. Try again in a moment.'
    this.dayWork = work
    this.clockNews = ''
    this.picksNews = ''
    try {
      const state = await call()
      this.state = state
      this.stateError = null
      return state
    } catch (err) {
      return sentence(err)
    } finally {
      this.dayWork = null
    }
  }

  /** Simulate next day. */
  async nextDay(): Promise<string | null> {
    const answer = await this.#change('next-day', nextDay)
    if (typeof answer === 'string') return answer
    this.clockNews = clockNews('next-day', answer)
    return null
  }

  /** Reset to day 1: every game and every pick goes, the pool stays. */
  async reset(): Promise<string | null> {
    const answer = await this.#change('reset', resetDays)
    if (typeof answer === 'string') return answer
    this.clockNews = clockNews('reset', answer)
    return null
  }

  /** Draws again for one section, or for all four with `null`. */
  async reroll(section: Pool | null): Promise<string | null> {
    const before = this.state
    const answer = await this.#change('reroll', () => reroll(section))
    if (typeof answer === 'string') return answer
    this.picksNews = rerollNews(section, before, answer)
    return null
  }
}

export const desk = new Desk()
