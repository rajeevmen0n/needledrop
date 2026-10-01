// Random mode as the page words it and decides about it: the session's score,
// what a refused request calls for, and the mark that says this browser tab
// has a session. Pure: no DOM and no imports, so `node --test` can load this
// file directly (see web/test/). The numbers themselves are the server's.

/** The score the server sends with every random song. */
export interface RandomScore {
  /** Wins in a row in this session; a lost song sets it back to 0. */
  run: number
  /** The longest run this player ever had: the one number that outlives a session. */
  bestRun: number
  /** Songs finished in this session, won or lost. */
  played: number
  /** How many of those were won. */
  won: number
}

function count(value: number): number {
  return Number.isFinite(value) && value > 0 ? Math.floor(value) : 0
}

/** "1 win in a row", "3 wins in a row". */
export function runCount(wins: number): string {
  const whole = count(wins)
  return `${whole} ${whole === 1 ? 'win' : 'wins'} in a row`
}

/** The run in a line, for a song still being played. */
export function runLine(score: RandomScore | null): string {
  if (!score) return ''
  return score.run > 0 ? `Run: ${count(score.run)}` : 'No run yet'
}

/** The share of the session's songs that were won, as a whole number from 0 to 100; 0 when none is finished. */
export function winPercent(won: number, played: number): number {
  const all = count(played)
  if (all === 0) return 0
  return Math.round((Math.min(count(won), all) / all) * 100)
}

/** The four figures under a finished random song, in reading order. */
export function scoreFigures(score: RandomScore): { label: string; value: string; spoken: string }[] {
  const run = count(score.run)
  // The server keeps the best at or above the run; this only keeps a bad number off the page.
  const best = Math.max(count(score.bestRun), run)
  const played = count(score.played)
  const won = Math.min(count(score.won), played)
  const percent = `${winPercent(won, played)}%`
  return [
    {
      label: 'Run',
      value: String(run),
      spoken: run > 0 ? `Current run: ${runCount(run)}` : 'Current run: none',
    },
    {
      label: 'Best ever',
      value: String(best),
      spoken: best > 0 ? `Best run ever: ${runCount(best)}` : 'Best run ever: none',
    },
    {
      label: 'Played',
      value: String(played),
      spoken: `Played this session: ${played}`,
    },
    {
      label: 'Won',
      value: percent,
      spoken: `Won this session: ${percent}, ${won} of ${played}`,
    },
  ]
}

/**
 * Which song of the session is on the record, 1 for the first. A finished
 * song is already counted in `played`; one still being played comes after them.
 */
export function songNumber(song: { status: string; played: number }): number {
  const played = count(song.played)
  return song.status === 'playing' ? played + 1 : Math.max(played, 1)
}

/** "Song 4": what the header and the record's label say in place of the day. */
export function songLine(song: { status: string; played: number } | null): string {
  return song ? `Song ${songNumber(song)}` : ''
}

// --- what a refused request calls for ----------------------------------------------

/**
 * What the page does when the server refuses a random move or draw:
 * `reload` asks for the game again and says nothing; `tell` does the same and
 * says the song was changed elsewhere; `restart` starts a new session, because
 * the server has no game for this player any more; `empty` shows that there
 * is no song to play; `report` shows the server's sentence and changes nothing.
 */
export type Remedy = 'reload' | 'tell' | 'restart' | 'empty' | 'report'

/** The remedy for an error code of the random routes (`ApiError.code`). */
export function remedyFor(code: string): Remedy {
  switch (code) {
    // The song ended, or has not, in another tab: show how it stands.
    case 'finished':
    case 'unfinished':
      return 'reload'
    case 'changed':
      return 'tell'
    case 'no_game':
      return 'restart'
    case 'no_song':
      return 'empty'
    default:
      return 'report'
  }
}

// --- the session mark ---------------------------------------------------------------

/** Under this key `sessionStorage` holds the mark. */
export const SESSION_KEY = 'nd_random'

/** As much of `Storage` as the mark needs. */
export interface MarkStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

/**
 * Whether this browser tab has a random session. A session is the tab's: the
 * mark lives in `sessionStorage`, which a reload keeps and closing the tab
 * drops, so a reload continues the session and a new visit starts one.
 *
 * `storage` hands over `sessionStorage`. It may throw (a browser that blocks
 * site data) or give nothing; then the mark is only remembered here, for as
 * long as the page lives, and every page load is a new session.
 */
export function sessionMark(storage: () => MarkStorage | null | undefined) {
  let held = false
  return {
    /** A session was started in this tab and not dropped since. */
    known(): boolean {
      if (held) return true
      try {
        return storage()?.getItem(SESSION_KEY) != null
      } catch {
        return false
      }
    },
    /** A session has been started. */
    keep(): void {
      held = true
      try {
        storage()?.setItem(SESSION_KEY, '1')
      } catch {
        // Remembered above for this page load; that is all there is.
      }
    },
    /** The session is over (Clear my data): the next look starts another. */
    drop(): void {
      held = false
      try {
        storage()?.removeItem(SESSION_KEY)
      } catch {
        // Nothing was stored, so there is nothing to remove.
      }
    },
  }
}
