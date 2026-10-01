// Run with `pnpm test` (plain `node --test`; Node strips the types itself).

import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  MAX_SILENCE_SKIP,
  clipLabel,
  clipShort,
  findAudibleStart,
  ladderFraction,
  playableSeconds,
} from '../src/lib/clip.ts'

const RATE = 44100
const LADDER = [0.1, 0.3, 1, 3, 8, 16, 30]

/** `silent` samples of near-silence, then a loud signal. */
function channel(silent: number, total: number, floor = 0): Float32Array {
  const data = new Float32Array(total).fill(floor)
  for (let i = silent; i < total; i++) data[i] = 0.5
  return data
}

function close(actual: number, expected: number): void {
  assert.ok(Math.abs(actual - expected) < 1e-9, `${actual} is not ${expected}`)
}

test('findAudibleStart finds the first sample above the threshold', () => {
  // 1105 samples is a typical decoder delay for a file without a LAME header.
  close(findAudibleStart([channel(1105, 8820)], RATE), 1105 / RATE)
})

test('findAudibleStart returns 0 when the sound starts at once', () => {
  assert.equal(findAudibleStart([channel(0, 8820)], RATE), 0)
})

test('findAudibleStart ignores noise below the threshold', () => {
  close(findAudibleStart([channel(1200, 8820, 0.0005)], RATE), 1200 / RATE)
})

test('findAudibleStart counts negative samples', () => {
  const data = new Float32Array(8820)
  data[700] = -0.3
  close(findAudibleStart([data], RATE), 700 / RATE)
})

test('findAudibleStart takes the earliest channel', () => {
  close(findAudibleStart([channel(2000, 8820), channel(900, 8820)], RATE), 900 / RATE)
})

test('findAudibleStart never skips more than the cap', () => {
  const cap = Math.floor(MAX_SILENCE_SKIP * RATE) / RATE
  // The sound starts after 100 ms: a quiet intro, not decoder priming.
  close(findAudibleStart([channel(4410, 8820)], RATE), cap)
  close(findAudibleStart([new Float32Array(8820)], RATE), cap)
})

test('findAudibleStart copes with a buffer shorter than the cap', () => {
  close(findAudibleStart([new Float32Array(100)], RATE), 100 / RATE)
  assert.equal(findAudibleStart([], RATE), 0)
})

test('playableSeconds clamps to what the buffer holds', () => {
  assert.equal(playableSeconds(0.1, 0.209, 0.025), 0.1)
  // The real preview is 29.988 s, so the 30 second step plays what there is.
  close(playableSeconds(30, 29.988, 0.025), 29.963)
  assert.equal(playableSeconds(1, 0.02, 0.06), 0)
})

test('clipLabel and clipShort', () => {
  assert.equal(clipLabel(0.1), '0.1 seconds')
  assert.equal(clipLabel(1), '1 second')
  assert.equal(clipLabel(30), '30 seconds')
  assert.equal(clipLabel(0.1 + 0.2), '0.3 seconds')
  assert.equal(clipShort(0.3), '0.3 s')
  assert.equal(clipShort(16), '16 s')
})

test('ladderFraction gives every step the same width', () => {
  LADDER.forEach((step, i) => close(ladderFraction(step, LADDER), (i + 1) / LADDER.length))
})

test('ladderFraction is linear inside a step and clamped outside', () => {
  assert.equal(ladderFraction(0, LADDER), 0)
  assert.equal(ladderFraction(-1, LADDER), 0)
  close(ladderFraction(0.05, LADDER), 0.5 / 7)
  // 2 s is half-way through the fourth step, 1 s to 3 s.
  close(ladderFraction(2, LADDER), 3.5 / 7)
  assert.equal(ladderFraction(45, LADDER), 1)
  assert.equal(ladderFraction(1, []), 0)
})
