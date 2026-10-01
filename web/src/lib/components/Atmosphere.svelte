<script lang="ts">
  // The night sky the record hangs in. This component owns the canvas, its
  // size and the one animation loop; `sky.ts` holds the stars and their
  // motion, `starfield.ts` draws them. The haze is plain CSS on this element.
  //
  // The page tells the sky about itself through two attributes rather than
  // props: `data-sky-calm` marks anything with text in it (stars dim behind it
  // and shooting stars keep away), and `data-sky-origin` marks the record's
  // centre (left/top) and radius (width), where the stardust of a win starts.
  //
  // Decorative: hidden from assistive technology, never takes a pointer event.
  import { onMount } from 'svelte'
  import { Sky, SKY_SEED, rms } from '../sky'
  import type { Rect } from '../sky'
  import { StarfieldPainter } from '../starfield'

  type Phase = 'loading' | 'playing' | 'won' | 'lost'

  let {
    playing = false,
    paused = false,
    status = 'loading',
    analyser = () => null,
  }: {
    /** A clip is sounding: the sky picks up pace and brightens. */
    playing?: boolean
    /** The footer's "Pause motion": hold the frame. */
    paused?: boolean
    /** The game's status, or `loading` before the first state arrives. */
    status?: Phase
    /** The audio analyser, once there is one; the stars breathe with its level. */
    analyser?: () => AnalyserNode | null
  } = $props()

  /** Cap pixel density and backing-store size: a sky of points gains nothing beyond this. */
  const MAX_SCALE = 2
  const MAX_PIXELS = 4_200_000
  /** Milliseconds between frames while nothing but the slow drift is happening. */
  const IDLE_FRAME = 1000 / 30
  /** Longest step in seconds, so a frame after a stall does not jump. */
  const MAX_STEP = 0.1
  /** The page is measured again this often while the loop runs, in case it moved without an event. */
  const REMEASURE = 600
  /** Scrolling gets every frame for this long after the last scroll event. */
  const SCROLL_SETTLE = 200

  let canvas = $state<HTMLCanvasElement>()
  let painter: StarfieldPainter | undefined
  let sizes: ResizeObserver | undefined
  const watched = new WeakSet<Element>()
  // The stars are the same for everyone; when the shooting stars fall is not.
  const sky = new Sky(SKY_SEED, Date.now())

  // What the page looks like now. Plain variables: only the animation frame reads them.
  let calm: Rect[] = []
  let origin: { x: number; y: number; radius: number } | null = null
  let scroll = 0
  let scale = 0

  let still = false
  let stale = true
  let frame = 0
  let last = 0
  let measuredAt = 0
  let scrolledAt = -Infinity
  let previous: Phase = 'loading'
  /** A win is waiting for the next frame, which knows where the record is. */
  let pending = false
  const samples = new Float32Array(256)

  /** Reads the layout once: the canvas size, the calm rectangles and the record's place. */
  function measure(node: HTMLCanvasElement, now: number): void {
    if (!painter) return
    const width = node.clientWidth
    const height = node.clientHeight
    const density = Math.min(
      window.devicePixelRatio || 1,
      MAX_SCALE,
      Math.sqrt(MAX_PIXELS / Math.max(width * height, 1)),
    )
    if (width !== sky.width || height !== sky.height || density !== scale) {
      scale = density
      painter.configure({ width, height, scale })
      sky.resize(width, height)
    }

    const box = node.getBoundingClientRect()
    calm = []
    document.querySelectorAll('[data-sky-calm]').forEach((element) => {
      if (!watched.has(element)) {
        watched.add(element)
        sizes?.observe(element)
      }
      const rect = element.getBoundingClientRect()
      if (rect.width === 0 || rect.height === 0) return
      calm.push({
        left: rect.left - box.left,
        top: rect.top - box.top,
        right: rect.right - box.left,
        bottom: rect.bottom - box.top,
      })
    })
    const centre = document
      .querySelector('[data-sky-origin]')
      ?.getBoundingClientRect()
    origin =
      centre && centre.width > 0
        ? {
            x: centre.left - box.left,
            y: centre.top - box.top,
            radius: centre.width,
          }
        : null
    // A held sky does not follow the scroll either: with reduced motion it
    // never has, and "Pause motion" keeps it where it was.
    if (still) scroll = 0
    else if (!paused) scroll = window.scrollY
    stale = false
    measuredAt = now
  }

  /** Loudness of the clip right now; 0 when nothing sounds. */
  function level(): number {
    if (!playing) return 0
    const node = analyser()
    if (!node) return 0
    node.getFloatTimeDomainData(samples)
    return rms(samples)
  }

  function tick(now: number): void {
    frame = 0
    if (!painter || !canvas) return
    const frozen = still || paused
    const scrolling = now - scrolledAt < SCROLL_SETTLE
    if (
      !frozen &&
      !stale &&
      !pending &&
      !scrolling &&
      !sky.busy &&
      last !== 0 &&
      now - last < IDLE_FRAME - 2
    ) {
      // The drift is a fraction of a pixel a frame: every other frame is plenty.
      frame = requestAnimationFrame(tick)
      return
    }
    const dt = last === 0 ? 0 : Math.min((now - last) / 1000, MAX_STEP)
    last = now
    if (stale || now - measuredAt > REMEASURE) measure(canvas, now)

    if (pending) {
      pending = false
      if (!frozen && origin) sky.celebrate(origin.x, origin.y, origin.radius)
    }
    if (frozen) {
      // Held: the stars stay where they are, and nothing is left hanging mid-flight.
      sky.settle()
    } else {
      sky.step(dt, { playing, level: level(), avoid: calm })
    }
    painter.paint(sky, { calm, scroll })

    if (frozen) last = 0
    else frame = requestAnimationFrame(tick)
  }

  /** Asks for a frame. Frames then keep coming unless the sky is held. */
  function wake(): void {
    if (frame === 0 && painter && !document.hidden)
      frame = requestAnimationFrame(tick)
  }

  function remeasure(): void {
    stale = true
    wake()
  }

  onMount(() => {
    if (!canvas) return
    try {
      painter = new StarfieldPainter(canvas)
    } catch {
      // No canvas: the page works on plain black.
      return
    }
    const style = getComputedStyle(canvas)
    const value = (name: string) => style.getPropertyValue(name).trim()
    painter.setPalette({
      ivory: value('--paper'),
      silver: value('--muted'),
      cool: value('--star-cool'),
      champagne: value('--accent'),
    })

    const motion = window.matchMedia('(prefers-reduced-motion: reduce)')
    still = motion.matches
    const onmotion = () => {
      still = motion.matches
      remeasure()
    }
    motion.addEventListener('change', onmotion)

    sizes = new ResizeObserver(remeasure)
    sizes.observe(canvas)

    const onscroll = () => {
      scrolledAt = performance.now()
      remeasure()
    }
    window.addEventListener('scroll', onscroll, { passive: true })
    window.addEventListener('resize', remeasure)

    // A hidden tab draws nothing; coming back picks up where the sky was.
    const onvisible = () => {
      if (document.hidden) {
        cancelAnimationFrame(frame)
        frame = 0
        last = 0
      } else {
        remeasure()
      }
    }
    document.addEventListener('visibilitychange', onvisible)

    wake()
    return () => {
      motion.removeEventListener('change', onmotion)
      sizes?.disconnect()
      sizes = undefined
      window.removeEventListener('scroll', onscroll)
      window.removeEventListener('resize', remeasure)
      document.removeEventListener('visibilitychange', onvisible)
      cancelAnimationFrame(frame)
      frame = 0
      painter = undefined
    }
  })

  $effect(() => {
    void playing
    void paused
    wake()
  })

  // A win made on this page, not one found on reload, sets off the stardust.
  $effect(() => {
    const now = status
    if (previous === 'playing' && now === 'won') pending = true
    previous = now
    // Every change of status rearranges the game column.
    remeasure()
  })
