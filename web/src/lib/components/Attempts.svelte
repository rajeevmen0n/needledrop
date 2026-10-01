<script lang="ts">
  // The seven tries, one row each, in the order of the record's bands. Every
  // state has its own mark and words, so none is told apart by colour alone.
  //
  // While the game is open the heading also carries the section's streak.
  import { clipShort } from '../clip'
  import type { Game } from '../game.svelte'
  import { streakLine } from '../stats'

  let { game }: { game: Game } = $props()
</script>

<section class="attempts" aria-labelledby="attempts-heading">
  <h2 id="attempts-heading">
    {game.finished
      ? 'Your listening notes'
      : `Try ${game.turn} of ${game.ladder.length}`}<span
      class="numeric"
      >{game.finished
        ? "Today's pressing"
        : streakLine(game.stats) || 'Trust your ears'}</span
    >
  </h2>
  <ol>
    {#each game.slots as slot, i}
      <li
        class={slot.kind}
        aria-current={slot.kind === 'current' ? 'step' : undefined}
      >
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
  .attempts {
    border-top: 1px solid var(--border);
    padding-top: 1rem;
  }
  h2 {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    font-size: 0.8125rem;
    font-weight: 600;
  }
  h2 span {
    color: var(--muted);
    font-weight: 400;
  }
  ol {
    margin-top: 0.5rem;
  }
  li {
    display: grid;
    grid-template-columns: 3rem 1.25rem minmax(0, 1fr);
    align-items: start;
    gap: 0.5rem;
    padding: 0.45rem 0;
    font-size: 0.8125rem;
    line-height: 1.25rem;
    color: var(--muted);
  }
  li.empty {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
  li.current {
    color: var(--paper);
    font-weight: 600;
  }
  .mark {
    display: grid;
    place-items: center;
    width: 1.25rem;
    height: 1.25rem;
    border-radius: 50%;
  }
  svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .what {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .title {
    color: var(--paper);
  }
  .dot {
    fill: var(--accent);
    stroke: none;
  }
  li.correct .mark {
    background: var(--accent);
    color: var(--ink);
  }
  li.correct {
    color: var(--paper);
  }
</style>
