// Paints the night sky on a 2D canvas: stars at three depths, the halos and
// diffraction spikes of the bright ones, shooting stars, and the stardust and
// flare of a win. Framework-free. `paint(sky, view)` draws exactly what the
// `Sky` says (see `sky.ts`, which holds the model and all the motion); the only
// things kept between frames are the pre-rendered glow sprites.

import {
  BURST_SECONDS,
  PULSE_LIGHT,
  TINTS,
  calmFactor,
  dustPose,
  easeOutCubic,
  hexChannels,
  meteorPose,
  smoothstep,
  starPoint,
  twinkle,
} from './sky'
import type { Point, Rect, Sky } from './sky'

/** The four star colours, as the stylesheet has them. Indexed by `Tint`. */
export interface SkyPalette {
  ivory: string
  silver: string
  cool: string
  champagne: string
}

export interface SkyGeometry {
  /** Size of the canvas in CSS pixels. */
  width: number
  height: number
  /** Backing-store pixels per CSS pixel. */
  scale: number
}

/** What the page around the sky looks like for this frame. */
export interface SkyView {
  /** Rectangles with text in them, in canvas coordinates: stars dim behind these. */
  calm: readonly Rect[]
  /** Pixels the page has scrolled, for the parallax; 0 to leave the sky where it is. */
  scroll: number
}

const TAU = Math.PI * 2

// Sprites, in backing-store pixels. Drawn once per palette, scaled per star.
const HALO_SPRITE = 64
const SPARK_SPRITE = 128
/** Half the thickness of a spike at its root, as a fraction of the sprite. */
const SPIKE = 0.012

/** A halo and its spikes at their strongest, as a share of the star's own light. */
const HALO_ALPHA = 0.5
const SPARK_ALPHA = 0.42
/** How far the spikes reach, in halo radii. */
const SPARK_REACH = 2.6
/** Anything fainter than this is not worth a draw call. */
const FAINTEST = 0.012

// Shooting stars, in CSS pixels.
const METEOR_WIDTH = 1.3
const METEOR_WIDTH_BRIGHT = 1.9
const METEOR_GLINT = 6
const METEOR_GLINT_BRIGHT = 9
/** An ordinary shooting star is a little dimmer than the one that follows a win. */
const METEOR_ALPHA = 0.8

/** The flare behind the record at its brightest, and how far it spreads, in record radii. */
const FLARE_ALPHA = 0.26
const FLARE_FROM = 1.5
const FLARE_GROWTH = 1.3

/** Used when a token is missing or is not a hex colour: ivory, silver, pale blue, champagne. */
const FALLBACK: readonly (readonly [number, number, number])[] = [
  [243, 239, 231],
  [150, 153, 158],
  [199, 210, 232],
  [216, 185, 131],
]

type Channels = readonly [number, number, number]

const ink = ([r, g, b]: Channels, alpha: number) => `rgb(${r} ${g} ${b} / ${alpha})`

function sprite(size: number): [HTMLCanvasElement, CanvasRenderingContext2D] | null {
  const canvas = document.createElement('canvas')
  canvas.width = size
  canvas.height = size
  const ctx = canvas.getContext('2d')
  return ctx ? [canvas, ctx] : null
}

/** A soft round glow: a bright heart and a wide, faint skirt. */
function haloSprite(color: Channels): HTMLCanvasElement | null {
  const made = sprite(HALO_SPRITE)
  if (!made) return null
  const [canvas, ctx] = made
  const c = HALO_SPRITE / 2
  const glow = ctx.createRadialGradient(c, c, 0, c, c, c)
  glow.addColorStop(0, ink(color, 0.95))
  glow.addColorStop(0.1, ink(color, 0.5))
  glow.addColorStop(0.3, ink(color, 0.16))
  glow.addColorStop(0.6, ink(color, 0.04))
  glow.addColorStop(1, ink(color, 0))
  ctx.fillStyle = glow
  ctx.fillRect(0, 0, HALO_SPRITE, HALO_SPRITE)
  return canvas
}

