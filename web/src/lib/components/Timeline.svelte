<script lang="ts">
  // The preview as a bar: one equal-width step per ladder length, the unlocked
  // steps filled, and a playhead that follows the audio clock.
  import { clipLabel, clipShort, ladderFraction } from '../clip'
  import type { Game } from '../game.svelte'

  let { game }: { game: Game } = $props()

  let elapsed = $state(0)

  const unlocked = $derived(ladderFraction(game.clipSeconds, game.ladder))
  const played = $derived(ladderFraction(elapsed, game.ladder))

  // The player has no timer of its own: ask it once a frame while it plays.
  $effect(() => {
    if (!game.playing) {
      elapsed = 0
      return
    }
    let frame = requestAnimationFrame(function tick() {
      elapsed = game.progress().elapsed
      frame = requestAnimationFrame(tick)
    })
    return () => cancelAnimationFrame(frame)
  })
</script>

<div class="timeline">
  <!-- Decorative: the sentence below says the same thing to screen readers. -->
  <div class="track" aria-hidden="true">
    <div class="unlocked" style:width="{unlocked * 100}%"></div>
    <div class="played" style:width="{played * 100}%"></div>
    <ol class="steps numeric">
      {#each game.ladder as step (step)}
        <li class:open={step <= game.clipSeconds}>{clipShort(step)}</li>
      {/each}
    </ol>
  </div>
  <!-- One line either way, so the page does not jump while a clip loads. -->
  {#if game.clipLoading}
    <p class="readout">Loading the clip…</p>
  {:else}
    <p class="readout numeric" aria-hidden="true">
      {elapsed.toFixed(1)} s / {clipShort(game.clipSeconds)}
    </p>
  {/if}
  <p class="sr-only">
    {clipLabel(game.clipSeconds)} of {game.totalSeconds} unlocked.
  </p>
</div>

<style>
  .timeline {
    display: grid;
    gap: var(--space-1);
  }

  .track {
    position: relative;
    height: 2rem;
    border: var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    /* Hatching marks the locked part by pattern, not only by colour. */
    background: repeating-linear-gradient(
      -45deg,
      var(--color-surface) 0 6px,
      var(--color-bg) 6px 9px
    );
  }

  .unlocked,
  .played {
    position: absolute;
    inset: 0 auto 0 0;
  }

  .unlocked {
    background: var(--color-accent-soft);
  }

  .played {
    background: var(--color-accent);
    opacity: 0.45;
  }

  .steps {
    position: absolute;
    inset: 0;
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: 1fr;
  }

  .steps li {
    display: grid;
    place-items: center;
    border-right: var(--border);
    color: var(--color-muted);
    font-size: 0.75rem;
    white-space: nowrap;
  }

  .steps li:last-child {
    border-right: 0;
  }

  .steps li.open {
    color: var(--color-text);
    font-weight: 600;
  }

  .readout {
    color: var(--color-muted);
    font-size: var(--text-small);
  }
</style>
