// A player's record in one section, as the page words and draws it. Pure: no
// DOM and no imports, so `node --test` can load this file directly (see
// web/test/). The numbers themselves are worked out by the server.

/** The `stats` of a game response. */
export interface Stats {
  /** Finished games, won or lost. */
  played: number
  won: number
  /** Whole number, 0 to 100. */
  winPercent: number
  /** Consecutive days won, ending today (or yesterday while today's game is open). */
  currentStreak: number
  bestStreak: number
  /** Wins on the first try, the second, … the seventh. */
  guessDistribution: number[]
}

/** How many tries a game has, and so how many rows the distribution has. */
export const TRIES = 7

function count(value: number): number {
  return Number.isFinite(value) && value > 0 ? Math.floor(value) : 0
}

/** "1 day", "3 days". */
export function dayCount(days: number): string {
  const whole = count(days)
  return `${whole} ${whole === 1 ? 'day' : 'days'}`
}

/** The streak in a line, for a game still being played. */
export function streakLine(stats: Stats | null): string {
  if (!stats) return ''
  return stats.currentStreak > 0 ? `Streak: ${dayCount(stats.currentStreak)}` : 'No streak yet'
}

/** "83%". The server rounds; this only keeps a bad number off the page. */
export function percentText(percent: number): string {
  return `${Math.min(100, Math.round(count(percent)))}%`
}

/**
 * The try a won game was won on, 1-based; `null` for a game that is lost or
 * still open. A win adds no attempt, so it came on the try after the recorded ones.
 */
export function winningTry(status: string, attempts: number): number | null {
  return status === 'won' ? count(attempts) + 1 : null
}

/** The four figures above the distribution, in reading order. */
export function figures(stats: Stats): { label: string; value: string; spoken: string }[] {
  return [
    {
      label: 'Streak',
      value: String(count(stats.currentStreak)),
      spoken: `Current streak: ${dayCount(stats.currentStreak)}`,
    },
    {
      label: 'Best',
      value: String(count(stats.bestStreak)),
      spoken: `Best streak: ${dayCount(stats.bestStreak)}`,
    },
    {
      label: 'Played',
      value: String(count(stats.played)),
      spoken: `Played: ${count(stats.played)}`,
    },
    {
      label: 'Won',
      value: percentText(stats.winPercent),
      spoken: `Won: ${percentText(stats.winPercent)}, ${count(stats.won)} of ${count(stats.played)}`,
    },
  ]
}

/** One row of the guess distribution. */
export interface DistributionRow {
  /** 1 to 7. */
  try: number
  /** Wins on that try. */
  wins: number
  /** The bar's length, 0 to 1: this row's wins against the longest row's. */
  fraction: number
  /** Today's win came on this try. */
  today: boolean
  /** The row as a sentence: "Try 3: 4 wins, including today's". */
  spoken: string
}

/**
 * The guess distribution as rows to draw. Bars are scaled to the largest
 * count, so the best try always fills the track; with no wins at all every
 * bar is empty. `todaysTry` (see `winningTry`) marks the row of today's win.
 */
export function distribution(
  counts: readonly number[],
  todaysTry: number | null = null,
): DistributionRow[] {
  const wins = Array.from({ length: TRIES }, (_, i) => count(counts[i] ?? 0))
  const most = Math.max(...wins)
  return wins.map((won, i) => {
    const today = todaysTry === i + 1 && won > 0
    const amount = won === 0 ? 'no wins' : won === 1 ? '1 win' : `${won} wins`
    return {
      try: i + 1,
      wins: won,
      fraction: most > 0 ? won / most : 0,
      today,
      spoken: `Try ${i + 1}: ${amount}${today ? (won === 1 ? ", today's" : ", including today's") : ''}`,
    }
  })
}
