<script lang="ts">
  // The play button and a one-line readout of the clip.
  import { clipLabel, clipShort } from '../clip'
  import type { Game } from '../game.svelte'

  let { game }: { game: Game } = $props()

  /** Clips shorter than this are over before "Stop" could be read, so the button keeps saying "Play". */
  const STOPPABLE_SECONDS = 3

  let elapsed = $state(0)

  const stoppable = $derived(game.playing && game.clipSeconds >= STOPPABLE_SECONDS)
  const label = $derived.by(() => {
    if (stoppable) return 'Stop'
    return game.finished ? `Play all ${clipLabel(game.clipSeconds)}` : `Play ${clipLabel(game.clipSeconds)}`
  })
  const readout = $derived.by(() => {
    if (game.clipLoading) return 'Loading the clip…'
    if (stoppable) return `${elapsed.toFixed(1)} s`
    return game.finished ? '' : `${clipShort(game.clipSeconds)} of ${game.totalSeconds}`
  })

  function press() {
    // A short clip that is still sounding starts again; a long one stops.
    if (stoppable) game.stop()
    else void game.play()
  }

  // The player has no timer of its own: ask it once a frame while a long clip plays.
  $effect(() => {
    if (!stoppable) {
      elapsed = 0
      return
    }
    let frame = requestAnimationFrame(function tick() {
      // Tenths: the text only changes ten times a second.
      elapsed = Math.floor(game.progress().elapsed * 10) / 10
      frame = requestAnimationFrame(tick)
    })
    return () => cancelAnimationFrame(frame)
  })
</script>

<div class="play">
  <button type="button" class:playing={stoppable} onclick={press}>
    <span class="disc" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="24" height="24">
        {#if stoppable}
          <rect x="6" y="6" width="12" height="12" />
        {:else}
          <path d="M8 4.5v15l12-7.5z" />
        {/if}
      </svg>
    </span>
    <span class="text numeric">
      <span class="label">{label}</span>
      <!-- Decorative: the sentence after the button says the same thing to screen readers. -->
      <span class="readout" aria-hidden="true">{readout}</span>
    </span>
  </button>
  <p class="sr-only">{clipLabel(game.clipSeconds)} of {game.totalSeconds} unlocked.</p>
  {#if game.clipError}
    <p class="error" role="alert">{game.clipError}</p>
  {/if}
</div>

<style>
  .play {
    display: grid;
    justify-items: start;
    gap: var(--space-2);
  }

  button {
    display: inline-flex;
    align-items: center;
    gap: var(--space-3);
    min-height: var(--play-disc);
    /* Room on the right so the focus ring does not hug the text. */
    padding-right: var(--space-3);
    border-radius: calc(var(--play-disc) / 2);
    text-align: left;
  }

  /* A small record of its own: the one round control on the page. */
  .disc {
    flex: none;
    display: grid;
    place-items: center;
    width: var(--play-disc);
    height: var(--play-disc);
    border: var(--line) solid var(--paper);
    border-radius: 50%;
    background: var(--accent);
    color: var(--ink);
    transition: transform 120ms var(--ease-out);
  }

  svg {
    fill: currentColor;
  }

  button:active .disc {
    transform: scale(0.93);
  }

  @media (hover: hover) {
    button:hover .disc {
      background: var(--paper);
    }
  }

  .text {
    display: grid;
  }

  .label {
    font-size: var(--text-large);
    font-weight: var(--weight-medium);
    line-height: 1.2;
  }

  .readout {
    font-size: var(--text-small);
    font-weight: var(--weight-regular);
  }

  .readout:empty {
    display: none;
  }
</style>
