// Run with `pnpm test` (plain `node --test`; Node strips the types itself).

import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  POOLS,
  POOL_LEGEND,
  POOL_NOTE,
  emptyLine,
  poolName,
  poolNews,
  poolOf,
  remedyFor,
  runCount,
  runLine,
  sameSession,
  scoreFigures,
  songLine,
  songNumber,
  winPercent,
} from '../src/lib/random.ts'
import type { Pool, RandomScore } from '../src/lib/random.ts'

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

test('a session that has ended, or never was, asks for the session to play in', () => {
  assert.equal(remedyFor('no_game'), 'restart')
})

test('nothing to draw shows that there is no song', () => {
  assert.equal(remedyFor('no_song'), 'empty')
})

test('every other failure is reported and changes nothing', () => {
  for (const code of ['upstream', 'internal', 'rate_limited', 'bad_request', 'unknown_track', 'network', 'bad_response', '']) {
    assert.equal(remedyFor(code), 'report', code)
  }
})

// --- one session or another --------------------------------------------------------

test('the same song is the same session, whatever happened to it', () => {
  const playing = { round: 12, status: 'playing', played: 3 }
  assert.equal(sameSession(playing, playing), true)
  // Won since: it is counted now, and still the fourth song.
  assert.equal(sameSession(playing, { round: 12, status: 'won', played: 4 }), true)
})

test('a later song of the session is as many songs on as rounds', () => {
  const before = { round: 12, status: 'playing', played: 3 }
  assert.equal(sameSession(before, { round: 13, status: 'playing', played: 4 }), true)
  assert.equal(sameSession(before, { round: 15, status: 'lost', played: 7 }), true)
  // From a finished song to the next one.
  assert.equal(sameSession({ round: 12, status: 'won', played: 4 }, { round: 13, status: 'playing', played: 4 }), true)
})

test('a new session counts its songs from one while the rounds go on', () => {
  const before = { round: 12, status: 'playing', played: 3 }
  assert.equal(sameSession(before, { round: 13, status: 'playing', played: 0 }), false)
  assert.equal(sameSession(before, { round: 14, status: 'won', played: 2 }), false)
  // Even from the first song of a session: the next round would be its second song.
  assert.equal(
    sameSession({ round: 5, status: 'playing', played: 0 }, { round: 6, status: 'playing', played: 0 }),
    false,
  )
  assert.equal(
    sameSession({ round: 5, status: 'lost', played: 1 }, { round: 6, status: 'playing', played: 0 }),
    false,
  )
})

test('rounds that went back are another player\'s session', () => {
  // The data was cleared, or everything was reset: the rounds start again.
  assert.equal(sameSession({ round: 5, status: 'playing', played: 4 }, { round: 1, status: 'playing', played: 0 }), false)
})

// --- the pool the songs are drawn from -----------------------------------------------

test('the pools are everything first, then the three genres in tab order', () => {
  assert.deepEqual(POOLS, ['general', 'pop', 'rock', 'hip-hop'])
})

test('the whole pool is called All, a genre by its name', () => {
  assert.deepEqual(POOLS.map(poolName), ['All', 'Pop', 'Rock', 'Hip-hop'])
})

test('the choice has a short label and a note for a song still being played', () => {
  assert.equal(POOL_LEGEND, 'Genre')
  assert.equal(POOL_NOTE, 'Applies from the next song')
})

test('a pool the page does not know is the whole pool', () => {
  assert.equal(poolOf('rock'), 'rock')
  assert.equal(poolOf('hip-hop'), 'hip-hop')
  assert.equal(poolOf('general'), 'general')
  // An older server names none; nothing asked for yet is `null`.
  assert.equal(poolOf(undefined), 'general')
  assert.equal(poolOf(null), 'general')
  assert.equal(poolOf('jazz'), 'general')
  assert.equal(poolOf('Rock'), 'general')
  assert.equal(poolOf(3), 'general')
  assert.equal(poolName('jazz' as Pool), 'All')
})

test('a choice made during a song says that the song stays', () => {
  assert.equal(poolNews('rock', true), 'Genre set to Rock. It applies from the next song.')
  assert.equal(poolNews('general', true), 'Genre set to All. It applies from the next song.')
})

test('a choice made after a song only says what was chosen', () => {
  assert.equal(poolNews('hip-hop', false), 'Genre set to Hip-hop.')
  assert.equal(poolNews('general', false), 'Genre set to All.')
})

test('choosing twice in a row never says the same sentence, so each is announced', () => {
  const said = POOLS.map((pool) => poolNews(pool, true))
  assert.equal(new Set(said).size, POOLS.length)
})

test('an empty genre points at the other genres', () => {
  assert.equal(
    emptyLine('rock'),
    'Rock has nothing Random can play right now. Choose another genre, or check again in a moment.',
  )
  assert.equal(
    emptyLine('hip-hop'),
    'Hip-hop has nothing Random can play right now. Choose another genre, or check again in a moment.',
  )
})

test('the whole pool being empty points at the day\'s sections', () => {
  assert.equal(
    emptyLine('general'),
    "Random has nothing it can play right now. Check again in a moment, or play today's sections.",
  )
})
