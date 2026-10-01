// Run with `pnpm test` (plain `node --test`; Node strips the types itself).

import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  BURST_DUST,
  BURST_SECONDS,
  CALM_FEATHER,
  CALM_FLOOR,
  EDGE,
  IDLE_LIGHT,
  IDLE_PACE,
  MAX_STARS,
  METEOR_GAP,
  METEOR_SECONDS,
  MIN_STARS,
  PLAY_PACE,
  SKY_SEED,
  Sky,
  approach,
  calmFactor,
  dustPose,
  easeOutCubic,
  generateStars,
  hexChannels,
  makeDust,
  meteorDelay,
  meteorPose,
  planMeteor,
  rms,
  seededRandom,
  segmentHitsRect,
  smoothstep,
  starCount,
  starPoint,
  twinkle,
  wrap,
} from '../src/lib/sky.ts'
import type { Rect, SkyInput } from '../src/lib/sky.ts'

const IDLE: SkyInput = { playing: false, level: 0, avoid: [] }
const PLAYING: SkyInput = { playing: true, level: 0, avoid: [] }
const FRAME = 1 / 30

function close(actual: number, expected: number, tolerance = 1e-9): void {
  assert.ok(
    Math.abs(actual - expected) < tolerance,
    `${actual} is not ${expected}`,
  )
}

/** Steps a sky for `seconds` and returns how many shooting stars appeared. */
function run(sky: Sky, seconds: number, input: SkyInput): number {
  let seen = 0
  let before = new Set(sky.meteors)
  for (let t = 0; t < seconds; t += FRAME) {
    sky.step(FRAME, input)
    for (const meteor of sky.meteors) if (!before.has(meteor)) seen++
    before = new Set(sky.meteors)
  }
  return seen
}

test('seededRandom repeats for a seed and stays in [0, 1)', () => {
  const a = seededRandom(7)
  const b = seededRandom(7)
  const c = seededRandom(8)
  const first = Array.from({ length: 1000 }, a)
  assert.deepEqual(
    first,
    Array.from({ length: 1000 }, b),
  )
  assert.notDeepEqual(
    first.slice(0, 8),
    Array.from({ length: 8 }, c),
  )
  assert.ok(first.every((value) => value >= 0 && value < 1))
  // Spread out, not stuck in a corner.
  const mean = first.reduce((sum, value) => sum + value, 0) / first.length
  assert.ok(mean > 0.45 && mean < 0.55, `mean ${mean}`)
})

test('starCount scales with the viewport and stays within its limits', () => {
  assert.equal(starCount(390, 844), 120)
  assert.equal(starCount(320, 568), 66)
  assert.equal(starCount(1440, 900), MAX_STARS)
  assert.equal(starCount(3840, 2160), MAX_STARS)
  assert.equal(starCount(200, 200), MIN_STARS)
  assert.equal(starCount(0, 900), 0)
  assert.equal(starCount(Number.NaN, 900), 0)
})

test('generateStars gives the same sky every time', () => {
  assert.deepEqual(generateStars(120), generateStars(120, SKY_SEED))
  assert.notDeepEqual(generateStars(20, 1), generateStars(20, 2))
})

test('a larger sky keeps every star of a smaller one', () => {
  assert.deepEqual(generateStars(MAX_STARS).slice(0, 120), generateStars(120))
})

test('generateStars keeps every star within bounds', () => {
  const stars = generateStars(MAX_STARS)
  assert.equal(stars.length, MAX_STARS)
  for (const star of stars) {
    assert.ok(star.x >= 0 && star.x < 1 && star.y >= 0 && star.y < 1)
    assert.ok([0, 1, 2].includes(star.layer))
    assert.ok([0, 1, 2, 3].includes(star.tint))
    assert.ok(star.radius >= 0.6 && star.radius <= 2.1)
    assert.ok(star.alpha > 0.3 && star.alpha <= 1)
    assert.ok(star.period >= 2.8 && star.period <= 8.5)
    assert.ok(star.depth > 0 && star.depth < 0.6)
    if (star.halo > 0) {
      // Bright stars are near, warm or ivory, and larger than the rest.
      assert.equal(star.layer, 2)
      assert.ok(star.tint === 0 || star.tint === 3)
      assert.ok(star.radius >= 1.5)
    } else {
      assert.equal(star.sparkle, false)
    }
  }
})

