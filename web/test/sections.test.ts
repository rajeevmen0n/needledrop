// Run with `pnpm test` (plain `node --test`; Node strips the types itself).

import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  RANDOM,
  SECTIONS,
  TABS,
  isSection,
  isTab,
  pageTitle,
  routeFor,
  tabForKey,
  tabId,
  tabLabel,
  tabPath,
  tabState,
  tabStateWords,
} from '../src/lib/sections.ts'
import type { TabInfo, TabState } from '../src/lib/sections.ts'

test('the sections are the four slugs, in tab order', () => {
  assert.deepEqual([...SECTIONS], ['general', 'pop', 'rock', 'hip-hop'])
  assert.deepEqual(SECTIONS.map(tabLabel), ['General', 'Pop', 'Rock', 'Hip-hop'])
})

test('the tab row is the four sections and then random mode', () => {
  assert.deepEqual([...TABS], ['general', 'pop', 'rock', 'hip-hop', 'random'])
  assert.equal(TABS.at(-1), RANDOM)
  assert.equal(tabLabel(RANDOM), 'Random')
})

test('random mode is a tab and not a section', () => {
  assert.equal(isTab(RANDOM), true)
  assert.equal(isSection(RANDOM), false)
  for (const section of SECTIONS) assert.equal(isTab(section), true)
  for (const text of ['', 'Random', 'random ', 'shuffle', 'admin']) {
    assert.equal(isTab(text), false, text)
  }
})

test('isSection knows the slugs and nothing else', () => {
  for (const section of SECTIONS) assert.equal(isSection(section), true)
  for (const text of ['', 'jazz', 'Pop', 'hiphop', 'hip hop', 'admin', 'general ']) {
    assert.equal(isSection(text), false, text)
  }
})

test('each tab has its own address, General at the root', () => {
  assert.deepEqual(TABS.map(tabPath), ['/', '/pop', '/rock', '/hip-hop', '/random'])
})

test('an address leads back to its tab', () => {
  for (const tab of TABS) {
    assert.deepEqual(routeFor(tabPath(tab)), {
      page: 'game',
      tab,
      path: tabPath(tab),
    })
  }
})

test('case and trailing slashes do not matter, and the canonical address is given back', () => {
  assert.deepEqual(routeFor('/pop/'), { page: 'game', tab: 'pop', path: '/pop' })
  assert.deepEqual(routeFor('/Hip-Hop'), { page: 'game', tab: 'hip-hop', path: '/hip-hop' })
  assert.deepEqual(routeFor('//rock//'), { page: 'game', tab: 'rock', path: '/rock' })
  assert.deepEqual(routeFor('/Random/'), { page: 'game', tab: 'random', path: '/random' })
  // General has no address of its own but the root.
  assert.deepEqual(routeFor('/general'), { page: 'game', tab: 'general', path: '/' })
})

test('an unknown address is the General game', () => {
  const general = { page: 'game', tab: 'general', path: '/' }
  for (const path of [
    '',
    '/',
    '/jazz',
    '/hiphop',
    '/pop/extra',
    '/random/next',
    '/randomly',
    '/index.html',
    '/administrator',
    '/pop%20',
    '/api/today',
  ]) {
    assert.deepEqual(routeFor(path), general, path)
  }
})

test('/admin and everything under it is the admin page, never the game', () => {
  for (const path of ['/admin', '/admin/', '/Admin', '/admin/songs', '/admin/pop']) {
    assert.deepEqual(routeFor(path), { page: 'admin' }, path)
  }
})

test('the page title names the tab, except for General', () => {
  assert.equal(pageTitle('general'), 'Needledrop')
  assert.equal(pageTitle('hip-hop'), 'Hip-hop · Needledrop')
  assert.equal(pageTitle('random'), 'Random · Needledrop')
})

test('tab ids are distinct and usable as DOM ids', () => {
  const ids = TABS.map(tabId)
  assert.equal(new Set(ids).size, TABS.length)
  for (const id of ids) assert.match(id, /^[a-z][a-z-]*$/)
})

const info = (status: TabInfo['status'], attempts: number, song: TabInfo['song'] = 'picked'): TabInfo => ({
  status,
  attempts,
  song,
})

test('a tab is unknown until the server has said something', () => {
  assert.equal(tabState(null), 'unknown')
  assert.equal(tabStateWords('unknown'), '')
})

test('a tab tells unplayed from in progress by the tries used', () => {
  assert.equal(tabState(info('playing', 0)), 'unplayed')
  assert.equal(tabState(info('playing', 1)), 'playing')
  assert.equal(tabState(info('playing', 6)), 'playing')
  // Not picked yet is still a game that can be played.
  assert.equal(tabState(info('playing', 0, 'pending')), 'unplayed')
})

test('a tab shows a finished game as won or lost', () => {
  assert.equal(tabState(info('won', 0)), 'won')
  assert.equal(tabState(info('won', 6)), 'won')
  assert.equal(tabState(info('lost', 7)), 'lost')
})

test('a section without a song says so, unless its game is already over', () => {
  assert.equal(tabState(info('playing', 0, 'none')), 'none')
  assert.equal(tabState(info('playing', 2, 'none')), 'none')
  assert.equal(tabState(info('won', 2, 'none')), 'won')
  assert.equal(tabState(info('lost', 7, 'none')), 'lost')
})

test('every known state has its own words', () => {
  const states: TabState[] = ['unplayed', 'playing', 'won', 'lost', 'none', 'endless']
  const words = states.map(tabStateWords)
  assert.deepEqual(words, ['not played yet', 'in progress', 'won', 'lost', 'no song today', 'endless'])
  assert.equal(new Set(words).size, states.length)
})

test('the arrow keys move between tabs and wrap around', () => {
  assert.equal(tabForKey('ArrowRight', 0, 5), 1)
  assert.equal(tabForKey('ArrowRight', 4, 5), 0)
  assert.equal(tabForKey('ArrowLeft', 2, 5), 1)
  assert.equal(tabForKey('ArrowLeft', 0, 5), 4)
})

test('Home and End go to the first and the last tab', () => {
  assert.equal(tabForKey('Home', 2, 5), 0)
  assert.equal(tabForKey('End', 0, 5), 4)
})

test('other keys move nothing', () => {
  for (const key of ['Enter', ' ', 'Tab', 'ArrowDown', 'ArrowUp', 'a', 'Escape']) {
    assert.equal(tabForKey(key, 1, 4), null, key)
  }
  assert.equal(tabForKey('ArrowRight', 0, 0), null)
})
