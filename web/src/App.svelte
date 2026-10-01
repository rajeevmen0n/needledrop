<script lang="ts">
  import { onMount } from 'svelte'
  import Attempts from './lib/components/Attempts.svelte'
  import Controls from './lib/components/Controls.svelte'
  import GuessInput from './lib/components/GuessInput.svelte'
  import Reveal from './lib/components/Reveal.svelte'
  import { game } from './lib/game.svelte'

  onMount(() => {
    void game.load()
    // Coming back to the tab: the day may have changed, or another tab may have played.
    const onvisible = () => {
      if (document.visibilityState === 'visible') void game.refresh()
    }
    document.addEventListener('visibilitychange', onvisible)
    return () => document.removeEventListener('visibilitychange', onvisible)
  })
</script>

<div class="page">
  <header>
    <h1>Needledrop</h1>
    {#if game.daily}
      <p class="number numeric">No. {game.daily.number}</p>
    {/if}
  </header>

  <main>
    {#if game.daily}
      {#if game.finished}
        <Reveal {game} />
      {:else}
        <p class="intro">
          Listen to the clip and name the song. Each skip or wrong guess unlocks a longer clip.
        </p>
        <Controls {game} />
        <GuessInput onguess={(track) => game.guess(track)} busy={game.submitting} />
        {#if game.moveError}
          <p class="error" role="alert">{game.moveError}</p>
        {/if}
      {/if}
      <!-- Always in the page, so screen readers announce it when the text changes. -->
      <p class="notice" role="status">{game.finished ? '' : game.notice}</p>
      <Attempts {game} />
    {:else if game.loadError}
      <div class="failed" role="alert">
        <p>{game.loadError}</p>
        <button class="primary" type="button" onclick={() => game.load()}>Try again</button>
      </div>
    {:else}
      <p class="muted" role="status">Loading today's song…</p>
    {/if}
  </main>
</div>

<style>
  .page {
    display: grid;
    gap: var(--space-6);
    max-width: var(--page-width);
    margin: 0 auto;
    padding: var(--space-4) var(--space-4) var(--space-8);
  }

  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-4);
    padding-bottom: var(--space-3);
    border-bottom: var(--border);
  }

  h1 {
    font-size: var(--text-title);
    line-height: 1.2;
  }

  .number {
    font-size: var(--text-large);
    font-weight: 600;
  }

  main {
    display: grid;
    gap: var(--space-6);
  }

  .intro {
    color: var(--color-muted);
  }

  .notice:empty {
    display: none;
  }

  .failed {
    display: grid;
    justify-items: start;
    gap: var(--space-3);
  }
</style>