test('the sky is mostly small far stars with a few bright ones', () => {
  for (const count of [120, MAX_STARS]) {
    const stars = generateStars(count)
    const far = stars.filter((star) => star.layer === 0).length
    const bright = stars.filter((star) => star.halo > 0)
    assert.ok(far > count * 0.4, `${far} far stars of ${count}`)
    assert.ok(
      bright.length >= count * 0.03 && bright.length <= count * 0.08,
      `${bright.length} bright stars of ${count}`,
    )
    assert.ok(bright.some((star) => star.sparkle))
    assert.ok(bright.some((star) => star.tint === 3))
  }
})

test('twinkle stays between the dip and full brightness, and moves', () => {
  for (const star of generateStars(40)) {
    let low = 1
    let high = 0
    for (let time = 0; time < 60; time += 0.05) {
      const value = twinkle(star, time)
      assert.ok(value >= 1 - star.depth - 1e-9 && value <= 1 + 1e-9)
      low = Math.min(low, value)
      high = Math.max(high, value)
    }
    assert.ok(high - low > star.depth * 0.6, 'the star barely twinkles')
  }
})

test('approach eases towards the target without overshooting', () => {
  assert.equal(approach(1, 5, 0, 0.9), 1)
  assert.equal(approach(1, 5, 1, 0), 5)
  close(approach(0, 1, 0.9, 0.9), 1 - Math.exp(-1))
  let value = 1
  let previous = value
  for (let i = 0; i < 600; i++) {
    value = approach(value, 5, FRAME, 0.9)
    assert.ok(value >= previous && value <= 5)
    previous = value
  }
  close(value, 5, 1e-6)
  // Two half steps land where one whole step does: frame rate does not change the motion.
  close(
    approach(approach(1, 5, 0.05, 0.9), 5, 0.05, 0.9),
    approach(1, 5, 0.1, 0.9),
  )
})

test('smoothstep and easeOutCubic are clamped and monotonic', () => {
  assert.equal(smoothstep(0, 10, -3), 0)
  assert.equal(smoothstep(0, 10, 5), 0.5)
  assert.equal(smoothstep(0, 10, 30), 1)
  assert.equal(easeOutCubic(-1), 0)
  assert.equal(easeOutCubic(0), 0)
  assert.equal(easeOutCubic(2), 1)
  let previous = 0
  for (let t = 0; t <= 1; t += 0.01) {
    assert.ok(easeOutCubic(t) >= previous)
    previous = easeOutCubic(t)
  }
  // Fast first, slow last.
  assert.ok(easeOutCubic(0.25) > 0.5)
})

test('wrap folds a position back onto the sheet', () => {
  assert.equal(wrap(10, 100, 20), 10)
  assert.equal(wrap(-30, 100, 20), 110)
  assert.equal(wrap(125, 100, 20), -15)
  assert.equal(wrap(5, 0, 0), 0)
  for (let value = -1000; value < 1000; value += 37.3) {
    const folded = wrap(value, 100, 20)
    assert.ok(folded >= -20 && folded < 120)
  }
})

test('starPoint keeps stars on the sheet however far the sky has drifted', () => {
  const stars = generateStars(MAX_STARS)
  const point = { x: 0, y: 0 }
  for (const drift of [0, 12.5, 4000, 1e6]) {
    for (const star of stars) {
      starPoint(star, 1440, 900, drift, 300, point)
      assert.ok(point.x >= -EDGE && point.x < 1440 + EDGE)
      assert.ok(point.y >= -EDGE && point.y < 900 + EDGE)
    }
  }
})

