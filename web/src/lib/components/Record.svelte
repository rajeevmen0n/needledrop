<script lang="ts">
  // The record: fine champagne groove details follow the unlocked clip.
  // While a clip plays the record turns, the
  // needle crosses the lit bands on the audio clock and the grooves ripple
  // with the signal; when the game ends the cover prints onto the label.
  //
  // Decorative: the tries list and the play button say the same in text.
  import { onMount } from 'svelte'
  import { ladderFraction } from '../clip'
  import type { Game } from '../game.svelte'
  import { RecordPainter } from '../record'
  import type { Palette, RecordView } from '../record'

  let { game }: { game: Game } = $props()

  const TURN = Math.PI * 2
  /** 33⅓ turns a minute, in radians a second. */
  const SPIN = (100 / 3 / 60) * TURN
  /** Seconds for a band to light up, and between one band and the next when several do. */
  const LIGHT = 0.32
  const STAGGER = 0.09
  /** The reveal keeps the record turning for at least this long, and gives the cover this long to arrive. */
  const REVEAL_MIN = 1.2
  const REVEAL_MAX = 6
  /** Cap pixel density and backing-store size to keep animation economical. */
  const MAX_SCALE = 2
  const MAX_PIXELS = 1800

  let canvas = $state<HTMLCanvasElement>()
  let painter: RecordPainter | undefined

  // What is on screen now. Plain variables: only the animation frame reads them.
  let rotation = 0
  let speed = 0
  /** A spin-down that ends with the label upright: where and how fast it began, and how long it takes. */
  let coast: {
    from: number
    speed: number
    time: number
    length: number
  } | null = null
  let needle = 0
  let lit: number[] = []
  let targets: number[] = []
  let delays: number[] = []
  let labels: string[] = []
  let printed = 0
  let stamp = ''

  let cover: HTMLImageElement | null = null
  let coverReady = false
  /** Seconds the reveal has run; `null` when none is in flight. */
  let reveal: number | null = null
  /** False until the first state of the game on screen has been shown; that one appears without animation. */
  let shown = false
  let wasFinished = false
  /** The game the record is showing: a section on a day. Another one is put on without ceremony. */
  let scene = ''

  let still = false
  let stale = true
  let frame = 0
  let last = 0
  const samples = new Float32Array(2048)

  const figure = (seconds: number) => String(Math.round(seconds * 10) / 10)
  const dayFormat = new Intl.DateTimeFormat('en-GB', {
    day: 'numeric',
    month: 'short',
    year: 'numeric',
    timeZone: 'UTC',
  })
  /** "1 Oct 2026" for the game's day; empty for anything that is not a date. */
  function dayStamp(day: string | undefined): string {
    const time = day ? Date.parse(`${day}T00:00:00Z`) : NaN
    return Number.isNaN(time) ? '' : dayFormat.format(time)
  }

  function readStyle(node: HTMLCanvasElement): void {
    if (!painter) return
    const style = getComputedStyle(node)
    const value = (name: string) => style.getPropertyValue(name).trim()
    const palette: Palette = {
      field: value('--field'),
      accent: value('--accent'),
      paper: value('--paper'),
      ink: value('--ink'),
      vinyl: value('--vinyl'),
      vinylText: value('--vinyl-text'),
      font: value('--font'),
    }
    painter.setPalette(palette)

    const size = node.clientWidth
    const scale = Math.min(
      window.devicePixelRatio || 1,
      MAX_SCALE,
      MAX_PIXELS / Math.max(size, 1),
    )
    painter.configure({
      size,
      scale,
      turned: value('--record-arm') === '1',
      scaleAngle: (Number(value('--record-scale-angle')) * Math.PI) / 180,
    })
    stale = false
  }

  /** Moves everything one step. Returns whether anything is still moving. */
  function step(dt: number): boolean {
    const playing = game.playing
    let moving = playing

    if (still) {
      // Reduced motion: nothing turns or slides. Bands and cover are simply there.
      coast = null
      speed = 0
      rotation = 0
      needle = 0
      lit = [...targets]
      printed = coverReady ? 1 : 0
      reveal = null
      return moving
    }

    if (reveal !== null) {
      reveal += dt
      const waiting = cover !== null && !coverReady && reveal < REVEAL_MAX
      if (reveal >= REVEAL_MIN && !waiting && (printed >= 1 || !coverReady))
        reveal = null
    }
    const turning = playing || reveal !== null
    if (turning) {
      coast = null
      speed += (SPIN - speed) * (1 - Math.exp(-dt / 0.12))
      rotation = (rotation + speed * dt) % TURN
      moving = true
    } else if (speed > 0 && printed >= 1) {
      // With the cover on the label, the record slows evenly and stops the right way up.
      if (!coast) {
        let distance = TURN - rotation
        while (distance < speed * 0.35) distance += TURN
        coast = {
          from: rotation,
          speed,
          time: 0,
          length: (2 * distance) / speed,
        }
      }
      coast.time += dt
      if (coast.time >= coast.length) {
        rotation = 0
        speed = 0
        coast = null
      } else {
        const t = coast.time
        rotation =
          (coast.from + coast.speed * t * (1 - t / (2 * coast.length))) % TURN
        speed = coast.speed * (1 - t / coast.length)
        moving = true
      }
    } else if (speed > 0) {
      // A blank label can stop anywhere, as a real one does.
      speed *= Math.exp(-dt / 0.45)
      if (speed < 0.03) speed = 0
      rotation = (rotation + speed * dt) % TURN
      moving = true
    }

    if (playing) {
      needle = ladderFraction(game.progress().elapsed, game.ladder)
    } else {
      // Back to the lead-in groove, ready for the next play.
      needle *= Math.exp(-dt / 0.14)
      if (needle < 0.002) needle = 0
      else moving = true
    }

    for (let i = 0; i < targets.length; i++) {
      if (lit[i] === targets[i]) continue
      moving = true
      if (delays[i] > 0) {
        delays[i] -= dt
        continue
      }
      const change = dt / LIGHT
      lit[i] =
        targets[i] > lit[i]
          ? Math.min(targets[i], lit[i] + change)
          : Math.max(targets[i], lit[i] - change)
    }

    if (coverReady && printed < 1) {
      // One turn of the record prints the whole label. Outside a reveal the cover is just there.
      printed = reveal === null ? 1 : Math.min(1, printed + (speed * dt) / TURN)
      moving = true
    }
    return moving
  }

  function view(): RecordView {
    const analyser = game.player.analyser
    let wave: Float32Array | null = null
    if (!still && game.playing && analyser) {
      analyser.getFloatTimeDomainData(samples)
      wave = samples
    }
    return {
      lit,
      labels,
      rotation,
      needle,
      wave,
      stamp,
      cover: coverReady ? cover : null,
      printed,
      arc: still && game.playing ? game.progress().fraction : null,
    }
  }

  function tick(now: number): void {
    frame = 0
    if (!painter || !canvas) return
    if (stale) readStyle(canvas)
    // Capped, so a frame after a long pause does not jump.
    const dt = last === 0 ? 0 : Math.min((now - last) / 1000, 0.05)
    last = now
    const moving = step(dt)
    painter.paint(view())
    if (moving) frame = requestAnimationFrame(tick)
    else last = 0
  }

  /** Asks for a frame. Frames then keep coming only while something moves. */
  function wake(): void {
    if (frame === 0 && painter && !document.hidden)
      frame = requestAnimationFrame(tick)
  }

  function restyle(): void {
    stale = true
    wake()
  }

  onMount(() => {
    if (!canvas) return
    try {
      painter = new RecordPainter(canvas)
    } catch {
      // No canvas: the page works without the picture.
      return
    }

    const motion = window.matchMedia('(prefers-reduced-motion: reduce)')
    still = motion.matches
    const onmotion = () => {
      still = motion.matches
      wake()
    }
    motion.addEventListener('change', onmotion)

    const sizes = new ResizeObserver(restyle)
    sizes.observe(canvas)

    // A hidden tab draws nothing; coming back draws the current state once.
    const onvisible = () => {
      if (document.hidden) {
        cancelAnimationFrame(frame)
        frame = 0
        last = 0
      } else {
        wake()
      }
    }
    document.addEventListener('visibilitychange', onvisible)

    // Text drawn before the font arrived used a fallback: draw it again.
    void document.fonts
      .load(
        `800 condensed 16px ${getComputedStyle(canvas).getPropertyValue('--font')}`,
      )
      .then(() => {
        painter?.invalidate()
        wake()
      })

    wake()
    return () => {
      motion.removeEventListener('change', onmotion)
      sizes.disconnect()
      document.removeEventListener('visibilitychange', onvisible)
      cancelAnimationFrame(frame)
      frame = 0
      painter = undefined
    }
  })

  // Which bands are lit follows the unlocked clip length.
  $effect(() => {
    const ladder = game.ladder
    const unlocked = game.clipSeconds
    const finished = game.finished
    const day = game.day ?? undefined
    const now = `${game.section} ${day}`
    if (now !== scene) {
      // Another tab or another day: a different record, not this one changing.
      scene = now
      shown = false
      reveal = null
    }
    stamp = dayStamp(day)
    if (ladder.length === 0) {
      // Nothing is known of this game yet (or it has no song): a blank side.
      lit = lit.map(() => 0)
      targets = [...lit]
      delays = lit.map(() => 0)
      wasFinished = false
      wake()
      return
    }

    labels = ladder.map(figure)
    targets = ladder.map((seconds) => (seconds <= unlocked + 1e-6 ? 1 : 0))
    if (!shown || lit.length !== targets.length) {
      lit = [...targets]
      delays = targets.map(() => 0)
    } else {
      // Bands light one after another, outermost first.
      let order = 0
      delays = targets.map((target, i) =>
        target !== lit[i] ? order++ * STAGGER : 0,
      )
    }
    if (finished && !wasFinished && shown) reveal = 0
    wasFinished = finished
    shown = true
    wake()
  })

  $effect(() => {
    void game.playing
    wake()
  })

  // The cover arrives with the answer.
  $effect(() => {
    const url = game.answer?.cover
    cover = null
    coverReady = false
    printed = 0
    if (!url) {
      wake()
      return
    }
    const image = new Image()
    image.decoding = 'async'
    image.onload = () => {
      if (cover !== image) return
      coverReady = true
      wake()
    }
    cover = image
    image.src = url
    wake()
    return () => {
      image.onload = null
    }
  })
</script>

<canvas bind:this={canvas} aria-hidden="true"></canvas>

<style>
  canvas {
    position: absolute;
    left: calc(var(--record-x) - var(--record-r) * 1.12);
    top: calc(var(--record-y) - var(--record-r) * 1.12);
    width: calc(var(--record-r) * 2.24);
    height: calc(var(--record-r) * 2.24);
    pointer-events: none;
  }
</style>
