// Which of the seven sleeves (colour pairs in styles/tokens.css) a game wears.
// Pure, so `node --test` can load this file directly (see web/test/).

/** One sleeve per weekday, numbered like `Date.getUTCDay()`: 0 is Sunday. */
export const SLEEVE_COUNT = 7

/**
 * The sleeve for a game day such as "2026-10-01": its weekday in UTC.
 * The game's day comes from the server, so two players in different time zones
 * see the same pressing. `null` when the text is not a date.
 */
export function sleeveForDay(day: string): number | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(day)) return null
  const time = Date.parse(`${day}T00:00:00Z`)
  return Number.isNaN(time) ? null : new Date(time).getUTCDay()
}

/** `?sleeve=0..6` in the page address previews a sleeve on any day. `null` without a valid one. */
export function sleeveOverride(search: string): number | null {
  const value = new URLSearchParams(search).get('sleeve')
  if (value === null || !/^\d$/.test(value)) return null
  const index = Number(value)
  return index < SLEEVE_COUNT ? index : null
}