test('near stars drift and follow the scroll more than far ones', () => {
  const [star] = generateStars(1)
  const at = (layer: number, drift: number, scroll: number) =>
    starPoint(
      { ...star, x: 0.5, y: 0.5, layer },
      1440,
      900,
      drift,
      scroll,
      { x: 0, y: 0 },
    )
  const moved = (layer: number) => at(layer, 0, 0).x - at(layer, 10, 0).x
  assert.ok(moved(0) > 0, 'the sky drifts westward')
  assert.ok(moved(2) > moved(1) && moved(1) > moved(0))
  // Ten seconds at idle pace moves the nearest layer about a dozen pixels: calm.
  assert.ok(moved(2) < 15)
  const scrolled = (layer: number) => at(layer, 0, 0).y - at(layer, 0, 100).y
  assert.ok(scrolled(2) > scrolled(0) && scrolled(0) > 0)
  assert.ok(scrolled(2) < 10)
})

test('calmFactor dims stars behind text and fades out around it', () => {
  const column: Rect = { left: 800, top: 100, right: 1260, bottom: 800 }
  assert.equal(calmFactor(1000, 400, [column]), CALM_FLOOR)
  assert.equal(calmFactor(800, 100, [column]), CALM_FLOOR)
  assert.equal(calmFactor(800 - CALM_FEATHER, 400, [column]), 1)
  assert.equal(calmFactor(300, 400, [column]), 1)
  assert.equal(calmFactor(300, 400, []), 1)
  let previous = CALM_FLOOR
  for (let gap = 0; gap <= CALM_FEATHER; gap += 4) {
    const factor = calmFactor(800 - gap, 400, [column])
    assert.ok(factor >= previous && factor <= 1)
    previous = factor
  }
  // The dimmest of several rectangles wins.
  const header: Rect = { left: 0, top: 0, right: 400, bottom: 60 }
  assert.equal(calmFactor(200, 30, [column, header]), CALM_FLOOR)
  assert.equal(calmFactor(1000, 400, [header, column], 64, 0.5), 0.5)
})

test('segmentHitsRect', () => {
  const rect: Rect = { left: 10, top: 10, right: 20, bottom: 20 }
  assert.ok(segmentHitsRect(0, 15, 30, 15, rect)) // straight through
  assert.ok(segmentHitsRect(0, 0, 30, 30, rect)) // diagonal
  assert.ok(segmentHitsRect(12, 12, 18, 14, rect)) // wholly inside
  assert.ok(segmentHitsRect(15, 15, 40, 40, rect)) // starts inside
  assert.ok(!segmentHitsRect(0, 0, 30, 5, rect)) // passes above
  assert.ok(!segmentHitsRect(0, 15, 8, 15, rect)) // stops short
  assert.ok(!segmentHitsRect(25, 0, 25, 30, rect)) // vertical, to the right
  assert.ok(!segmentHitsRect(0, 25, 30, 25, rect)) // horizontal, below
  assert.ok(!segmentHitsRect(0, 18, 12, 30, rect)) // cuts past a corner
})

test('meteorDelay is 9 to 22 seconds', () => {
  const random = seededRandom(3)
  for (let i = 0; i < 500; i++) {
    const delay = meteorDelay(random)
    assert.ok(delay >= METEOR_GAP[0] && delay < METEOR_GAP[1])
  }
  assert.equal(meteorDelay(() => 0), 9)
})

test('planMeteor starts in the upper sky and falls briefly at a shallow angle', () => {
  const random = seededRandom(11)
  let left = 0
  for (let i = 0; i < 400; i++) {
    const meteor = planMeteor(random, 1440, 900, [])
    assert.ok(meteor)
    assert.ok(meteor.x >= 0.04 * 1440 && meteor.x <= 0.96 * 1440)
    assert.ok(meteor.y >= 0.03 * 900 && meteor.y <= 0.42 * 900)
    close(Math.hypot(meteor.dx, meteor.dy), 1)
    assert.ok(meteor.dy > 0, 'it falls')
    assert.ok(Math.abs(meteor.dx) > meteor.dy, 'shallower than 45 degrees')
    assert.ok(meteor.length >= 110 && meteor.length <= 360)
    assert.ok(
      meteor.duration >= METEOR_SECONDS[0] &&
        meteor.duration <= METEOR_SECONDS[1],
    )
    assert.equal(meteor.age, 0)
    if (meteor.dx < 0) left++
  }
  assert.ok(left > 120 && left < 280, 'both directions turn up')
  assert.equal(planMeteor(random, 0, 900, []), null)
})

