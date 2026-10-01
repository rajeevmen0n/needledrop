// Run with `pnpm test` (plain `node --test`; Node strips the types itself).

import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  SESSION_KEY,
  remedyFor,
  runCount,
  runLine,
  scoreFigures,
  sessionMark,
  songLine,
  songNumber,
  winPercent,
} from '../src/lib/random.ts'
import type { MarkStorage, RandomScore } from '../src/lib/random.ts'

const NONE: RandomScore = { run: 0, bestRun: 0, played: 0, won: 0 }
const SOME: RandomScore = { run: 3, bestRun: 7, played: 8, won: 6 }

// --- the score ----------------------------------------------------------------

test('runCount knows one win from several', () => {
  assert.equal(runCount(0), '0 wins in a row')
  assert.equal(runCount(1), '1 win in a row')
  assert.equal(runCount(12), '12 wins in a row')
})

test('the run line says the run, or that there is none yet', () => {
  assert.equal(runLine(SOME), 'Run: 3')
  assert.equal(runLine({ ...SOME, run: 1 }), 'Run: 1')
  assert.equal(runLine(NONE), 'No run yet')
  // A long best run is not a run: only this session's wins in a row count.
  assert.equal(runLine({ ...SOME, run: 0 }), 'No run yet')
})

test('the run line is empty while the score is unknown', () => {
  assert.equal(runLine(null), '')
})

test('the win rate is a whole number, and 0 before a song is finished', () => {
  assert.equal(winPercent(0, 0), 0)
  assert.equal(winPercent(6, 8), 75)
  assert.equal(winPercent(1, 3), 33)
  assert.equal(winPercent(2, 3), 67)
  // A half goes up.
  assert.equal(winPercent(1, 8), 13)
  assert.equal(winPercent(5, 5), 100)
})

test('the win rate stays between 0 and 100 whatever arrives', () => {
  assert.equal(winPercent(9, 5), 100)
  assert.equal(winPercent(-1, 5), 0)
  assert.equal(winPercent(3, -2), 0)
  assert.equal(winPercent(Number.NaN, 4), 0)
})

test('the figures are run, best ever, played and win rate, in that order', () => {
  assert.deepEqual(scoreFigures(SOME), [
    { label: 'Run', value: '3', spoken: 'Current run: 3 wins in a row' },
    { label: 'Best ever', value: '7', spoken: 'Best run ever: 7 wins in a row' },
    { label: 'Played', value: '8', spoken: 'Played this session: 8' },
    { label: 'Won', value: '75%', spoken: 'Won this session: 75%, 6 of 8' },
  ])
})

test('the figures of an empty score say so in words', () => {
  assert.deepEqual(scoreFigures(NONE), [
    { label: 'Run', value: '0', spoken: 'Current run: none' },
    { label: 'Best ever', value: '0', spoken: 'Best run ever: none' },
    { label: 'Played', value: '0', spoken: 'Played this session: 0' },
    { label: 'Won', value: '0%', spoken: 'Won this session: 0%, 0 of 0' },
  ])
})

test('the best run shown is never below the run', () => {
  const figures = scoreFigures({ run: 4, bestRun: 2, played: 4, won: 4 })
  assert.equal(figures[1].value, '4')
  assert.equal(figures[1].spoken, 'Best run ever: 4 wins in a row')
})

test('a bad number never reaches the figures', () => {
  const figures = scoreFigures({ run: -3, bestRun: Number.NaN, played: 2.9, won: 5 })
  assert.deepEqual(
    figures.map((figure) => figure.value),
    ['0', '0', '2', '100%'],
  )
  assert.equal(figures[3].spoken, 'Won this session: 100%, 2 of 2')
})

// --- which song of the session ---------------------------------------------------

test('a song being played comes after the finished ones', () => {
  assert.equal(songNumber({ status: 'playing', played: 0 }), 1)
  assert.equal(songNumber({ status: 'playing', played: 3 }), 4)
})

test('a finished song is already counted', () => {
  assert.equal(songNumber({ status: 'won', played: 4 }), 4)
  assert.equal(songNumber({ status: 'lost', played: 1 }), 1)
  // Never "Song 0", whatever the totals say.
  assert.equal(songNumber({ status: 'won', played: 0 }), 1)
})

