<script lang="ts">
  // The seven tries, one row each, in the order of the record's bands. Every
  // state has its own mark and words, so none is told apart by colour alone.
  import { clipShort } from '../clip'
  import type { Game } from '../game.svelte'

  let { game }: { game: Game } = $props()
</script>

<section class="attempts" aria-labelledby="attempts-heading">
  <h2 class="sr-only" id="attempts-heading">Tries</h2>
  <ol>
    {#each game.slots as slot, i}
      <li class={slot.kind} aria-current={slot.kind === 'current' ? 'step' : undefined}>
        <span class="clip numeric">{clipShort(game.ladder[i])}</span>
        <span class="mark" aria-hidden="true">
          <svg viewBox="0 0 16 16" width="16" height="16">
            {#if slot.kind === 'skip'}
              <path d="M2.5 3.5 7 8l-4.5 4.5M8.5 3.5 13 8l-4.5 4.5" />
            {:else if slot.kind === 'wrong'}
              <path d="m3.5 3.5 9 9m0-9-9 9" />
            {:else if slot.kind === 'correct'}
              <path d="m3 8.5 3.5 3.5L13 4.5" />
            {:else if slot.kind === 'current'}
              <circle class="dot" cx="8" cy="8" r="5" />
            {:else}
              <circle cx="8" cy="8" r="4.5" />
            {/if}
          </svg>
        </span>
        <span class="what">
          {#if slot.kind === 'skip'}
            Skipped
          {:else if slot.kind === 'wrong'}
            <span class="sr-only">Wrong:</span>
            <span class="title">{slot.title}</span> by {slot.artist}
          {:else if slot.kind === 'correct'}
            <span class="sr-only">Correct:</span>
            <span class="title">{slot.title}</span> by {slot.artist}
          {:else if slot.kind === 'current'}
            This try
          {:else}
            <span class="sr-only">Not used</span>
          {/if}
        </span>
      </li>
    {/each}
  </ol>
</section>

<style>
  li {
    display: grid;
    grid-template-columns: 3.25rem 1.5rem 1fr;
    align-items: start;
    gap: var(--space-2);
    padding: var(--space-2) 0;
    border-top: var(--line) solid transparent;
    line-height: 1.5rem;
  }

  .mark {
    display: grid;
    place-items: center;
    width: 1.5rem;
    height: 1.5rem;
    border-radius: 50%;
  }

  svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 2.2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .what {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .title {
    font-weight: var(--weight-medium);
  }

  /* The try being played: a full dot, like the lit band, and heavier words. */
  li.current {
    font-weight: var(--weight-medium);
  }

  .dot {
    fill: var(--accent);
    stroke: var(--paper);
    stroke-width: 1.5;
  }

  /* The right answer: the mark turns into a filled disc. */
  li.correct .mark {
    background: var(--accent);
    color: var(--ink);
  }

  li.correct {
    font-weight: var(--weight-medium);
  }
</style>