test('planMeteor never crosses the game column', () => {
  // Side layout at 1440×900 and the stacked phone layout, column and header text.
  const layouts: [number, number, Rect[]][] = [
    [
      1440,
      900,
      [
        { left: 830, top: 150, right: 1294, bottom: 850 },
        { left: 40, top: 30, right: 330, bottom: 62 },
        { left: 1300, top: 30, right: 1400, bottom: 66 },
      ],
    ],
    [390, 844, [{ left: 20, top: 440, right: 370, bottom: 1200 }]],
  ]
  for (const [width, height, avoid] of layouts) {
    const random = seededRandom(width)
    let found = 0
    for (let i = 0; i < 500; i++) {
      const meteor = planMeteor(random, width, height, avoid)
      if (!meteor) continue
      found++
      const endX = meteor.x + meteor.dx * meteor.length
      const endY = meteor.y + meteor.dy * meteor.length
      for (const rect of avoid)
        assert.ok(!segmentHitsRect(meteor.x, meteor.y, endX, endY, rect))
    }
    assert.ok(found > 400, `${found} of 500 found a clear path`)
  }
  // With text everywhere there is no shooting star at all.
  const everywhere: Rect = { left: 0, top: 0, right: 390, bottom: 844 }
  assert.equal(planMeteor(seededRandom(5), 390, 844, [everywhere]), null)
})

test('the bright shooting star of a win is longer', () => {
  const plain = planMeteor(seededRandom(21), 1440, 900, [])
  const bright = planMeteor(seededRandom(21), 1440, 900, [], true)
  assert.ok(plain && bright)
  assert.equal(bright.bright, true)
  close(bright.length, plain.length * 1.35)
})

test('meteorPose: the head leads, the tail catches up, both ends are invisible', () => {
  assert.deepEqual(meteorPose(0), { head: 0, tail: 0, alpha: 0 })
  assert.deepEqual(meteorPose(1), { head: 1, tail: 1, alpha: 0 })
  assert.deepEqual(meteorPose(7), meteorPose(1))
  let previous = meteorPose(0)
  let brightest = 0
  for (let u = 0.01; u <= 1; u += 0.01) {
    const pose = meteorPose(u)
    assert.ok(pose.head >= previous.head && pose.tail >= previous.tail)
    assert.ok(pose.tail <= pose.head)
    assert.ok(pose.alpha >= 0 && pose.alpha <= 1)
    brightest = Math.max(brightest, pose.alpha)
    previous = pose
  }
  assert.equal(brightest, 1)
  // Half-way through, the streak is a good part of the path long.
  assert.ok(meteorPose(0.5).head - meteorPose(0.5).tail > 0.3)
})

test('makeDust starts at the edge of the record and fades within the burst', () => {
  const dust = makeDust(seededRandom(4), BURST_DUST, 200)
  assert.equal(dust.length, BURST_DUST)
  for (const grain of dust) {
    assert.ok(grain.from >= 180 && grain.from < 200)
    assert.ok(grain.travel >= 60 && grain.travel <= 320)
    assert.ok(grain.life >= 0.9 && grain.life <= BURST_SECONDS)
    assert.ok(grain.age <= 0 && grain.age > -0.2)
    assert.ok(grain.tint === 0 || grain.tint === 3)
    assert.equal(dustPose(grain).alpha, 0)
    const late = { ...grain, age: grain.life }
    assert.equal(dustPose(late).alpha, 0)
    close(dustPose(late).distance, grain.from + grain.travel)
    const mid = dustPose({ ...grain, age: grain.life * 0.3 })
    assert.ok(mid.alpha > 0.3 && mid.alpha <= 1)
    assert.ok(mid.distance > grain.from)
  }
  // Most grains settle near the record.
  const near = dust.filter((grain) => grain.travel < 200).length
  assert.ok(near > BURST_DUST * 0.6)
})