/** Four thin spikes, as a lens makes of a bright point. */
function sparkSprite(color: Channels): HTMLCanvasElement | null {
  const made = sprite(SPARK_SPRITE)
  if (!made) return null
  const [canvas, ctx] = made
  const size = SPARK_SPRITE
  const c = size / 2
  const t = size * SPIKE
  const fade = ctx.createRadialGradient(c, c, 0, c, c, c)
  fade.addColorStop(0, ink(color, 0.9))
  fade.addColorStop(0.45, ink(color, 0.22))
  fade.addColorStop(1, ink(color, 0))
  ctx.fillStyle = fade
  ctx.beginPath()
  ctx.moveTo(0, c)
  ctx.lineTo(c, c - t)
  ctx.lineTo(size, c)
  ctx.lineTo(c, c + t)
  ctx.closePath()
  ctx.moveTo(c, 0)
  ctx.lineTo(c + t, c)
  ctx.lineTo(c, size)
  ctx.lineTo(c - t, c)
  ctx.closePath()
  ctx.fill()
  return canvas
}

export class StarfieldPainter {
  private readonly canvas: HTMLCanvasElement
  private readonly ctx: CanvasRenderingContext2D
  private geometry: SkyGeometry = { width: 0, height: 0, scale: 1 }
  private channels: Channels[] = FALLBACK.map((color) => color)
  private solid: string[] = FALLBACK.map((color) => ink(color, 1))
  private halos: (HTMLCanvasElement | null)[] = []
  private sparks: (HTMLCanvasElement | null)[] = []
  private readonly point: Point = { x: 0, y: 0 }

  constructor(canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new Error('no 2D canvas')
    this.canvas = canvas
    this.ctx = ctx
    this.render()
  }

  setPalette(palette: SkyPalette): void {
    const colors = [palette.ivory, palette.silver, palette.cool, palette.champagne]
    this.channels = colors.map((color, i) => hexChannels(color, FALLBACK[i]))
    this.solid = this.channels.map((color) => ink(color, 1))
    this.render()
  }

  /** Sizes the backing store. Cheap to call with what it already has. */
  configure(geometry: SkyGeometry): void {
    this.geometry = geometry
    const width = Math.max(1, Math.round(geometry.width * geometry.scale))
    const height = Math.max(1, Math.round(geometry.height * geometry.scale))
    if (this.canvas.width !== width) this.canvas.width = width
    if (this.canvas.height !== height) this.canvas.height = height
  }

  private render(): void {
    this.halos = this.channels.map(haloSprite)
    this.sparks = this.channels.map(sparkSprite)
  }

  paint(sky: Sky, view: SkyView): void {
    const { ctx } = this
    const { width, height, scale } = this.geometry
    ctx.setTransform(scale, 0, 0, scale, 0, 0)
    ctx.clearRect(0, 0, width, height)
    if (width <= 0 || height <= 0) return

    this.paintFlare(sky)
    this.paintStars(sky, view)
    this.paintDust(sky)
    this.paintMeteors(sky, view)
    ctx.globalAlpha = 1
  }

  private paintStars(sky: Sky, view: SkyView): void {
    const { ctx, point } = this
    const { width, height } = this.geometry
    const light = sky.light + PULSE_LIGHT * sky.pulse
    // One pass per colour, so the fill style changes four times a frame rather than once a star.
    for (let tint = 0; tint < TINTS; tint++) {
      ctx.fillStyle = this.solid[tint]
      const halo = this.halos[tint]
      const spark = this.sparks[tint]
      for (const star of sky.stars) {
        if (star.tint !== tint) continue
        starPoint(star, width, height, sky.drift, view.scroll, point)
        const shine = twinkle(star, sky.time)
        const alpha =
          Math.min(1, star.alpha * shine * light) *
          calmFactor(point.x, point.y, view.calm)
        if (alpha < FAINTEST) continue
        ctx.globalAlpha = alpha
        ctx.beginPath()
        ctx.arc(point.x, point.y, star.radius, 0, TAU)
        ctx.fill()
        if (star.halo <= 0) continue
        if (halo) {
          ctx.globalAlpha = alpha * HALO_ALPHA
          ctx.drawImage(
            halo,
            point.x - star.halo,
            point.y - star.halo,
            star.halo * 2,
            star.halo * 2,
          )
        }
        if (star.sparkle && spark) {
          // The spikes flicker more than the star does.
          const reach = star.halo * SPARK_REACH
          ctx.globalAlpha = alpha * SPARK_ALPHA * shine * shine
          ctx.drawImage(spark, point.x - reach, point.y - reach, reach * 2, reach * 2)
        }
      }
    }
  }

