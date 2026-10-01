<script lang="ts">
  // Play the unlocked clip, see how much is unlocked, and skip the turn.
  import { clipLabel } from '../clip'
  import type { Game } from '../game.svelte'
  import Timeline from './Timeline.svelte'

  let { game }: { game: Game } = $props()

  const skipLabel = $derived(
    game.nextClipSeconds === null ? 'Give up' : `Skip to ${clipLabel(game.nextClipSeconds)}`,
  )
</script>

<section class="controls" aria-label="Clip">
  <button class="primary play" type="button" onclick={() => game.toggle()}>
    {game.playing ? 'Stop' : `Play ${clipLabel(game.clipSeconds)}`}
  </button>

  {#if game.clipError}
    <p class="error" role="alert">{game.clipError}</p>
  {/if}

  <Timeline {game} />

  <div class="skip">
    <button type="button" onclick={() => game.skip()} disabled={game.submitting}>
      {skipLabel}
    </button>
    {#if game.lastTurn}
      <p class="muted">This is the last try. Giving up shows the answer.</p>
    {/if}
  </div>
</section>

<style>
  .controls {
    display: grid;
    gap: var(--space-3);
  }

  .play {
    font-size: var(--text-large);
    min-height: 3.5rem;
  }

  .skip {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2) var(--space-3);
  }

  .skip p {
    font-size: var(--text-small);
  }
</style>
