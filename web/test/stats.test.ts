// Run with `pnpm test` (plain `node --test`; Node strips the types itself).

import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  TRIES,
  dayCount,
  distribution,
  figures,
  percentText,
  streakLine,
  winningTry,
} from '../src/lib/stats.ts'
import type { Stats } from '../src/lib/stats.ts'

const NONE: Stats = {
  played: 0,
  won: 0,
  winPercent: 0,
  currentStreak: 0,
  bestStreak: 0,
  guessDistribution: [0, 0, 0, 0, 0, 0, 0],
}

const SOME: Stats = {
  played: 12,
  won: 10,
  winPercent: 83,
  currentStreak: 3,
  bestStreak: 5,
  guessDistribution: [1, 0, 4, 2, 2, 0, 1],
}

test('dayCount knows one day from several', () => {
  assert.equal(dayCount(0), '0 days')
  assert.equal(dayCount(1), '1 day')
  assert.equal(dayCount(2), '2 days')
  assert.equal(dayCount(365), '365 days')
})

test('dayCount keeps nonsense off the page', () => {
  assert.equal(dayCount(-3), '0 days')
  assert.equal(dayCount(Number.NaN), '0 days')
  assert.equal(dayCount(2.9), '2 days')
})

test('streakLine names the streak, or says there is none yet', () => {
  assert.equal(streakLine(SOME), 'Streak: 3 days')
  assert.equal(streakLine({ ...SOME, currentStreak: 1 }), 'Streak: 1 day')
  assert.equal(streakLine(NONE), 'No streak yet')
  assert.equal(streakLine(null), '')
})

test('percentText is the whole percentage the server sent', () => {
  assert.equal(percentText(0), '0%')
  assert.equal(percentText(83), '83%')
  assert.equal(percentText(100), '100%')
  assert.equal(percentText(140), '100%')
  assert.equal(percentText(-5), '0%')
  assert.equal(percentText(Number.NaN), '0%')
})

test('winningTry is the try after the recorded attempts, and only for a win', () => {
  assert.equal(winningTry('won', 0), 1)
  assert.equal(winningTry('won', 2), 3)
  assert.equal(winningTry('won', 6), 7)
  assert.equal(winningTry('lost', 7), null)
  assert.equal(winningTry('playing', 3), null)
})

test('figures are streak, best, played and win rate, each with its sentence', () => {
  assert.deepEqual(figures(SOME), [
    { label: 'Streak', value: '3', spoken: 'Current streak: 3 days' },
    { label: 'Best', value: '5', spoken: 'Best streak: 5 days' },
    { label: 'Played', value: '12', spoken: 'Played: 12' },
    { label: 'Won', value: '83%', spoken: 'Won: 83%, 10 of 12' },
  ])
  assert.deepEqual(
    figures(NONE).map((figure) => figure.value),
    ['0', '0', '0', '0%'],
  )
  assert.equal(figures({ ...NONE, currentStreak: 1, bestStreak: 1 })[0].spoken, 'Current streak: 1 day')
})

test('distribution has one row per try, in order', () => {
  const rows = distribution(SOME.guessDistribution)
  assert.equal(rows.length, TRIES)
  assert.deepEqual(
    rows.map((row) => row.try),
    [1, 2, 3, 4, 5, 6, 7],
  )
  assert.deepEqual(
    rows.map((row) => row.wins),
    [1, 0, 4, 2, 2, 0, 1],
  )
})

test('distribution scales the bars to the best try', () => {
  const rows = distribution(SOME.guessDistribution)
  assert.deepEqual(
    rows.map((row) => row.fraction),
    [0.25, 0, 1, 0.5, 0.5, 0, 0.25],
  )
  // One win in all still fills its bar.
  assert.equal(distribution([0, 1, 0, 0, 0, 0, 0])[1].fraction, 1)
})

test('distribution with no wins has seven empty bars and no division by zero', () => {
  const rows = distribution(NONE.guessDistribution, null)
  assert.deepEqual(
    rows.map((row) => row.fraction),
    [0, 0, 0, 0, 0, 0, 0],
  )
  assert.ok(rows.every((row) => !row.today && row.spoken.endsWith('no wins')))
})

test("distribution marks the row of today's win, and no other", () => {
  const rows = distribution(SOME.guessDistribution, 3)
  assert.deepEqual(
    rows.map((row) => row.today),
    [false, false, true, false, false, false, false],
  )
  assert.equal(distribution(SOME.guessDistribution, null).some((row) => row.today), false)
  assert.equal(distribution(SOME.guessDistribution).some((row) => row.today), false)
})

test('distribution does not mark a row the server counted no win on', () => {
  // The stats and the game disagree (a stale answer): believe the counts.
  assert.equal(distribution(SOME.guessDistribution, 2).some((row) => row.today), false)
  assert.equal(distribution(SOME.guessDistribution, 9).some((row) => row.today), false)
})

test('distribution says each row in words', () => {
  const rows = distribution(SOME.guessDistribution, 3)
  assert.equal(rows[0].spoken, 'Try 1: 1 win')
  assert.equal(rows[1].spoken, 'Try 2: no wins')
  assert.equal(rows[2].spoken, "Try 3: 4 wins, including today's")
  assert.equal(rows[3].spoken, 'Try 4: 2 wins')
  assert.equal(distribution([0, 0, 0, 0, 0, 0, 1], 7)[6].spoken, "Try 7: 1 win, today's")
})

test('distribution copes with a list that is not seven whole numbers', () => {
  assert.deepEqual(
    distribution([2, 1]).map((row) => row.wins),
    [2, 1, 0, 0, 0, 0, 0],
  )
  assert.equal(distribution([1, 2, 3, 4, 5, 6, 7, 8, 9]).length, TRIES)
  assert.deepEqual(
    distribution([-1, Number.NaN, 2.7, 0, 0, 0, 0]).map((row) => row.wins),
    [0, 0, 2, 0, 0, 0, 0],
  )
  assert.equal(distribution([]).length, TRIES)
})
