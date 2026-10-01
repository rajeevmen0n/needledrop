<script lang="ts">
  import { onMount } from 'svelte'
  import Attempts from './lib/components/Attempts.svelte'
  import GuessInput from './lib/components/GuessInput.svelte'
  import Play from './lib/components/Play.svelte'
  import Record from './lib/components/Record.svelte'
  import Reveal from './lib/components/Reveal.svelte'
  import Skip from './lib/components/Skip.svelte'
  import { game } from './lib/game.svelte'
  onMount(() => {
    void game.load()
    const onvisible = () => {
      if (document.visibilityState === 'visible') void game.refresh()
    }
    document.addEventListener('visibilitychange', onvisible)
    return () => document.removeEventListener('visibilitychange', onvisible)
  })
</script>

<div class="page">
  <header>
    <h1>
      <span class="brand-disc" aria-hidden="true">◎</span> Needledrop<span
        class="brand-note">The daily listening game</span
      >
    </h1>
    <p class="number numeric">
      <span>Daily pressing</span>{game.daily
        ? `No. ${String(game.daily.number).padStart(3, '0')}`
        : 'No. —'}
    </p>
  </header>
  <div class="stage" aria-hidden="true">
    <Record {game} />
    <div class="record-caption">
      <span>One record. Seven chances.</span><span class="numeric"
        >33⅓ rpm · Stereo</span
      >
    </div>
  </div>
  <main>
    {#if game.daily}
      {#if game.finished}<Reveal {game} />
      {:else}
        <div class="intro">
          <p class="eyebrow">Put your music memory to the test</p>
          <h2>Know it from <br /><span>the first note?</span></h2>
          <p>Listen closely. Name today's mystery song.</p>
        </div>
      <div class="clip-steps" aria-hidden="true">
          {#each game.ladder as seconds, i}<div
              class:unlocked={seconds <= game.clipSeconds}
              class:current={i === game.turn - 1}
            >
              <span class="step-line"></span><span class="numeric"
                >{seconds}<span class="unit">s</span></span
              >
            </div>{/each}
        </div>
        <Play {game} />
        <GuessInput
          onguess={(track) => game.guess(track)}
          busy={game.submitting}
        />
        <Skip {game} />
        {#if game.moveError}<p class="error" role="alert">
            {game.moveError}
          </p>{/if}
      {/if}
      <p class="sr-only" role="status">{game.finished ? '' : game.notice}</p>
      <Attempts {game} />
      <p class="rules">
        {#if game.finished}
          Come back tomorrow for a fresh game.
        {:else}
          Each skip or wrong guess unlocks a longer clip.
        {/if}
      </p>
    {:else if game.loadError}
      <div class="failed" role="alert">
        <p class="error">{game.loadError}</p>
        <button class="button solid" type="button" onclick={() => game.load()}
          >Try again</button
        >
      </div>
    {:else}<p role="status">Loading today's song…</p>{/if}
  </main>
  <footer>
    <span>A little sound. A familiar feeling.</span><span
      >Music previews by Deezer</span
    >
  </footer>
</div>

<style>
  .page {
    max-width: 1440px;
    margin: auto;
    min-height: var(--screen);
    padding: 0 var(--gutter);
    display: grid;
    grid-template-columns: minmax(0, 1fr);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: max(1.25rem, env(safe-area-inset-top)) 0 1.25rem;
    border-bottom: 1px solid var(--border);
  }
  h1 {
    font-size: 1.35rem;
    font-weight: 800;
    letter-spacing: -0.04em;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .brand-disc {
    font-size: 2rem;
    color: var(--accent);
    line-height: 1;
  }
  .brand-note {
    font-size: 0.75rem;
    font-weight: 400;
    color: var(--muted);
    letter-spacing: 0;
    display: none;
    margin-left: 1.5rem;
  }
  .number {
    text-align: right;
    font-size: 0.9375rem;
  }
  .number span {
    display: block;
    font-size: 0.6875rem;
    color: var(--muted);
  }
  .stage {
    position: relative;
    height: calc(var(--record-r) * 2.45);
    margin: 0 calc(var(--gutter) * -1);
  }
  .record-caption {
    display: none;
  }
  main {
    display: grid;
    width: 100%;
    max-width: 34rem;
    justify-self: center;
    gap: 1.25rem;
    align-content: start;
    padding: 0 0 2rem;
    min-width: 0;
  }
  .intro {
    display: grid;
    gap: 0.75rem;
  }
  .eyebrow {
    color: var(--accent);
    font-size: 0.75rem;
  }
  .intro h2 {
    font-size: clamp(2.35rem, 8vw, 3.2rem);
    font-weight: 600;
    font-stretch: 100%;
    letter-spacing: -0.035em;
    line-height: 1.04;
  }
  .intro h2 span {
    color: var(--accent);
  }
  .intro > p:last-child {
    color: var(--muted);
    font-size: 0.875rem;
  }
  .clip-steps {
    display: grid;
    grid-template-columns: repeat(7, minmax(0, 1fr));
    gap: 0.5rem;
  }
  .clip-steps > div {
    display: grid;
    gap: 0.5rem;
    color: var(--muted);
    font-size: 0.75rem;
  }
  .step-line {
    height: 3px;
    background: var(--border);
    border-radius: 2px;
  }
  .unlocked .step-line {
    background: var(--accent);
  }
  .clip-steps .current {
    color: var(--accent);
  }
  .unit {
    margin-left: 0.15em;
    font-size: 0.625rem;
  }
  .rules {
    color: var(--muted);
    font-size: 0.75rem;
    line-height: 1.7;
    max-width: 25rem;
  }
  .failed {
    display: grid;
    justify-items: start;
    gap: 1rem;
  }
  footer {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    border-top: 1px solid var(--border);
    padding: 1.25rem 0 max(1.25rem, env(safe-area-inset-bottom));
    color: var(--muted);
    font-size: 0.6875rem;
  }
  @media (max-width: 55.99rem), (max-aspect-ratio: 6/5) {
    .eyebrow {
      display: none;
    }
    .intro h2 br {
      display: none;
    }
    .intro h2 {
      font-size: 1.65rem;
    }
    .intro > p:last-child {
      display: none;
    }
    main {
      gap: 1rem;
    }
    .stage {
      height: calc(var(--record-r) * 2.35);
    }
  }
  @media (min-width: 40rem) and (min-aspect-ratio: 6/5) {
    .page {
      grid-template-columns: minmax(0, 1.14fr) minmax(0, 1fr);
      column-gap: clamp(2rem, 6vw, 6rem);
    }
    header,
    footer {
      grid-column: 1/-1;
    }
    .brand-note {
      display: inline;
    }
    header {
      padding-top: 2rem;
      padding-bottom: 1.75rem;
    }
    .stage {
      grid-column: 1;
      grid-row: 2;
      margin: 0;
      align-self: center;
      height: calc(var(--record-r) * 2.8);
    }
    .record-caption {
      position: absolute;
      bottom: 0;
      left: 0;
      right: 0;
      display: flex;
      justify-content: space-between;
      font-size: 0.75rem;
      color: var(--muted);
    }
    main {
      grid-column: 2;
      grid-row: 2;
      max-width: 29rem;
      padding: 3.5rem 0;
    }
    .intro {
      gap: 1rem;
      margin-bottom: 0.75rem;
    }
    .intro h2 {
      font-size: clamp(2.75rem, 4.2vw, 3.75rem);
    }
    .intro h2 br {
      display: block;
    }
    .intro > p:last-child {
      display: block;
    }
    .eyebrow {
      display: block;
    }
    footer {
      margin-top: auto;
    }
  }
  @media (max-height: 600px) and (min-width: 40rem) and (min-aspect-ratio: 6/5) {
    main {
      padding: 1.5rem 0;
      gap: 0.8rem;
    }
    .intro h2 {
      font-size: 2.25rem;
    }
    .eyebrow {
      display: none;
    }
    .intro {
      margin-bottom: 0;
    }
    .brand-note {
      display: none;
    }
    header {
      padding: 1rem 0;
    }
    .stage {
      position: sticky;
      top: 1rem;
      align-self: start;
      margin-top: 1.5rem;
    }
  }
  @media (max-width: 39.99rem) and (max-height: 650px) {
    .intro {
      display: none;
    }
    header {
      padding: 1rem 0;
    }
    .stage {
      height: calc(var(--record-r) * 2.3);
    }
    main {
      gap: 0.75rem;
    }
  }
</style>
