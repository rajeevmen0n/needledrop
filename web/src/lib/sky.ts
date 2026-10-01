// The night sky behind the page, as numbers: where the stars are, how they
// twinkle and drift, when a shooting star crosses, and the stardust of a win.
// Pure: no DOM, no clock of its own, no Math.random. `starfield.ts` draws
// what this says; `Atmosphere.svelte` feeds it time.

const TAU = Math.PI * 2

/**
 * The sky every visitor sees. Change it and every star moves. Picked offline from
 * a few thousand for bright stars that are spread out rather than clumped.
 */
export const SKY_SEED = 1034

// How many stars: one per this many CSS pixels of viewport, within limits.
export const AREA_PER_STAR = 2750
export const MIN_STARS = 60
export const MAX_STARS = 280

/** Stars live on a sheet this much larger than the viewport on every side, so none pops in at an edge. */
export const EDGE = 24

// Three depths, far to near. Shares are cumulative.
const LAYER_SHARE = [0.5, 0.84]
const RADIUS: readonly (readonly [number, number])[] = [
  [0.6, 0.95],
  [0.8, 1.25],
  [1, 1.55],
]
const ALPHA: readonly (readonly [number, number])[] = [
  [0.34, 0.62],
  [0.5, 0.82],
  [0.7, 0.98],
]
/** Of the near stars, the share that is a bright one with a halo. */
const BRIGHT_SHARE = 0.34

/** CSS pixels a second each layer drifts at idle pace. Slow enough to miss while reading. */
export const LAYER_DRIFT: readonly number[] = [0.35, 0.7, 1.3]
/** Pixels each layer moves per pixel the page scrolls. */
export const LAYER_PARALLAX: readonly number[] = [0.015, 0.035, 0.07]
/** The whole sky slides this way: westward, sinking a little. */
const DRIFT_X = -0.99
const DRIFT_Y = 0.14

// Pace multiplies the drift. It eases between the two, never jumps.
export const IDLE_PACE = 1
export const PLAY_PACE = 5
const PACE_UP = 0.9
const PACE_DOWN = 1.7

// Overall brightness, and how far the sound can lift it.
export const IDLE_LIGHT = 0.9
export const PLAY_LIGHT = 1.08
export const PULSE_LIGHT = 0.3
const LIGHT_TIME = 0.8
const LEVEL_GAIN = 3
const PULSE_ATTACK = 0.07
const PULSE_RELEASE = 0.4

// Behind text the stars dim to this share of their light, over this many pixels.
export const CALM_FLOOR = 0.2
export const CALM_FEATHER = 64

// Shooting stars.
export const METEOR_GAP: readonly [number, number] = [9, 22]
/** While a clip plays the wait runs this much faster: about 5 to 12 seconds. */
export const METEOR_PLAY_RATE = 1.8
export const METEOR_SECONDS: readonly [number, number] = [0.7, 1.1]
/** A shooting star keeps this far from anything calm. */
const METEOR_CLEARANCE = 16
const METEOR_TRIES = 10
/** Seconds before trying again when no clear path was found. */
const METEOR_RETRY = 3

// The win.
export const BURST_DUST = 72
export const BURST_SECONDS = 1.6
const BURST_METEOR_DELAY = 0.3

export interface Rect {
  left: number
  top: number
  right: number
  bottom: number
}

export interface Point {
  x: number
  y: number
}

/** 0 ivory, 1 silver, 2 pale blue, 3 champagne. */
export type Tint = 0 | 1 | 2 | 3
export const TINTS = 4

export interface Star {
  /** Position on the sheet, 0 to 1. */
  x: number
  y: number
  /** 0 far, 1 middle, 2 near. */
  layer: number
  /** CSS pixels. */
  radius: number
  alpha: number
  tint: Tint
  /** Halo radius in CSS pixels; 0 for an ordinary star. */
  halo: number
  /** Whether the halo carries four diffraction spikes. */
  sparkle: boolean
  /** Twinkle: seconds per cycle, where in it the star starts, and how deep it dips (0 to 1). */
  period: number
  phase: number
  depth: number
}

