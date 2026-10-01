<script lang="ts">
  // A few figures on one line, parted by hairlines: a section's record, a
  // random session's score. It knows nothing about what they count.
  //
  // Every figure has its sentence for a screen reader; the big number and its
  // label are the same thing drawn, and hidden from it.
  interface Figure {
    label: string
    value: string
    /** The figure as a sentence: "Current streak: 3 days". */
    spoken: string
  }

  let { figures }: { figures: Figure[] } = $props()
</script>

<ul class="figures" role="list" style:--count={figures.length}>
  {#each figures as figure}
    <li>
      <span class="sr-only">{figure.spoken}</span>
      <span class="value numeric" aria-hidden="true">{figure.value}</span>
      <span class="label" aria-hidden="true">{figure.label}</span>
    </li>
  {/each}
</ul>

<style>
  /* A pressing's label, not a dashboard. */
  .figures {
    display: grid;
    grid-template-columns: repeat(var(--count), minmax(0, 1fr));
  }

  li {
    display: grid;
    gap: 0.125rem;
    padding: 0 0.75rem;
    border-left: var(--line) solid var(--border);
  }

  li:first-child {
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

  @media (max-width: 22.5rem) {
    li {
      padding: 0 0.5rem;
    }

    .value {
      font-size: 1.5rem;
    }
  }
</style>
