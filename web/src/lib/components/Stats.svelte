<script lang="ts">
  // A player's record in one section: four figures and the wins by try. It
  // knows nothing about the game; the page shows it with the result.
  //
  // Every figure and every bar has its sentence for a screen reader; the big
  // numbers and the bars are the same thing drawn, and hidden from it.
  import { distribution, figures } from '../stats'
  import type { Stats } from '../stats'

  interface Props {
    stats: Stats
    /** The heading: whose record this is. */
    title: string
    /** The try today's game was won on (1 to 7), to mark its bar; `null` when it was not won. */
    todaysTry?: number | null
  }

  let { stats, title, todaysTry = null }: Props = $props()

  const uid = $props.id()
  const rows = $derived(distribution(stats.guessDistribution, todaysTry))
</script>

<section class="stats" aria-labelledby="{uid}-heading">
  <h2 id="{uid}-heading">{title}</h2>
  <ul class="figures" role="list">
    {#each figures(stats) as figure}
      <li>
        <span class="sr-only">{figure.spoken}</span>
        <span class="value numeric" aria-hidden="true">{figure.value}</span>
        <span class="label" aria-hidden="true">{figure.label}</span>
      </li>
    {/each}
  </ul>
  <h3 id="{uid}-tries">Wins by try</h3>
  <ol class="tries" role="list" aria-labelledby="{uid}-tries">
    {#each rows as row}
      <li class:today={row.today}>
        <span class="sr-only">{row.spoken}</span>
        <span class="try numeric" aria-hidden="true">{row.try}</span>
        <span class="track" aria-hidden="true">
          <span class="bar" style:--share={row.fraction}></span>
          <span class="wins numeric"
            >{row.wins}{#if row.today}<span class="when">Today</span>{/if}</span
          >
        </span>
      </li>
    {/each}
  </ol>
</section>

<style>
  .stats {
    display: grid;
    gap: 0.75rem;
    border-top: var(--line) solid var(--border);
    padding-top: 1rem;
  }

  h2 {
    font-size: var(--text-small);
    font-weight: var(--weight-medium);
  }

  h3 {
    margin: 0.25rem 0 -0.25rem;
    color: var(--muted);
    font-size: 0.75rem;
    font-weight: var(--weight-regular);
  }

  /* Four figures on one line, parted by hairlines: a pressing's label, not a dashboard. */
  .figures {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
  }

  .figures li {
    display: grid;
    gap: 0.125rem;
    padding: 0 0.75rem;
    border-left: var(--line) solid var(--border);
  }

  .figures li:first-child {
    padding-left: 0;
    border-left: 0;
  }

  .value {
    font-size: var(--text-title);
    font-weight: var(--weight-medium);
    line-height: 1.1;
    letter-spacing: -0.02em;
  }

  .label {
    color: var(--muted);
    font-size: 0.75rem;
    line-height: 1.3;
  }

  .tries {
    display: grid;
    gap: 0.125rem;
  }

  .tries li {
    display: grid;
    grid-template-columns: 0.875rem minmax(0, 1fr);
    align-items: center;
    gap: 0.5rem;
    min-height: 1.375rem;
    color: var(--muted);
    font-size: var(--text-small);
    line-height: 1.375rem;
  }

  .track {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
    /* Kept free at the end of the longest bar for its count and "Today". */
    --room: 4.5rem;
  }

  /* Thin, square at the baseline, rounded where the count ends. No wins is a tick on the baseline. */
  .bar {
    flex: none;
    width: calc((100% - var(--room)) * var(--share));
    min-width: 2px;
    height: 0.625rem;
    border-radius: 0 4px 4px 0;
    background: var(--control-border);
  }

  .wins {
    color: var(--paper);
    white-space: nowrap;
  }

  /* Today's win: the one champagne bar, and the word says so too. */
  .today .bar {
    background: var(--accent);
  }

  .today .try {
    color: var(--paper);
    font-weight: var(--weight-medium);
  }

  .when {
    margin-left: 0.5rem;
    color: var(--muted);
    font-size: 0.75rem;
  }

  @media (max-width: 22.5rem) {
    .figures li {
      padding: 0 0.5rem;
    }

    .value {
      font-size: 1.5rem;
    }
  }
</style>