export interface Meteor {
  x: number
  y: number
  /** Unit direction. */
  dx: number
  dy: number
  length: number
  duration: number
  /** Seconds since it appeared; negative while it waits. */
  age: number
  /** The one that follows a win. */
  bright: boolean
}

export interface Dust {
  angle: number
  /** Distance from the centre it starts at, and how much further it travels. */
  from: number
  travel: number
  size: number
  life: number
  /** Seconds since it appeared; negative while it waits. */
  age: number
  tint: Tint
}

export interface Flare {
  x: number
  y: number
  /** The record's radius. */
  radius: number
  age: number
}

export interface SkyInput {
  playing: boolean
  /** Loudness of the sound right now: RMS of the samples, 0 when silent. */
  level: number
  /** Where shooting stars may not go. */
  avoid: readonly Rect[]
}

/** mulberry32: small, fast, good enough for scattering stars. Returns numbers in [0, 1). */
export function seededRandom(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const lerp = (from: number, to: number, t: number) => from + (to - from) * t
const clamp = (value: number, low: number, high: number) =>
  Math.min(high, Math.max(low, value))

/** How many stars a viewport of this size gets: about 120 on a phone, 280 at most. */
export function starCount(width: number, height: number): number {
  if (!(width > 0) || !(height > 0)) return 0
  return clamp(Math.round((width * height) / AREA_PER_STAR), MIN_STARS, MAX_STARS)
}

/**
 * The first `count` stars of the sky with this seed. Every star takes the same
 * number of draws, so a larger sky is a smaller one plus more stars: resizing
 * the window never rearranges what is already there.
 */
export function generateStars(count: number, seed = SKY_SEED): Star[] {
  const random = seededRandom(seed)
  const stars: Star[] = []
  for (let i = 0; i < count; i++) {
    const x = random()
    const y = random()
    const depthRoll = random()
    const size = random()
    const light = random()
    const tintRoll = random()
    const brightRoll = random()
    const sparkleRoll = random()
    const period = lerp(2.8, 8.5, random())
    const phase = random() * TAU
    const dip = random()

    const layer = depthRoll < LAYER_SHARE[0] ? 0 : depthRoll < LAYER_SHARE[1] ? 1 : 2
    if (layer === 2 && brightRoll < BRIGHT_SHARE) {
      // A bright one: larger, warm, steady, with a soft halo.
      const radius = lerp(1.5, 2.1, size)
      stars.push({
        x,
        y,
        layer,
        radius,
        alpha: lerp(0.85, 1, light),
        tint: tintRoll < 0.3 ? 0 : 3,
        halo: radius * lerp(4.5, 7, light),
        sparkle: sparkleRoll < 0.5,
        period,
        phase,
        depth: lerp(0.12, 0.3, dip),
      })
      continue
    }
    const tint: Tint = tintRoll < 0.5 ? 0 : tintRoll < 0.76 ? 1 : tintRoll < 0.89 ? 2 : 3
    stars.push({
      x,
      y,
      layer,
      radius: lerp(RADIUS[layer][0], RADIUS[layer][1], size),
      alpha: lerp(ALPHA[layer][0], ALPHA[layer][1], light),
      tint,
      halo: 0,
      sparkle: false,
      period,
      phase,
      depth: lerp(0.2, 0.55, dip),
    })
  }
  return stars
}

/** A star's brightness at `time` seconds, as a share of its own: between `1 - depth` and 1. */
export function twinkle(star: Star, time: number): number {
  const angle = (time / star.period) * TAU + star.phase
  // Two sines that never line up twice the same way, so no star visibly loops.
  const wave = 0.5 + 0.35 * Math.sin(angle) + 0.15 * Math.sin(angle * 2.7 + star.phase * 3)
  return 1 - star.depth * wave
}

/**
 * Moves `value` towards `target` the way a warm filament follows a dimmer:
 * `tau` seconds to cover about 63% of what is left. Never overshoots.
 */
export function approach(value: number, target: number, dt: number, tau: number): number {
  if (dt <= 0) return value
  if (tau <= 0) return target
  return target + (value - target) * Math.exp(-dt / tau)
}

export function smoothstep(from: number, to: number, value: number): number {
  const t = clamp((value - from) / (to - from), 0, 1)
  return t * t * (3 - 2 * t)
}

export function easeOutCubic(t: number): number {
  const rest = 1 - clamp(t, 0, 1)
  return 1 - rest * rest * rest
}

/** `value` folded into `-margin` to `size + margin`. */
export function wrap(value: number, size: number, margin: number): number {
  const span = size + margin * 2
  if (!(span > 0)) return 0
  return ((((value + margin) % span) + span) % span) - margin
}

/**
 * Where a star is on screen: its place on the sheet, moved by the drift
 * (`drift` is the pixels a layer of speed 1 has travelled) and by the page
 * scroll, wrapped around the edges. Writes into `out` and returns it.
 */
export function starPoint(
  star: Star,
  width: number,
  height: number,
  drift: number,
  scroll: number,
  out: Point,
): Point {
  const travelled = drift * LAYER_DRIFT[star.layer]
  out.x = wrap(star.x * (width + EDGE * 2) - EDGE + travelled * DRIFT_X, width, EDGE)
  out.y = wrap(
    star.y * (height + EDGE * 2) - EDGE + travelled * DRIFT_Y - scroll * LAYER_PARALLAX[star.layer],
    height,
    EDGE,
  )
  return out
}

/**
 * How much of its light a star at this point keeps: `floor` inside any of the
 * rectangles, 1 once `feather` pixels clear of all of them, smooth in between.
 */
export function calmFactor(
  x: number,
  y: number,
  rects: readonly Rect[],
  feather = CALM_FEATHER,
  floor = CALM_FLOOR,
): number {
  let factor = 1
  for (const rect of rects) {
    const dx = Math.max(rect.left - x, 0, x - rect.right)
    const dy = Math.max(rect.top - y, 0, y - rect.bottom)
    if (dx >= feather || dy >= feather) continue
    const here = lerp(floor, 1, smoothstep(0, feather, Math.hypot(dx, dy)))
    if (here < factor) factor = here
  }
  return factor
}

/** Whether the segment touches the rectangle (Liang–Barsky). */
export function segmentHitsRect(
  x0: number,
  y0: number,
  x1: number,
  y1: number,
  rect: Rect,
): boolean {
  const dx = x1 - x0
  const dy = y1 - y0
  let enter = 0
  let leave = 1
  const edges: readonly (readonly [number, number])[] = [
    [-dx, x0 - rect.left],
    [dx, rect.right - x0],
    [-dy, y0 - rect.top],
    [dy, rect.bottom - y0],
  ]
  for (const [p, q] of edges) {
    if (p === 0) {
      if (q < 0) return false
      continue
    }
    const t = q / p
    if (p < 0) {
      if (t > leave) return false
      if (t > enter) enter = t
    } else {
      if (t < enter) return false
      if (t < leave) leave = t
    }
  }
  return true
}

/** Seconds until the next shooting star at idle pace. */
export function meteorDelay(random: () => number): number {
  return lerp(METEOR_GAP[0], METEOR_GAP[1], random())
}

/**
 * A shooting star for a sky of this size: it starts in the upper sky and falls
 * at a shallow angle, left or right. Paths that would come near an `avoid`
 * rectangle are thrown away; `null` when no clear path turned up.
 */
export function planMeteor(
  random: () => number,
  width: number,
  height: number,
  avoid: readonly Rect[],
  bright = false,
): Meteor | null {
  if (!(width > 0) || !(height > 0)) return null
  for (let i = 0; i < METEOR_TRIES; i++) {
    const x = lerp(0.04, 0.96, random()) * width
    const y = lerp(0.03, 0.42, random()) * height
    const side = random() < 0.5 ? -1 : 1
    const slope = (lerp(12, 38, random()) * Math.PI) / 180
    const reach = clamp(lerp(0.2, 0.42, random()) * Math.min(width, height), 110, 360)
    const duration = lerp(METEOR_SECONDS[0], METEOR_SECONDS[1], random())
    const length = bright ? reach * 1.35 : reach
    const dx = side * Math.cos(slope)
    const dy = Math.sin(slope)
    const endX = x + dx * length
    const endY = y + dy * length
    const blocked = avoid.some((rect) =>
      segmentHitsRect(x, y, endX, endY, {
        left: rect.left - METEOR_CLEARANCE,
        top: rect.top - METEOR_CLEARANCE,
        right: rect.right + METEOR_CLEARANCE,
        bottom: rect.bottom + METEOR_CLEARANCE,
      }),
    )
    if (!blocked) return { x, y, dx, dy, length, duration, age: 0, bright }
  }
  return null
}

/**
 * A shooting star `progress` (0 to 1) through its life: how far along its path
 * the head and the end of the tail are (0 to 1), and how visible it is. The
 * head starts fast and slows; the tail leaves late and catches up at the end.
 */
export function meteorPose(progress: number): { head: number; tail: number; alpha: number } {
  const u = clamp(progress, 0, 1)
  const ease = (t: number) => 1 - (1 - t) * (1 - t)
  return {
    head: ease(u),
    tail: ease(Math.max(0, (u - 0.34) / 0.66)),
    alpha: smoothstep(0, 0.12, u) * (1 - smoothstep(0.62, 1, u)),
  }
}

/** The stardust of a win, around a record of this radius. */
export function makeDust(random: () => number, count: number, radius: number): Dust[] {
  const dust: Dust[] = []
  for (let i = 0; i < count; i++) {
    const far = random()
    dust.push({
      angle: random() * TAU,
      // From just behind the record's edge, so it appears to come from under it.
      from: radius * lerp(0.9, 0.99, random()),
      // Most settles close by; a few grains fly far.
      travel: radius * lerp(0.3, 1.6, far * Math.sqrt(far)),
      size: lerp(0.5, 1.6, random()),
      life: lerp(0.9, BURST_SECONDS, random()),
      age: -lerp(0, 0.16, random()),
      tint: random() < 0.65 ? 3 : 0,
    })
  }
  return dust
}

/** Where a grain is (pixels from the centre) and how visible, at its age. */
export function dustPose(grain: Dust): { distance: number; alpha: number } {
  const u = clamp(grain.age / grain.life, 0, 1)
  const fade = 1 - u
  return {
    distance: grain.from + grain.travel * easeOutCubic(u),
    alpha: grain.age < 0 ? 0 : smoothstep(0, 0.1, u) * fade * Math.sqrt(fade),
  }
}

/** Root mean square of audio samples; 0 for none. */
export function rms(samples: ArrayLike<number>): number {
  if (samples.length === 0) return 0
  let sum = 0
  for (let i = 0; i < samples.length; i++) sum += samples[i] * samples[i]
  return Math.sqrt(sum / samples.length)
}

/** Red, green and blue of a `#rgb` or `#rrggbb` colour; `fallback` for anything else. */
export function hexChannels(
  color: string,
  fallback: readonly [number, number, number],
): [number, number, number] {
  const match = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(color.trim())
  if (!match) return [fallback[0], fallback[1], fallback[2]]
  const hex = match[1]
  const wide = hex.length === 3 ? [...hex].map((digit) => digit + digit).join('') : hex
  return [
    parseInt(wide.slice(0, 2), 16),
    parseInt(wide.slice(2, 4), 16),
    parseInt(wide.slice(4, 6), 16),
  ]
}

/**
 * The sky's moving parts. `step` advances it; everything it does follows from
 * the two seeds and the calls made, so a test can replay it.
 */
export class Sky {
  width = 0
  height = 0
  /** The stars this viewport shows: the first `starCount` of `all`. */
  stars: Star[] = []
  /** The twinkle clock, in seconds. */
  time = 0
  /** Pixels a layer of speed 1 has drifted. */
  drift = 0
  pace = IDLE_PACE
  light = IDLE_LIGHT
  /** The sound's loudness, smoothed, 0 to 1. */
  pulse = 0
  meteors: Meteor[] = []
  dust: Dust[] = []
  /** Where the stardust radiates from. */
  burst: Point = { x: 0, y: 0 }
  flare: Flare | null = null

  private readonly all: Star[]
  private readonly random: () => number
  private untilMeteor: number
  private playing = false
  private avoid: readonly Rect[] = []

  /** `seed` places the stars; `eventSeed` decides when and where shooting stars fall. */
  constructor(seed = SKY_SEED, eventSeed = 1) {
    this.all = generateStars(MAX_STARS, seed)
    this.random = seededRandom(eventSeed)
    // The first one comes a little sooner, so a short visit still sees one.
    this.untilMeteor = meteorDelay(this.random) * 0.5
  }

  resize(width: number, height: number): void {
    this.width = width
    this.height = height
    this.stars = this.all.slice(0, starCount(width, height))
  }

  /** True while something is moving that deserves every frame; otherwise half the frames will do. */
  get busy(): boolean {
    const target = this.playing ? PLAY_PACE : IDLE_PACE
    return (
      this.playing ||
      this.meteors.length > 0 ||
      this.dust.length > 0 ||
      this.flare !== null ||
      Math.abs(this.pace - target) > 0.05 ||
      this.pulse > 0.01
    )
  }

  step(dt: number, input: SkyInput): void {
    this.playing = input.playing
    this.avoid = input.avoid

    this.pace = approach(
      this.pace,
      input.playing ? PLAY_PACE : IDLE_PACE,
      dt,
      input.playing ? PACE_UP : PACE_DOWN,
    )
    this.drift += this.pace * dt
    // The twinkle quickens a little with the drift.
    this.time += dt * (1 + (0.4 * (this.pace - IDLE_PACE)) / (PLAY_PACE - IDLE_PACE))
    this.light = approach(this.light, input.playing ? PLAY_LIGHT : IDLE_LIGHT, dt, LIGHT_TIME)
    const level = input.playing ? clamp(input.level * LEVEL_GAIN, 0, 1) : 0
    this.pulse = approach(this.pulse, level, dt, level > this.pulse ? PULSE_ATTACK : PULSE_RELEASE)
    if (this.pulse < 0.001 && level === 0) this.pulse = 0

    this.untilMeteor -= dt * (input.playing ? METEOR_PLAY_RATE : 1)
    if (this.untilMeteor <= 0) {
      const meteor = planMeteor(this.random, this.width, this.height, input.avoid)
      if (meteor) this.meteors.push(meteor)
      this.untilMeteor = meteor ? meteorDelay(this.random) : METEOR_RETRY
    }
    if (this.meteors.length > 0) {
      for (const meteor of this.meteors) meteor.age += dt
      this.meteors = this.meteors.filter((meteor) => meteor.age < meteor.duration)
    }
    if (this.dust.length > 0) {
      for (const grain of this.dust) grain.age += dt
      this.dust = this.dust.filter((grain) => grain.age < grain.life)
    }
    if (this.flare) {
      this.flare.age += dt
      if (this.flare.age >= BURST_SECONDS) this.flare = null
    }
  }

  /** The win: stardust and a flare from behind the record at this point, then one bright shooting star. */
  celebrate(x: number, y: number, radius: number): void {
    this.dust = makeDust(this.random, BURST_DUST, radius)
    this.burst = { x, y }
    this.flare = { x, y, radius, age: 0 }
    const meteor = planMeteor(this.random, this.width, this.height, this.avoid, true)
    if (meteor) {
      meteor.age = -BURST_METEOR_DELAY
      this.meteors.push(meteor)
    }
  }

  /** For a held frame: the stars stay put, and whatever was passing through is gone. */
  settle(): void {
    if (this.meteors.length > 0) this.meteors = []
    if (this.dust.length > 0) this.dust = []
    this.flare = null
    this.pulse = 0
  }
}