  /** Light spreading from behind the record. The record itself is drawn over it by the page. */
  private paintFlare(sky: Sky): void {
    const flare = sky.flare
    if (!flare) return
    const { ctx } = this
    const u = Math.min(flare.age / BURST_SECONDS, 1)
    const reach = flare.radius * (FLARE_FROM + FLARE_GROWTH * easeOutCubic(u))
    const alpha = FLARE_ALPHA * smoothstep(0, 0.1, u) * (1 - u) * (1 - u)
    if (alpha < FAINTEST / 4) return
    const warm = this.channels[3]
    const glow = ctx.createRadialGradient(
      flare.x,
      flare.y,
      flare.radius * 0.85,
      flare.x,
      flare.y,
      reach,
    )
    glow.addColorStop(0, ink(warm, alpha))
    glow.addColorStop(0.35, ink(warm, alpha * 0.4))
    glow.addColorStop(1, ink(warm, 0))
    ctx.globalAlpha = 1
    ctx.fillStyle = glow
    ctx.beginPath()
    ctx.arc(flare.x, flare.y, reach, 0, TAU)
    ctx.fill()
  }

  private paintDust(sky: Sky): void {
    const { ctx } = this
    for (const grain of sky.dust) {
      const pose = dustPose(grain)
      if (pose.alpha < FAINTEST) continue
      ctx.globalAlpha = pose.alpha
      ctx.fillStyle = this.solid[grain.tint]
      ctx.beginPath()
      ctx.arc(
        sky.burst.x + Math.cos(grain.angle) * pose.distance,
        sky.burst.y + Math.sin(grain.angle) * pose.distance,
        grain.size,
        0,
        TAU,
      )
      ctx.fill()
    }
  }

  private paintMeteors(sky: Sky, view: SkyView): void {
    const { ctx } = this
    const warm = this.channels[3]
    const glint = this.halos[0]
    for (const meteor of sky.meteors) {
      if (meteor.age < 0) continue
      const pose = meteorPose(meteor.age / meteor.duration)
      const headX = meteor.x + meteor.dx * meteor.length * pose.head
      const headY = meteor.y + meteor.dy * meteor.length * pose.head
      const tailX = meteor.x + meteor.dx * meteor.length * pose.tail
      const tailY = meteor.y + meteor.dy * meteor.length * pose.tail
      const alpha =
        pose.alpha *
        (meteor.bright ? 1 : METEOR_ALPHA) *
        calmFactor(headX, headY, view.calm)
      if (alpha < FAINTEST || pose.head - pose.tail < 1e-4) continue

      const streak = ctx.createLinearGradient(tailX, tailY, headX, headY)
      streak.addColorStop(0, ink(warm, 0))
      streak.addColorStop(0.75, ink(warm, 0.6))
      streak.addColorStop(1, ink(this.channels[0], 1))
      ctx.globalAlpha = alpha
      ctx.strokeStyle = streak
      ctx.lineWidth = meteor.bright ? METEOR_WIDTH_BRIGHT : METEOR_WIDTH
      ctx.lineCap = 'round'
      ctx.beginPath()
      ctx.moveTo(tailX, tailY)
      ctx.lineTo(headX, headY)
      ctx.stroke()
      if (glint) {
        const size = meteor.bright ? METEOR_GLINT_BRIGHT : METEOR_GLINT
        ctx.globalAlpha = alpha * 0.8
        ctx.drawImage(glint, headX - size, headY - size, size * 2, size * 2)
      }
    }
  }
}
