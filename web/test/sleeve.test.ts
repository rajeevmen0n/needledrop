// Run with `pnpm test` (plain `node --test`; Node strips the types itself).

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { SLEEVE_COUNT, sleeveForDay, sleeveOverride } from '../src/lib/sleeve.ts'

test('sleeveForDay is the UTC weekday, Sunday first', () => {
  assert.equal(sleeveForDay('2026-10-01'), 4) // a Thursday
  assert.equal(sleeveForDay('2026-10-04'), 0)
  assert.equal(sleeveForDay('2026-10-10'), 6)
  // A whole week uses every sleeve once.
  const week = Array.from({ length: 7 }, (_, i) => sleeveForDay(`2026-10-0${i + 1}`))
  assert.deepEqual([...week].sort(), [0, 1, 2, 3, 4, 5, 6])
})

test('sleeveForDay refuses text that is not a date', () => {
  assert.equal(sleeveForDay(''), null)
  assert.equal(sleeveForDay('today'), null)
  assert.equal(sleeveForDay('2026-13-01'), null)
  assert.equal(sleeveForDay('2026-10-01T12:00:00Z'), null)
})

test('sleeveOverride reads ?sleeve=0..6 and nothing else', () => {
  assert.equal(sleeveOverride('?sleeve=0'), 0)
  assert.equal(sleeveOverride('?a=1&sleeve=6'), 6)
  assert.equal(sleeveOverride(''), null)
  assert.equal(sleeveOverride('?sleeve='), null)
  assert.equal(sleeveOverride(`?sleeve=${SLEEVE_COUNT}`), null)
  assert.equal(sleeveOverride('?sleeve=-1'), null)
  assert.equal(sleeveOverride('?sleeve=2.5'), null)
  assert.equal(sleeveOverride('?sleeve=blue'), null)
})
