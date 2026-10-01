<script lang="ts">
  import { onMount } from 'svelte'
  import Attempts from './lib/components/Attempts.svelte'
  import GuessInput from './lib/components/GuessInput.svelte'
  import Play from './lib/components/Play.svelte'
  import Record from './lib/components/Record.svelte'
  import Reveal from './lib/components/Reveal.svelte'
  import Skip from './lib/components/Skip.svelte'
  import { game } from './lib/game.svelte'
  import { sleeveForDay, sleeveOverride } from './lib/sleeve'

  // `?sleeve=0..6` previews a sleeve; otherwise the game's UTC day decides, and
  // until the game has loaded, this clock's UTC weekday is the best guess.
  const override = sleeveOverride(window.location.search)
  const sleeve = $derived(
    override ?? (game.daily ? sleeveForDay(game.daily.day) : null) ?? new Date().getUTCDay(),
  )

  $effect(() => {
    const root = document.documentElement
    root.dataset.sleeve = String(sleeve)
    // The browser's own chrome takes the sleeve's colour.
    const field = getComputedStyle(root).getPropertyValue('--field').trim()
    document.querySelector('meta[name="theme-color"]')?.setAttribute('content', field)
  })

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
    <!-- Always there, so the header keeps its height while the game loads. -->
    <p class="number numeric">{game.daily ? `No. ${game.daily.number}` : ''}</p>
  </header>

  <!-- The picture of the game. Everything it shows is also in the text beside it. -->
  <div class="stage" aria-hidden="true">
    <Record {game} {sleeve} />
  </div>

  <main>
    {#if game.daily}
      {#if game.finished}
        <Reveal {game} />
      {:else}
        <Play {game} />
        <GuessInput onguess={(track) => game.guess(track)} busy={game.submitting} />
        <Skip {game} />
        {#if game.moveError}
          <p class="error" role="alert">{game.moveError}</p>
        {/if}
      {/if}
      <!-- Always in the page, so screen readers announce it when the text changes. The list below shows the same. -->
      <p class="sr-only" role="status">{game.finished ? '' : game.notice}</p>
      <Attempts {game} />
      <p class="rules">
        {#if !game.finished}Each skip or wrong guess unlocks a longer clip.{/if}
        A new song arrives every day at midnight UTC.
      </p>
    {:else if game.loadError}
      <div class="failed" role="alert">
        <p class="error">{game.loadError}</p>
        <button class="button solid" type="button" onclick={() => game.load()}>Try again</button>
      </div>
    {:else}
      <p role="status">Loading today's song…</p>
    {/if}
  </main>
</div>

<style>
  /* Stacked (phones, portrait): header, then the record hanging from under it, then the game. */
  .page {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    min-height: var(--screen);
    align-content: start;
  }

  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-4);
    padding: max(var(--space-3), env(safe-area-inset-top)) var(--gutter) var(--space-2);
  }

  h1 {
    font-size: var(--text-large);
    font-weight: var(--weight-heavy);
    line-height: 1;
  }

  /* The pressing number: the sleeve's only headline until the song is known. */
  .number {
    font-size: var(--text-number);
    font-weight: var(--weight-heavy);
    font-stretch: calc(var(--width-condensed) * 1%);
    line-height: var(--leading-display);
    min-height: calc(var(--text-number) * var(--leading-display));
    white-space: nowrap;
  }

  .stage {
    position: relative;
    height: calc(var(--record-y) + var(--record-r) + var(--space-3));
    overflow: hidden;
  }

  main {
    display: grid;
    gap: var(--space-4);
    align-content: start;
    padding: var(--space-3) var(--gutter) max(var(--space-6), env(safe-area-inset-bottom));
  }

  .rules {
    font-size: var(--text-small);
  }

  .failed {
    display: grid;
    justify-items: start;
    gap: var(--space-4);
  }

  /* Side by side: the record fills the left of the window and stays put; the game scrolls past it. */
  @media (min-width: 40rem) and (min-aspect-ratio: 6/5) {
    .page {
      grid-template-columns: calc(var(--record-r) * 1.5 + var(--space-6)) minmax(0, var(--column-width));
      column-gap: var(--space-5);
      padding-right: var(--gutter);
    }

    header,
    main {
      grid-column: 2;
      padding-left: 0;
      padding-right: 0;
    }

    header {
      padding-top: max(var(--space-5), env(safe-area-inset-top));
      padding-bottom: var(--space-5);
    }

    h1 {
      font-size: var(--text-title);
    }

    main {
      gap: var(--space-5);
      padding-top: 0;
    }

    .stage {
      position: fixed;
      inset: 0 auto 0 0;
      width: calc(var(--record-r) * 1.62);
      height: auto;
    }
  }
</style>
