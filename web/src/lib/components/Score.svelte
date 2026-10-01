<script lang="ts">
  // A random session's score: the run, the best run ever, and how the session
  // has gone. It knows nothing about the game; the page shows it with the
  // result. There are no wins by try here: only the longest run is kept.
  import { scoreFigures } from '../random'
  import type { RandomScore } from '../random'
  import Figures from './Figures.svelte'

  let { score }: { score: RandomScore } = $props()

  const uid = $props.id()
</script>

<section class="score" aria-labelledby="{uid}-heading">
  <h2 id="{uid}-heading">Your random run</h2>
  <Figures figures={scoreFigures(score)} />
</section>

<style>
  .score {
    display: grid;
    gap: 0.75rem;
    border-top: var(--line) solid var(--border);
    padding-top: 1rem;
  }

  h2 {
    font-size: var(--text-small);
    font-weight: var(--weight-medium);
  }
</style>
