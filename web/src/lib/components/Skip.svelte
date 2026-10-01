<script lang="ts">
  // Gives up the turn for a longer clip. On the last turn it gives up the game.
  import { clipLabel } from '../clip'
  import type { Game } from '../game.svelte'

  let { game }: { game: Game } = $props()

  const label = $derived(
    game.nextClipSeconds === null
      ? 'Give up'
      : `Skip to ${clipLabel(game.nextClipSeconds)}`,
  )
</script>

<div class="skip">
  <button
    class="button outline numeric"
    type="button"
    onclick={() => game.skip()}
    disabled={game.submitting}
  >
    {label}<span aria-hidden="true"> →</span>
  </button>
  {#if game.lastTurn}
    <p>Last try. Giving up shows the answer.</p>
  {/if}
</div>

<style>
  .skip {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }

  .button {
    flex: none;
    padding: 0;
    border: 0;
    min-height: var(--target);
    color: var(--muted);
    background: transparent;
    font-size: var(--text-small);
  }

  .button:not(:disabled):hover {
    background: transparent;
    color: var(--paper);
  }
  .button span {
    margin-left: 0.5rem;
  }

  /* Beside the button, wrapping within its height, so the tries below do not move when it appears. */
  p {
    flex: 1;
    min-width: 0;
    font-size: var(--text-small);
    line-height: 1.25;
  }
</style>
