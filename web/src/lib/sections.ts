// The four daily sections and random mode: the five tabs, where each lives in
// the address bar, and what a tab says about its game. Pure: no DOM and no
// imports, so `node --test` can load this file directly (see web/test/).

/** The daily sections in tab order. The slugs are the ones the API and the URLs use. */
export const SECTIONS = ['general', 'pop', 'rock', 'hip-hop'] as const

export type Section = (typeof SECTIONS)[number]

/** Random mode: a tab like the sections, but not one of them. It has no day and never ends. */
export const RANDOM = 'random'

/** Everything in the tab row, in order: the four sections, then random mode. */
export const TABS = [...SECTIONS, RANDOM] as const

export type Tab = (typeof TABS)[number]

const LABELS: Record<Tab, string> = {
  general: 'General',
  pop: 'Pop',
  rock: 'Rock',
  'hip-hop': 'Hip-hop',
  random: 'Random',
}

export function isSection(text: string): text is Section {
  return (SECTIONS as readonly string[]).includes(text)
}

export function isTab(text: string): text is Tab {
  return (TABS as readonly string[]).includes(text)
}

/** "Hip-hop": the name on the tab. */
export function tabLabel(tab: Tab): string {
  return LABELS[tab]
}

/** Where a tab's game lives: `/` for General, `/pop` and so on for the rest. */
export function tabPath(tab: Tab): string {
  return tab === 'general' ? '/' : `/${tab}`
}

/** "Pop · Needledrop" for the browser tab and the history list; General is the plain name. */
export function pageTitle(tab: Tab): string {
  return tab === 'general' ? 'Needledrop' : `${tabLabel(tab)} · Needledrop`
}

/**
 * What an address shows. `path` is the address a game page should have: the
 * one it was asked for when that is already canonical, `/` for anything unknown.
 */
export type Route = { page: 'admin' } | { page: 'game'; tab: Tab; path: string }

/**
 * Which page a path is. `/admin` and everything under it is the admin page;
 * `/pop`, `/rock`, `/hip-hop` and `/random` are those tabs; everything else,
 * `/` included, is the General game. Case and trailing slashes do not matter.
 */
export function routeFor(pathname: string): Route {
  const parts = pathname
    .toLowerCase()
    .split('/')
    .filter((part) => part !== '')
  if (parts[0] === 'admin') return { page: 'admin' }
  const tab = parts.length === 1 && isTab(parts[0]) ? parts[0] : 'general'
  return { page: 'game', tab, path: tabPath(tab) }
}

// --- what a tab says ------------------------------------------------------------

/** What is known about a section's game today: the overview's entry, or the same read off a loaded game. */
export interface TabInfo {
  status: 'playing' | 'won' | 'lost'
  /** Tries used. */
  attempts: number
  song: 'picked' | 'pending' | 'none'
}

/**
 * `unknown` until the server has said anything about the section. `endless`
 * is random mode's, always: it has no game of the day to be won or lost.
 */
export type TabState = 'unknown' | 'unplayed' | 'playing' | 'won' | 'lost' | 'none' | 'endless'

export function tabState(info: TabInfo | null): TabState {
  if (!info) return 'unknown'
  // A finished game stays finished, whatever became of the song since.
  if (info.status === 'won' || info.status === 'lost') return info.status
  if (info.song === 'none') return 'none'
  return info.attempts > 0 ? 'playing' : 'unplayed'
}

const STATE_WORDS: Record<TabState, string> = {
  unknown: '',
  unplayed: 'not played yet',
  playing: 'in progress',
  won: 'won',
  lost: 'lost',
  none: 'no song today',
  endless: 'endless',
}

/** The state in words, for a screen reader and the tooltip. Empty while unknown. */
export function tabStateWords(state: TabState): string {
  return STATE_WORDS[state]
}

/**
 * Which tab a key moves the focus to: the arrows wrap around, Home and End go
 * to the ends. `null` for a key that is not one of those.
 */
export function tabForKey(key: string, index: number, count: number): number | null {
  if (count <= 0) return null
  switch (key) {
    case 'ArrowRight':
      return (index + 1) % count
    case 'ArrowLeft':
      return (index - 1 + count) % count
    case 'Home':
      return 0
    case 'End':
      return count - 1
    default:
      return null
  }
}

/** The `id` of a tab's element, which the tab panel names as its label. */
export function tabId(tab: Tab): string {
  return `tab-${tab}`
}