</script>

<div class="atmosphere" class:paused aria-hidden="true">
  <canvas bind:this={canvas}></canvas>
</div>

<style>
  .atmosphere {
    /* Where each haze sits, as a position on a sheet 12% larger than the viewport on every side. */
    --warm-at: 50% 30%;
    --cool-at: 88% 87%;
    --wisp-at: 9% 14%;
    position: fixed;
    top: 0;
    left: 0;
    width: 100%;
    height: 100vh;
    /* The tallest the viewport gets, so a phone's collapsing toolbar never resizes the sky. */
    height: 100lvh;
    z-index: 0;
    overflow: hidden;
    pointer-events: none;
    user-select: none;
    contain: strict;
    animation: dusk 1800ms var(--ease-out) backwards;
  }

  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }

  /* Two sheets of haze, moved by the compositor alone: no per-frame painting. */
  .atmosphere::before,
  .atmosphere::after {
    content: '';
    position: absolute;
    inset: -12%;
    will-change: transform;
  }

  /* Warm, behind the record. */
  .atmosphere::before {
    background: radial-gradient(
      ellipse 46% 40% at var(--warm-at),
      rgb(var(--sky-warm) / 0.085),
      rgb(var(--sky-warm) / 0.03) 48%,
      transparent 76%
    );
    animation: haze-warm 84s ease-in-out infinite alternate;
  }

  /* Cool, in the opposite corner, with a fainter wisp across from it. */
  .atmosphere::after {
    background:
      radial-gradient(
        ellipse 50% 46% at var(--cool-at),
        rgb(var(--sky-cool) / 0.17),
        rgb(var(--sky-cool) / 0.06) 50%,
        transparent 78%
      ),
      radial-gradient(
        ellipse 34% 30% at var(--wisp-at),
        rgb(var(--sky-cool) / 0.11),
        transparent 74%
      );
    animation: haze-cool 68s ease-in-out infinite alternate;
  }

  .paused::before,
  .paused::after {
    animation-play-state: paused;
  }

  @keyframes dusk {
    from {
      opacity: 0;
    }
  }

  @keyframes haze-warm {
    from {
      transform: translate3d(-3%, 2%, 0);
    }
    to {
      transform: translate3d(3%, -2%, 0);
    }
  }

  @keyframes haze-cool {
    from {
      transform: translate3d(2.5%, 2%, 0);
    }
    to {
      transform: translate3d(-3%, -1.5%, 0);
    }
  }

  /* Record on the left, game on the right: the same switch as App.svelte. */
  @media (min-width: 40rem) and (min-aspect-ratio: 6/5) {
    .atmosphere {
      --warm-at: 33% 54%;
      --cool-at: 88% 13%;
      --wisp-at: 14% 90%;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .atmosphere,
    .atmosphere::before,
    .atmosphere::after {
      animation: none;
    }
  }
</style>