test('rms', () => {
  assert.equal(rms([]), 0)
  assert.equal(rms(new Float32Array(64)), 0)
  close(rms(new Float32Array(64).fill(0.5)), 0.5)
  close(rms([1, -1, 1, -1]), 1)
})

test('hexChannels reads hex colours and falls back for the rest', () => {
  assert.deepEqual(hexChannels('#d8b983', [0, 0, 0]), [216, 185, 131])
  assert.deepEqual(hexChannels(' #F3EFE7 ', [0, 0, 0]), [243, 239, 231])
  assert.deepEqual(hexChannels('#fa0', [0, 0, 0]), [255, 170, 0])
  assert.deepEqual(hexChannels('', [1, 2, 3]), [1, 2, 3])
  assert.deepEqual(hexChannels('rgb(1 2 3)', [9, 9, 9]), [9, 9, 9])
})

test('Sky shows the stars its viewport calls for, and the same ones each time', () => {
  const sky = new Sky()
  assert.equal(sky.stars.length, 0)
  sky.resize(390, 844)
  assert.deepEqual(sky.stars, generateStars(120))
  sky.resize(1440, 900)
  assert.equal(sky.stars.length, MAX_STARS)
  assert.deepEqual(sky.stars.slice(0, 120), generateStars(120))
  assert.deepEqual(new Sky(SKY_SEED, 99).stars, new Sky(SKY_SEED, 5).stars)
})

test('Sky eases its pace up while a clip plays and back down after', () => {
  const sky = new Sky()
  sky.resize(1440, 900)
  assert.equal(sky.pace, IDLE_PACE)
  assert.equal(sky.light, IDLE_LIGHT)
  sky.step(FRAME, IDLE)
  assert.equal(sky.pace, IDLE_PACE)
  assert.equal(sky.busy, false)

  sky.step(FRAME, PLAYING)
  // No jump: the first frame adds a small part of the difference.
  assert.ok(sky.pace > IDLE_PACE && sky.pace < IDLE_PACE + 0.2)
  assert.equal(sky.busy, true)
  let previous = sky.pace
  for (let t = 0; t < 6; t += FRAME) {
    sky.step(FRAME, PLAYING)
    assert.ok(sky.pace >= previous && sky.pace <= PLAY_PACE)
    previous = sky.pace
  }
  close(sky.pace, PLAY_PACE, 0.01)
  assert.ok(sky.light > IDLE_LIGHT)

  sky.step(FRAME, IDLE)
  assert.ok(sky.pace < PLAY_PACE && sky.pace > PLAY_PACE - 0.2)
  for (let t = 0; t < 12; t += FRAME) {
    sky.step(FRAME, IDLE)
    assert.ok(sky.pace <= previous && sky.pace >= IDLE_PACE)
    previous = sky.pace
  }
  close(sky.pace, IDLE_PACE, 0.01)
  close(sky.light, IDLE_LIGHT, 0.001)
})

test('Sky drifts with time and faster while playing', () => {
  const idle = new Sky()
  const playing = new Sky()
  idle.resize(1440, 900)
  playing.resize(1440, 900)
  for (let t = 0; t < 10; t += FRAME) {
    idle.step(FRAME, IDLE)
    playing.step(FRAME, PLAYING)
  }
  close(idle.drift, 10, 0.05)
  assert.ok(playing.drift > idle.drift * 3 && playing.drift < 10 * PLAY_PACE)
  assert.ok(playing.time > idle.time)
})

