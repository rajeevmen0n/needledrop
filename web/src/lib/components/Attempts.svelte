<script lang="ts">
  // The seven tries, one row each. Every state has its own mark and words, so
  // none of them is told apart by colour alone.
  import { clipShort } from '../clip'
  import type { Game, Slot } from '../game.svelte'

  let { game }: { game: Game } = $props()

  const MARKS: Record<Slot['kind'], string> = {
    skip: '»',
    wrong: '✕',
    correct: '✓',
    current: '▶',
    empty: '',
  }
</script>

<section class="attempts" aria-labelledby="attempts-heading">
  <h2 id="attempts-heading">Tries</h2>
  <ol>
    {#each game.slots as slot, i}
      <li class={slot.kind} aria-current={slot.kind === 'current' ? 'step' : undefined}>
        <span class="number numeric">{i + 1}</span>
        <span class="clip numeric">{clipShort(game.ladder[i])}</span>
        <span class="mark" aria-hidden="true">{MARKS[slot.kind]}</span>
        <span class="what">
          {#if slot.kind === 'skip'}
            Skipped
          {:else if slot.kind === 'wrong'}
            <span class="sr-only">Wrong:</span>
            {slot.title} — {slot.artist}
          {:else if slot.kind === 'correct'}
            <span class="sr-only">Correct:</span>
            {slot.title} — {slot.artist}
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
  .attempts {
    display: grid;
    gap: var(--space-2);
  }

  h2 {
    font-size: var(--text-body);
  }

  ol {
    display: grid;
    gap: var(--space-1);
  }

  li {
    display: grid;
    grid-template-columns: 1.25rem 3rem 1.25rem 1fr;
    align-items: center;
    gap: var(--space-2);
    min-height: 2.5rem;
    padding: var(--space-1) var(--space-3);
    border: var(--border);
    border-radius: var(--radius);
    background: var(--color-surface);
  }

  .number,
  .clip {
    color: var(--color-muted);
    font-size: var(--text-small);
  }

  .mark {
    text-align: center;
    font-weight: 700;
  }

  .what {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  li.empty {
    border-style: dashed;
    background: none;
  }

  li.current {
    border-width: 2px;
    border-color: var(--color-text);
    font-weight: 600;
  }

  li.skip .what {
    color: var(--color-muted);
  }

  li.wrong .mark {
    color: var(--color-bad);
  }

  li.correct {
    border-width: 2px;
    border-color: var(--color-good);
  }

  li.correct .mark {
    color: var(--color-good);
  }
</style>