test('a song keeps its number when it ends', () => {
  const before = songNumber({ status: 'playing', played: 3 })
  const after = songNumber({ status: 'won', played: 4 })
  assert.equal(before, after)
})

test('the song line names the song, and is empty before there is one', () => {
  assert.equal(songLine({ status: 'playing', played: 3 }), 'Song 4')
  assert.equal(songLine({ status: 'lost', played: 12 }), 'Song 12')
  assert.equal(songLine(null), '')
})

// --- what a refused request calls for ----------------------------------------------

test('a song that ended or has not ended elsewhere is loaded again, without a word', () => {
  assert.equal(remedyFor('finished'), 'reload')
  assert.equal(remedyFor('unfinished'), 'reload')
})

test('a move for a song that is no longer played is told, and the game loaded again', () => {
  assert.equal(remedyFor('changed'), 'tell')
})

test('a game the server no longer has starts a new session', () => {
  assert.equal(remedyFor('no_game'), 'restart')
})

test('nothing to draw shows that there is no song', () => {
  assert.equal(remedyFor('no_song'), 'empty')
})

test('every other failure is reported and changes nothing', () => {
  for (const code of ['upstream', 'internal', 'bad_request', 'unknown_track', 'network', 'bad_response', '']) {
    assert.equal(remedyFor(code), 'report', code)
  }
})

// --- the session mark ---------------------------------------------------------------

/** A `sessionStorage` stand-in over a map the test can look into. */
function storageOver(items: Map<string, string>): MarkStorage {
  return {
    getItem: (key) => items.get(key) ?? null,
    setItem: (key, value) => void items.set(key, value),
    removeItem: (key) => void items.delete(key),
  }
}

test('a browser tab has no session until one is started', () => {
  const mark = sessionMark(() => storageOver(new Map()))
  assert.equal(mark.known(), false)
})

test('a started session is kept under the key and known', () => {
  const items = new Map<string, string>()
  const mark = sessionMark(() => storageOver(items))
  mark.keep()
  assert.equal(mark.known(), true)
  assert.equal(SESSION_KEY, 'nd_random')
  assert.equal(items.has(SESSION_KEY), true)
})

test('a reload finds the session of the same browser tab', () => {
  const items = new Map<string, string>()
  sessionMark(() => storageOver(items)).keep()
  // The page is loaded again: a new mark over the same storage.
  assert.equal(sessionMark(() => storageOver(items)).known(), true)
})

test('a dropped session is gone, for this page and the next', () => {
  const items = new Map<string, string>()
  const mark = sessionMark(() => storageOver(items))
  mark.keep()
  mark.drop()
  assert.equal(mark.known(), false)
  assert.equal(items.has(SESSION_KEY), false)
  assert.equal(sessionMark(() => storageOver(items)).known(), false)
})

test('without storage the session lasts as long as the page', () => {
  for (const storage of [() => null, () => undefined]) {
    const mark = sessionMark(storage)
    assert.equal(mark.known(), false)
    mark.keep()
    assert.equal(mark.known(), true)
    mark.drop()
    assert.equal(mark.known(), false)
    // The next page load has nothing to find.
    assert.equal(sessionMark(storage).known(), false)
  }
})

test('storage that throws breaks nothing', () => {
  const blocked = (): MarkStorage => {
    throw new Error('The operation is insecure.')
  }
  const mark = sessionMark(blocked)
  assert.equal(mark.known(), false)
  assert.doesNotThrow(() => mark.keep())
  assert.equal(mark.known(), true)
  assert.doesNotThrow(() => mark.drop())
  assert.equal(mark.known(), false)
})

test('storage whose methods throw breaks nothing either', () => {
  const full: MarkStorage = {
    getItem: () => {
      throw new Error('denied')
    },
    setItem: () => {
      throw new Error('quota')
    },
    removeItem: () => {
      throw new Error('denied')
    },
  }
  const mark = sessionMark(() => full)
  assert.equal(mark.known(), false)
  assert.doesNotThrow(() => mark.keep())
  assert.equal(mark.known(), true)
  assert.doesNotThrow(() => mark.drop())
  assert.equal(mark.known(), false)
})
