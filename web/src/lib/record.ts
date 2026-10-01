// Paints the record on a 2D canvas: vinyl, the seven groove bands, the sheen,
// the label and the tonearm. Framework-free. `paint(view)` draws exactly what
// the view says; the only things kept between frames are cached layers.

/** The colours and the font, as the stylesheet has them. */
export interface Palette {
  field: string
  accent: string
  paper: string
  ink: string
  vinyl: string
  /** Text on bare vinyl. */
  vinylText: string
  /** A CSS font-family list. */
  font: string
}

export interface RecordGeometry {
  /** Side of the square canvas in CSS pixels. */
  size: number
  /** Backing-store pixels per CSS pixel. */
  scale: number
  /** Tonearm turned a quarter clockwise, for the layout that shows the record's lower half. */
  turned: boolean
  /** Radians clockwise from 3 o'clock: the direction the clip lengths are printed along. */
  scaleAngle: number
}

/** Everything one frame shows. */
export interface RecordView {
  /** One per band, outermost first: 0 locked, 1 lit. */
  lit: readonly number[]
  /** The clip length printed on each band. */
  labels: readonly string[]
  /** Radians the record has turned. */
  rotation: number
  /** Where the needle is across the bands: 0 at the outer edge, 1 at the inner. */
  needle: number
  /** Time-domain samples that ripple the lit grooves; `null` for still grooves. */
  wave: Float32Array | null
  /** Stamped on the blank label. */
  stamp: string
  /** The album cover, once loaded. */
  cover: HTMLImageElement | null
  /** How much of the cover is printed on the label, 0 to 1. */
  printed: number
  /** A progress ring around the label, 0 to 1, for when nothing may move; `null` hides it. */
  arc: number | null
}

/** The canvas is a square this many record radii wide, so the tonearm fits beside the disc. */
export const CANVAS_RADII = 2.24

const TAU = Math.PI * 2

// Radii as fractions of the record's radius.
const BANDS_OUTER = 0.955
const BANDS_INNER = 0.385
const BAND_GAP = 0.01
const LABEL = 0.3
const HOLE = 0.022

// The tonearm, in record radii from the spindle, before any quarter turn.
const PIVOT = { x: 0.8, y: -0.86 }
const ARM_LENGTH = 1.13

/** Where the cover starts printing: under the needle's side of the label. */
const PRINT_HEAD = 0.27

interface Point {
  x: number
  y: number
}

/** Outer and inner radius of a band, as fractions of the record's radius. Band 0 is the outermost. */
function bandEdges(index: number, count: number): [number, number] {
  const width = (BANDS_OUTER - BANDS_INNER) / count
  const outer = BANDS_OUTER - index * width
  return [outer, outer - width + BAND_GAP]
}