test('Sky breathes with the sound only while a clip plays', () => {
  const sky = new Sky()
  sky.resize(1440, 900)
  sky.step(FRAME, { playing: false, level: 0.3, avoid: [] })
  assert.equal(sky.pulse, 0)
  for (let i = 0; i < 30; i++)
    sky.step(FRAME, { playing: true, level: 0.2, avoid: [] })
  assert.ok(sky.pulse > 0.5 && sky.pulse <= 1)
  for (let i = 0; i < 30; i++)
    sky.step(FRAME, { playing: true, level: 5, avoid: [] })
  assert.ok(sky.pulse <= 1)
  for (let i = 0; i < 150; i++) sky.step(FRAME, IDLE)
  assert.equal(sky.pulse, 0)
})

test('Sky sends a shooting star every 9 to 22 seconds at idle, more often while playing', () => {
  for (const seed of [1, 2, 3, 4, 5]) {
    const idle = new Sky(SKY_SEED, seed)
    idle.resize(1440, 900)
    // The first comes within 11 seconds; after that, 9 to 22 seconds apart.
    assert.equal(run(idle, 11.5, IDLE), 1)
    const seen = run(idle, 600, IDLE)
    assert.ok(seen >= 26 && seen <= 67, `${seen} in ten minutes at idle`)

    const playing = new Sky(SKY_SEED, seed)
    playing.resize(1440, 900)
    const more = run(playing, 611.5, PLAYING)
    assert.ok(more > seen + 1, `${more} while playing, ${seen + 1} at idle`)
    assert.ok(more <= 124)
  }
})

test('Sky never shows two shooting stars at idle, and each one ends', () => {
  const sky = new Sky(SKY_SEED, 9)
  sky.resize(390, 844)
  for (let t = 0; t < 300; t += FRAME) {
    sky.step(FRAME, IDLE)
    assert.ok(sky.meteors.length <= 1)
    for (const meteor of sky.meteors) assert.ok(meteor.age < meteor.duration)
  }
})

test('Sky waits and tries again when no path is clear', () => {
  const sky = new Sky(SKY_SEED, 2)
  sky.resize(390, 844)
  const covered: SkyInput = {
    playing: false,
    level: 0,
    avoid: [{ left: 0, top: 0, right: 390, bottom: 844 }],
  }
  assert.equal(run(sky, 120, covered), 0)
  // Once the page scrolls back and the sky is clear, they return within a few seconds.
  assert.equal(run(sky, 4, IDLE), 1)
})

test('Sky.celebrate bursts once and is over in about two seconds', () => {
  const sky = new Sky(SKY_SEED, 6)
  sky.resize(1440, 900)
  sky.step(FRAME, IDLE)
  sky.celebrate(400, 480, 280)
  assert.equal(sky.dust.length, BURST_DUST)
  assert.deepEqual(sky.burst, { x: 400, y: 480 })
  assert.ok(sky.flare)
  assert.equal(sky.busy, true)
  const bright = sky.meteors.filter((meteor) => meteor.bright)
  assert.equal(bright.length, 1)
  assert.ok(bright[0].age < 0, 'the shooting star follows the burst')

  for (let t = 0; t < 1; t += FRAME) sky.step(FRAME, IDLE)
  assert.ok(sky.dust.length > 0 && sky.flare)
  for (let t = 0; t < 1.2; t += FRAME) sky.step(FRAME, IDLE)
  assert.equal(sky.dust.length, 0)
  assert.equal(sky.flare, null)
  assert.equal(sky.meteors.filter((meteor) => meteor.bright).length, 0)
})

test('Sky.settle clears what was passing and leaves the stars alone', () => {
  const sky = new Sky(SKY_SEED, 6)
  sky.resize(1440, 900)
  for (let i = 0; i < 20; i++)
    sky.step(FRAME, { playing: true, level: 0.3, avoid: [] })
  sky.celebrate(400, 480, 280)
  const { drift, time, pace, stars } = sky
  sky.settle()
  assert.equal(sky.meteors.length, 0)
  assert.equal(sky.dust.length, 0)
  assert.equal(sky.flare, null)
  assert.equal(sky.pulse, 0)
  assert.equal(sky.drift, drift)
  assert.equal(sky.time, time)
  assert.equal(sky.pace, pace)
  assert.equal(sky.stars, stars)
})