// Small seeded generator: the grooves must look the same on every repaint.
function random(seed: number): () => number {
  let state = seed
  return () => {
    state = (state + 0x6d2b79f5) | 0
    let t = Math.imul(state ^ (state >>> 15), 1 | state)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

function ring(ctx: CanvasRenderingContext2D, outer: number, inner: number): void {
  ctx.beginPath()
  ctx.arc(0, 0, outer, 0, TAU)
  ctx.arc(0, 0, inner, 0, TAU, true)
  ctx.closePath()
}

export class RecordPainter {
  private readonly canvas: HTMLCanvasElement
  private readonly ctx: CanvasRenderingContext2D
  private palette: Palette | null = null
  private geometry: RecordGeometry = { size: 0, scale: 1, turned: false, scaleAngle: 0 }

  // Layers that only change with the size or the colours.
  private base: HTMLCanvasElement | null = null
  private sheen: HTMLCanvasElement | null = null
  private label: HTMLCanvasElement | null = null
  private stamped = ''

  constructor(canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new Error('No 2D canvas')
    this.canvas = canvas
    this.ctx = ctx
  }

  /** Sets the size and layout. Cheap to call with unchanged values. */
  configure(geometry: RecordGeometry): void {
    const previous = this.geometry
    this.geometry = geometry
    if (previous.size === geometry.size && previous.scale === geometry.scale) return
    const pixels = Math.max(1, Math.round(geometry.size * geometry.scale))
    this.canvas.width = pixels
    this.canvas.height = pixels
    this.invalidate()
  }

  setPalette(palette: Palette): void {
    this.palette = palette
    this.invalidate()
  }

  /** Drops the cached layers, e.g. once the font has loaded. */
  invalidate(): void {
    this.base = null
    this.sheen = null
    this.label = null
  }

  paint(view: RecordView): void {
    const { ctx, palette } = this
    const { size, scale } = this.geometry
    if (!palette || size === 0) return
    const radius = size / CANVAS_RADII
    const centre = size / 2

    ctx.setTransform(scale, 0, 0, scale, 0, 0)
    ctx.clearRect(0, 0, size, size)
    ctx.drawImage(this.baseLayer(palette), 0, 0, size, size)

    // From here on the spindle is the origin.
    ctx.translate(centre, centre)
    this.paintBands(view, palette, radius)
    this.paintScale(view, palette, radius)

    // The light stays put while the record turns; a slight rock stands in for the warp of a real disc.
    ctx.save()
    ctx.rotate(Math.sin(view.rotation) * 0.03)
    ctx.drawImage(this.sheenLayer(), -centre, -centre, size, size)
    ctx.restore()

    this.paintLabel(view, palette, radius)
    if (view.arc !== null) this.paintArc(view.arc, palette, radius)
    this.paintArm(view.needle, palette, radius)
  }

  private layer(): [HTMLCanvasElement, CanvasRenderingContext2D] {
    const { size, scale } = this.geometry
    const canvas = document.createElement('canvas')
    canvas.width = this.canvas.width
    canvas.height = this.canvas.height
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new Error('No 2D canvas')
    ctx.setTransform(scale, 0, 0, scale, size * scale * 0.5, size * scale * 0.5)
    return [canvas, ctx]
  }

  /** True where grooves are cut: inside a band, not in the smooth gaps between them. */
  private grooved(fraction: number, count: number): boolean {
    if (fraction > BANDS_OUTER || fraction < BANDS_INNER + BAND_GAP) return false
    const width = (BANDS_OUTER - BANDS_INNER) / count
    return (BANDS_OUTER - fraction) % width < width - BAND_GAP
  }

  // The disc itself: its shadow on the sleeve, black vinyl, fine grooves.
  private baseLayer(palette: Palette): HTMLCanvasElement {
    if (this.base) return this.base
    const [canvas, ctx] = this.layer()
    const { size, scale } = this.geometry
    const radius = size / CANVAS_RADII

    ctx.save()
    ctx.shadowColor = palette.ink
    ctx.shadowBlur = radius * 0.06 * scale
    ctx.shadowOffsetX = radius * 0.012 * scale
    ctx.shadowOffsetY = radius * 0.024 * scale
    ctx.fillStyle = palette.vinyl
    ctx.beginPath()
    ctx.arc(0, 0, radius, 0, TAU)
    ctx.fill()
    ctx.restore()

    // Grooves: hairlines of uneven brightness. Seven bands, so the gaps between them stay smooth.
    const next = random(7)
    ctx.strokeStyle = '#ffffff'
    ctx.lineWidth = 0.7
    for (let r = radius * BANDS_INNER; r < radius * BANDS_OUTER; r += 1.45) {
      const strength = next()
      if (!this.grooved(r / radius, 7)) continue
      ctx.globalAlpha = 0.02 + strength * 0.07
      ctx.beginPath()
      ctx.arc(0, 0, r, 0, TAU)
      ctx.stroke()
    }

    // The rim catches the light; the run-out next to the label has one scribed ring.
    ctx.globalAlpha = 0.22
    ctx.lineWidth = 1.2
    ctx.beginPath()
    ctx.arc(0, 0, radius - 0.8, 0, TAU)
    ctx.stroke()
    ctx.globalAlpha = 0.12
    ctx.lineWidth = 0.8
    ctx.beginPath()
    ctx.arc(0, 0, radius * (LABEL + BANDS_INNER) * 0.5, 0, TAU)
    ctx.stroke()

    this.base = canvas
    return canvas
  }

  // Two wedges of reflected light, cut into hairlines by the grooves.
  private sheenLayer(): HTMLCanvasElement {
    if (this.sheen) return this.sheen
    const [canvas, ctx] = this.layer()
    const radius = this.geometry.size / CANVAS_RADII
    this.sheen = canvas
    // Older browsers have no conic gradients; the record is then matt.
    if (typeof ctx.createConicGradient !== 'function') return canvas

    // A wedge of light: a narrow bright core with a wide, soft skirt. Stops are fractions of a turn.
    const lobe = (alpha: number): [number, number][] => [
      [0, alpha],
      [0.014, alpha * 0.66],
      [0.05, alpha * 0.24],
      [0.1, alpha * 0.05],
      [0.14, 0],
    ]
    const mirrored = (centre: number, alpha: number): [number, number][] => [
      ...lobe(alpha).map(([at, a]): [number, number] => [centre - at, a]).reverse(),
      ...lobe(alpha).map(([at, a]): [number, number] => [centre + at, a]).slice(1),
    ]
    // The brighter wedge points up and to the right, or to the left when turned; the other is opposite.
    const start = this.geometry.turned ? 2.76 : -0.66
    const light = ctx.createConicGradient(start, 0, 0)
    for (const [at, alpha] of [...lobe(0.62), ...mirrored(0.5, 0.42), ...lobe(0.62).map(([at, a]): [number, number] => [1 - at, a]).reverse()]) {
      light.addColorStop(at, `rgb(255 255 255 / ${alpha})`)
    }
    ctx.fillStyle = light
    ring(ctx, radius - 1, radius * LABEL)
    ctx.fill()

    // Carve the light where grooves are; the smooth gaps and the run-out keep all of it.
    const next = random(11)
    ctx.globalCompositeOperation = 'destination-out'
    ctx.strokeStyle = '#000000'
    ctx.lineWidth = 0.9
    for (let r = radius * BANDS_INNER; r < radius * BANDS_OUTER; r += 1.3) {
      const strength = next()
      if (!this.grooved(r / radius, 7)) continue
      ctx.globalAlpha = 0.25 + strength * 0.75
      ctx.beginPath()
      ctx.arc(0, 0, r, 0, TAU)
      ctx.stroke()
    }

    // Between the wedges the surface faces away from the light: a quarter turn on, it darkens a little.
    ctx.globalCompositeOperation = 'source-over'
    ctx.globalAlpha = 1
    const shade = ctx.createConicGradient(start + Math.PI / 2, 0, 0)
    for (const [at, alpha] of [...lobe(0.3), ...mirrored(0.5, 0.3), ...lobe(0.3).map(([at, a]): [number, number] => [1 - at, a]).reverse()]) {
      shade.addColorStop(at, `rgb(0 0 0 / ${alpha})`)
    }
    ctx.fillStyle = shade
    ring(ctx, radius * BANDS_OUTER, radius * BANDS_INNER)
    ctx.fill()
    return canvas
  }

  // The blank label: paper, a pressed ring, and the day stamped under the spindle like a test pressing.
  private labelLayer(palette: Palette, stamp: string): HTMLCanvasElement {
    if (this.label && this.stamped === stamp) return this.label
    const { size, scale } = this.geometry
    const radius = (size / CANVAS_RADII) * LABEL
    const canvas = document.createElement('canvas')
    const pixels = Math.max(1, Math.ceil(radius * 2 * scale))
    canvas.width = pixels
    canvas.height = pixels
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new Error('No 2D canvas')
    ctx.setTransform(scale, 0, 0, scale, pixels / 2, pixels / 2)

    ctx.fillStyle = palette.paper
    ctx.beginPath()
    ctx.arc(0, 0, radius, 0, TAU)
    ctx.fill()

    ctx.strokeStyle = palette.ink
    ctx.globalAlpha = 0.22
    ctx.lineWidth = Math.max(1, radius * 0.012)
    ctx.beginPath()
    ctx.arc(0, 0, radius * 0.9, 0, TAU)
    ctx.stroke()
    ctx.globalAlpha = 1

    if (stamp) {
      ctx.fillStyle = palette.ink
      ctx.textAlign = 'center'
      ctx.textBaseline = 'middle'
      ctx.font = `600 ${radius * 0.15}px ${palette.font}`
      ctx.fillText(stamp, 0, radius * 0.52, radius * 1.3)
    }

    this.label = canvas
    this.stamped = stamp
    return canvas
  }

  private paintBands(view: RecordView, palette: Palette, radius: number): void {
    const { ctx } = this
    const count = view.lit.length
    // The ripple fades to nothing at one point of the circle, so the ends of each line meet. That point is on the cropped side.
    const seam = this.geometry.turned ? -Math.PI / 2 : Math.PI

    for (let i = 0; i < count; i++) {
      const lit = view.lit[i]
      if (lit <= 0) continue
      const [outerEdge, innerEdge] = bandEdges(i, count)
      const outer = outerEdge * radius
      const inner = innerEdge * radius

      ctx.globalAlpha = lit
      ctx.fillStyle = palette.accent
      ring(ctx, outer, inner)
      ctx.fill()

      // The grooves of a lit band, in ink.
      const lines = Math.max(3, Math.min(8, Math.round((outer - inner) / 3.6)))
      const spacing = (outer - inner) / lines
      ctx.strokeStyle = palette.ink
      ctx.lineWidth = Math.max(0.8, spacing * 0.22)
      ctx.globalAlpha = lit * 0.42
      ctx.beginPath()
      for (let j = 0; j < lines; j++) {
        const r = inner + (j + 0.5) * spacing
        if (!view.wave) {
          ctx.moveTo(r, 0)
          ctx.arc(0, 0, r, 0, TAU)
          continue
        }
        const wave = view.wave
        const points = Math.max(64, Math.min(220, Math.round((TAU * r) / 7)))
        // Each line reads a different stretch of the signal, so neighbours do not move as one.
        const offset = (i * 211 + j * 67) % wave.length
        for (let k = 0; k <= points; k++) {
          const turn = k / points
          const sample = wave[(offset + k * 2) % wave.length]
          const push = Math.max(-1, Math.min(1, sample * 2.4)) * Math.sin(Math.PI * turn) * spacing * 0.55
          const angle = seam + turn * TAU
          const x = Math.cos(angle) * (r + push)
          const y = Math.sin(angle) * (r + push)
          if (k === 0) ctx.moveTo(x, y)
          else ctx.lineTo(x, y)
        }
      }
      ctx.stroke()
    }
    ctx.globalAlpha = 1
  }

  // The clip length of each band, printed along one radius. These do not turn with the record.
  private paintScale(view: RecordView, palette: Palette, radius: number): void {
    const { ctx } = this
    const count = view.labels.length
    if (count === 0) return
    const width = ((BANDS_OUTER - BANDS_INNER) / count - BAND_GAP) * radius
    const size = Math.max(9, Math.min(17, width * 0.6))
    const cos = Math.cos(this.geometry.scaleAngle)
    const sin = Math.sin(this.geometry.scaleAngle)

    ctx.font = `600 ${size}px ${palette.font}`
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    for (let i = 0; i < count; i++) {
      const [outer, inner] = bandEdges(i, count)
      const r = ((outer + inner) / 2) * radius
      const x = cos * r
      const y = sin * r
      const lit = view.lit[i] ?? 0
      const text = view.labels[i]
      const half = ctx.measureText(text).width / 2 + size * 0.35
      // A plain patch under the figure keeps the groove lines out of it.
      ctx.fillStyle = lit >= 0.5 ? palette.accent : palette.vinyl
      ctx.beginPath()
      ctx.ellipse(x, y, half, Math.min(width * 0.5, size * 0.78), 0, 0, TAU)
      ctx.fill()
      ctx.fillStyle = lit >= 0.5 ? palette.ink : palette.vinylText
      ctx.fillText(text, x, y + size * 0.04)
    }
  }

  private paintLabel(view: RecordView, palette: Palette, radius: number): void {
    const { ctx } = this
    const r = radius * LABEL

    ctx.save()
    ctx.rotate(view.rotation)
    ctx.drawImage(this.labelLayer(palette, view.stamp), -r, -r, r * 2, r * 2)
    ctx.restore()

    const cover = view.cover
    if (cover && view.printed > 0 && cover.naturalWidth > 0) {
      const sweep = Math.min(view.printed, 1) * TAU
      ctx.save()
      ctx.beginPath()
      if (view.printed < 1) {
        // The label turns under a fixed print head: what has passed it is printed.
        ctx.moveTo(0, 0)
        ctx.arc(0, 0, r, PRINT_HEAD, PRINT_HEAD + sweep)
        ctx.closePath()
      } else {
        ctx.arc(0, 0, r, 0, TAU)
      }
      ctx.clip()
      ctx.rotate(view.rotation)
      // Centre-crop to a square, whatever the image's shape.
      const side = Math.min(cover.naturalWidth, cover.naturalHeight)
      ctx.drawImage(
        cover,
        (cover.naturalWidth - side) / 2,
        (cover.naturalHeight - side) / 2,
        side,
        side,
        -r,
        -r,
        r * 2,
        r * 2,
      )
      ctx.restore()
    }

    // The label sits in a shallow dish: a dark edge all round.
    ctx.strokeStyle = palette.vinyl
    ctx.globalAlpha = 0.35
    ctx.lineWidth = Math.max(1, radius * 0.006)
    ctx.beginPath()
    ctx.arc(0, 0, r, 0, TAU)
    ctx.stroke()
    ctx.globalAlpha = 1

    // The spindle hole goes through to the sleeve.
    ctx.fillStyle = palette.field
    ctx.beginPath()
    ctx.arc(0, 0, radius * HOLE, 0, TAU)
    ctx.fill()
    ctx.strokeStyle = palette.ink
    ctx.lineWidth = Math.max(1, radius * 0.005)
    ctx.stroke()
  }

  private paintArc(fraction: number, palette: Palette, radius: number): void {
    const { ctx } = this
    const r = radius * (LABEL + BANDS_INNER) * 0.5
    ctx.lineWidth = radius * 0.03
    ctx.strokeStyle = palette.paper
    ctx.globalAlpha = 0.3
    ctx.beginPath()
    ctx.arc(0, 0, r, 0, TAU)
    ctx.stroke()
    ctx.globalAlpha = 1
    if (fraction <= 0) return
    ctx.lineCap = 'round'
    ctx.beginPath()
    ctx.arc(0, 0, r, -Math.PI / 2, -Math.PI / 2 + Math.min(fraction, 1) * TAU)
    ctx.stroke()
    ctx.lineCap = 'butt'
  }

  private turn(point: Point): Point {
    return this.geometry.turned ? { x: -point.y, y: point.x } : point
  }

  private paintArm(needle: number, palette: Palette, radius: number): void {
    const { ctx } = this
    const { scale } = this.geometry

    // The stylus sits on the groove at this radius; the arm swings about its pivot to reach it.
    const r = BANDS_OUTER - Math.max(0, Math.min(1, needle)) * (BANDS_OUTER - BANDS_INNER)
    const reach = Math.hypot(PIVOT.x, PIVOT.y)
    const swing = Math.acos((reach * reach + ARM_LENGTH * ARM_LENGTH - r * r) / (2 * reach * ARM_LENGTH))
    const heading = Math.atan2(-PIVOT.y, -PIVOT.x) - swing
    const at = (point: Point): Point => {
      const turned = this.turn(point)
      return { x: turned.x * radius, y: turned.y * radius }
    }
    const pivot = at(PIVOT)
    const stylus = at({
      x: PIVOT.x + Math.cos(heading) * ARM_LENGTH,
      y: PIVOT.y + Math.sin(heading) * ARM_LENGTH,
    })

    // Unit vectors: along the arm, and sideways away from the spindle.
    const length = Math.hypot(stylus.x - pivot.x, stylus.y - pivot.y)
    const along = { x: (stylus.x - pivot.x) / length, y: (stylus.y - pivot.y) / length }
    let side = { x: -along.y, y: along.x }
    if (side.x * stylus.x + side.y * stylus.y < 0) side = { x: -side.x, y: -side.y }

    // The tube runs outside the straight line and the headshell angles back in, as on a real arm.
    const elbow = {
      x: pivot.x + along.x * length * 0.8 + side.x * radius * 0.055,
      y: pivot.y + along.y * length * 0.8 + side.y * radius * 0.055,
    }
    const tubeLength = Math.hypot(elbow.x - pivot.x, elbow.y - pivot.y)
    const tube = { x: (elbow.x - pivot.x) / tubeLength, y: (elbow.y - pivot.y) / tubeLength }
    const shellLength = Math.hypot(stylus.x - elbow.x, stylus.y - elbow.y)
    const shell = { x: (stylus.x - elbow.x) / shellLength, y: (stylus.y - elbow.y) / shellLength }
    const lift = { x: -shell.y, y: shell.x }
    const liftSign = lift.x * side.x + lift.y * side.y < 0 ? -1 : 1

    const line = (from: Point, to: Point, width: number, colour: string, cap: CanvasLineCap = 'round') => {
      ctx.strokeStyle = colour
      ctx.lineWidth = width * radius
      ctx.lineCap = cap
      ctx.beginPath()
      ctx.moveTo(from.x, from.y)
      ctx.lineTo(to.x, to.y)
      ctx.stroke()
    }
    const along2 = (origin: Point, direction: Point, distance: number): Point => ({
      x: origin.x + direction.x * distance * radius,
      y: origin.y + direction.y * distance * radius,
    })
    const disc = (centre: Point, size: number, colour: string) => {
      ctx.fillStyle = colour
      ctx.beginPath()
      ctx.arc(centre.x, centre.y, size * radius, 0, TAU)
      ctx.fill()
    }

    const body = (paper: string, ink: string) => {
      // Counterweight behind the pivot.
      line(along2(pivot, tube, -0.075), along2(pivot, tube, -0.2), 0.092, paper, 'butt')
      line(pivot, elbow, 0.026, paper)
      // Headshell, finger lift, cartridge.
      line(along2(elbow, shell, -0.012), along2(stylus, shell, 0.03), 0.064, paper, 'butt')
      const grip = along2(stylus, shell, -0.115)
      line(grip, along2(grip, lift, 0.085 * liftSign), 0.014, paper)
      line(along2(stylus, shell, -0.075), along2(stylus, shell, 0.012), 0.04, ink, 'butt')
      disc(pivot, 0.072, paper)
    }

    // First only the shadow: the shapes are drawn off-canvas and their shadow is thrown back.
    const away = this.geometry.size * 2
    ctx.save()
    ctx.shadowColor = 'rgb(0 0 0 / 0.42)'
    ctx.shadowBlur = radius * 0.035 * scale
    ctx.shadowOffsetX = (away + radius * 0.014) * scale
    ctx.shadowOffsetY = radius * 0.03 * scale
    ctx.translate(-away, 0)
    body('#000000', '#000000')
    ctx.restore()

    body(palette.paper, palette.ink)

    // Details in ink: a band on the counterweight, the bearing, and the needle's point in the accent.
    line(along2(pivot, tube, -0.118), along2(pivot, tube, -0.13), 0.092, palette.ink, 'butt')
    ctx.strokeStyle = palette.ink
    ctx.lineWidth = radius * 0.011
    ctx.beginPath()
    ctx.arc(pivot.x, pivot.y, radius * 0.045, 0, TAU)
    ctx.stroke()
    disc(pivot, 0.016, palette.ink)
    disc(stylus, 0.011, palette.accent)
    ctx.lineCap = 'butt'
  }
}
