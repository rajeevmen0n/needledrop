<script lang="ts">
  import { onMount } from 'svelte'
  // The game page: the four sections as tabs, and the game of the one on
  // screen. Which section that is comes from the address (see main.ts) and
  // goes back into it: opening a tab is an entry in the browser's history.
  import Atmosphere from './lib/components/Atmosphere.svelte'
  import Attempts from './lib/components/Attempts.svelte'
  import ClearData from './lib/components/ClearData.svelte'
  import GuessInput from './lib/components/GuessInput.svelte'
  import Play from './lib/components/Play.svelte'
  import Record from './lib/components/Record.svelte'
  import Reveal from './lib/components/Reveal.svelte'
  import SectionTabs from './lib/components/SectionTabs.svelte'
  import Skip from './lib/components/Skip.svelte'
  import Stats from './lib/components/Stats.svelte'
  import { game } from './lib/game.svelte'
  import {
    pageTitle,
    routeFor,
    sectionLabel,
    sectionPath,
    tabId,
  } from './lib/sections'
  import type { Section } from './lib/sections'
  import { winningTry } from './lib/stats'

  /** The section the address asked for when the page loaded. */
  let { section }: { section: Section } = $props()

  const PANEL = 'game-panel'

  const CLEARED = 'Your data was cleared. This browser is now a new player.'

  let motionPaused = $state(false)
  let gameRegion: HTMLElement
  /** Clear my data went through, and nothing has been played since. */
  let cleared = $state(false)

  const label = $derived(sectionLabel(game.section))
  /** Another section still has a game to play today. */
  const more = $derived(
    game.tabs.some(
      (tab) =>
        tab.section !== game.section &&
        (tab.state === 'unplayed' || tab.state === 'playing'),
    ),
  )
  // One sentence at a time for a screen reader: news the player did not cause, else what the last move did.
  const spoken = $derived(game.info ?? (game.finished ? '' : game.notice))

  /** Clear my data. Resolves to `null` when it is done, or to why it failed. */
  async function clear(): Promise<string | null> {
    cleared = false
    const failure = await game.clearData()
    cleared = failure === null
    return failure
  }

  // The line that says so stays until the new player has a game on record.
  $effect(() => {
    if (game.tabs.some((tab) => tab.state !== 'unknown' && tab.state !== 'unplayed' && tab.state !== 'none'))
      cleared = false
  })

  /** A tab was chosen on the page: the address follows it. */
  function choose(next: Section) {
    if (next === game.section) return
    gameRegion.scrollTop = 0
    history.pushState(null, '', sectionPath(next))
    game.select(next)
  }

  onMount(() => {
    game.open(section)
    const onvisible = () => {
      if (document.visibilityState === 'visible') game.refresh()
    }
    // Back and forward: the address has changed already, and the page follows it.
    const onpop = () => {
      const route = routeFor(location.pathname)
      if (route.page === 'game') {
        gameRegion.scrollTop = 0
        game.select(route.section)
      }
      else location.reload()
    }
    document.addEventListener('visibilitychange', onvisible)
    window.addEventListener('popstate', onpop)
    return () => {
      document.removeEventListener('visibilitychange', onvisible)
      window.removeEventListener('popstate', onpop)
    }
  })

  $effect(() => {
    document.title = pageTitle(game.section)
  })
</script>

<Atmosphere
  playing={game.playing}
  paused={motionPaused}
  status={game.daily ? game.status : 'loading'}
  scene={game.section}
  analyser={() => game.player.analyser}
/>
<div class="page" class:still={motionPaused}>
  <header>
    <h1 data-sky-calm>
      <!-- A record with a point of light on its rim: the stylus, or the last of an eclipse. -->
      <svg class="brand-mark" viewBox="0 0 28 28" aria-hidden="true">
        <circle cx="13" cy="15" r="9.25" />
        <circle class="groove" cx="13" cy="15" r="5.2" />
        <circle class="spindle" cx="13" cy="15" r="1.6" />
        <path
          class="spark"
          d="M20.2 2.4Q20.9 7.1 25.6 7.8 20.9 8.5 20.2 13.2 19.5 8.5 14.8 7.8 19.5 7.1 20.2 2.4Z"
        />
      </svg>
      Needledrop<span class="brand-note">The daily listening game</span>
    </h1>
    <p class="number numeric" data-sky-calm>
      <span>Daily pressing</span>{game.number !== null
        ? `No. ${String(game.number).padStart(3, '0')}`
        : 'No. —'}
    </p>
  </header>
  <div class="sections">
    <SectionTabs
      tabs={game.tabs}
      selected={game.section}
      panel={PANEL}
      onselect={choose}
    />
  </div>
  <div class="stage" class:playing={game.playing} aria-hidden="true">
    <!-- Tells the sky where the record is: left/top is its centre, width its radius. -->
    <span class="origin" data-sky-origin></span>
    <Record {game} />
    <div class="record-caption" data-sky-calm>
      <span>One record. Seven chances.</span><span class="numeric"
        >33⅓ rpm · Stereo</span
      >
    </div>
  </div>
  <main bind:this={gameRegion} data-sky-calm>
    <!-- Always there, so that what it comes to say is announced. -->
    <p class="sr-only" role="status">{spoken}</p>
    {#key game.section}
      <div
        class="panel"
        id={PANEL}
        role="tabpanel"
        aria-labelledby={tabId(game.section)}
      >
        {#if game.info}<p class="info" aria-hidden="true">{game.info}</p>{/if}
        {#if game.daily}
          {#if game.finished}
            <Reveal {game} />
            {#if game.stats}
              <Stats
                stats={game.stats}
                title="Your {label} stats"
                todaysTry={winningTry(game.status, game.attempts.length)}
              />
            {/if}
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
          <Attempts {game} />
          <p class="rules">
            {#if !game.finished}
              Each skip or wrong guess unlocks a longer clip.
            {:else if more}
              {label} is back tomorrow. Today's other sections are still open.
            {:else}
              Come back tomorrow for four fresh games.
            {/if}
          </p>
        {:else if game.noSong}
          <div class="empty">
            <h2>No song today</h2>
            <p>
              {label} has nothing to play today. Try another section, or come
              back tomorrow.
            </p>
            {#if game.loadError}<p class="error" role="alert">
                {game.loadError}
              </p>{/if}
            <button
              class="button outline"
              type="button"
              onclick={() => game.load()}
              disabled={game.loading}
              >{game.loading ? 'Checking…' : 'Check again'}</button
            >
          </div>
        {:else if game.loadError}
          <div class="failed" role="alert">
            <p class="error">{game.loadError}</p>
            <button
              class="button solid"
              type="button"
              onclick={() => game.load()}>Try again</button
            >
          </div>
        {:else}<p class="loading" role="status">Loading today's song…</p>{/if}
      </div>
    {/key}
  </main>
  <footer data-sky-calm>
    <span>A little sound. A familiar feeling.</span><span
      >Music previews by Deezer</span
    >
    <div class="switches">
      <ClearData onclear={clear} disabled={game.busy} />
      <button
        class="quiet-button motion-toggle"
        type="button"
        onclick={() => (motionPaused = !motionPaused)}
      >
        {motionPaused ? 'Resume motion' : 'Pause motion'}
      </button>
    </div>
    <!-- The live region is there before it has anything to say, so the sentence is announced when it arrives. -->
    <p class="sr-only" role="status">{cleared ? CLEARED : ''}</p>
    {#if cleared}<p class="cleared" aria-hidden="true">{CLEARED}</p>{/if}
  </footer>
</div>

<style>
  @media (prefers-reduced-motion: reduce) { .motion-toggle { display: none; } }
  .page {
    position: relative;
    z-index: 1;
    max-width: 1440px;
    margin: auto;
    height: var(--screen);
    padding: 0 var(--gutter);
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    /* Header, tabs, record, game, footer: room to spare goes to the game, not to the bars above it. */
    grid-template-rows: auto auto auto minmax(0, 1fr) auto;
    /* The light behind the record is larger than its stage; it must not lengthen the page. */
    overflow-y: clip;
  }
  @media (max-width: 1440px) {
    /* The page spans the viewport here, so the same goes sideways. */
    .page {
      overflow-x: clip;
    }
  }

  /* One arrival, in order: the header, the record rising with its light, the game. */
  header {
    animation: arrive 700ms var(--ease-out) backwards;
  }
  .sections {
    animation: arrive 700ms var(--ease-out) var(--reveal-step) backwards;
  }
  .stage {
    animation: rise 1100ms var(--ease-out) calc(var(--reveal-step) * 2)
      backwards;
  }
  main {
    animation: settle 800ms var(--ease-out) calc(var(--reveal-step) * 6)
      backwards;
  }
  footer {
    animation: arrive 900ms var(--ease-out) calc(var(--reveal-step) * 10)
      backwards;
  }
  @keyframes arrive {
    from {
      opacity: 0;
    }
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(1.25rem) scale(0.965);
    }
  }
  @keyframes settle {
    from {
      opacity: 0;
      transform: translateY(0.6rem);
    }
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
  .brand-mark {
    flex: none;
    width: 1.75rem;
    height: 1.75rem;
    color: var(--accent);
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
  }
  .brand-mark .groove {
    stroke-width: 1;
    opacity: 0.45;
  }
  .brand-mark .spindle {
    fill: currentColor;
    stroke: none;
  }
  /* The dark outline lifts the point of light off the rim it sits on. */
  .brand-mark .spark {
    fill: currentColor;
    stroke: var(--field);
    stroke-width: 1.6;
    paint-order: stroke;
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
    height: calc(var(--record-r) * 2.3);
    margin: 0 calc(var(--gutter) * -1);
  }
  /* Light from behind the record, as around an eclipsed sun: brightest at the
     edge of the disc, gone within a radius or so. The disc itself hides the rest. */
  .stage::before {
    content: '';
    position: absolute;
    z-index: -1;
    left: calc(var(--record-x) - var(--record-r) * 2.2);
    top: calc(var(--record-y) - var(--record-r) * 2.2);
    width: calc(var(--record-r) * 4.4);
    height: calc(var(--record-r) * 4.4);
    border-radius: 50%;
    /* closest-side: 100% is 2.2 radii, so the disc's edge is at 45.5%. */
    background: radial-gradient(
      closest-side,
      transparent 40%,
      rgb(var(--sky-warm) / 0.3) 45%,
      rgb(var(--sky-warm) / 0.13) 51%,
      rgb(var(--sky-warm) / 0.055) 62%,
      rgb(var(--sky-warm) / 0.018) 80%,
      transparent
    );
    opacity: 0.55;
    pointer-events: none;
    transition: opacity 1400ms var(--ease-out);
    animation: bloom 2400ms var(--ease-out) calc(var(--reveal-step) * 8)
      backwards;
  }
  .stage.playing::before {
    opacity: 1;
    transition-duration: 700ms;
  }
  @keyframes bloom {
    from {
      opacity: 0;
      transform: scale(0.9);
    }
  }
  .origin {
    position: absolute;
    left: var(--record-x);
    top: var(--record-y);
    width: var(--record-r);
    height: 0;
    visibility: hidden;
    pointer-events: none;
  }
  .record-caption {
    display: none;
  }
  main {
    width: 100%;
    max-width: 34rem;
    justify-self: center;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-gutter: stable;
    padding: 0.5rem 0 1rem;
    min-width: 0;
  }
  /* One section's game. A tab that is opened fades in; nothing slides. */
  .panel {
    display: grid;
    gap: 1.25rem;
    align-content: start;
    min-width: 0;
    animation: arrive 320ms var(--ease-out) both;
  }
  /* News the player did not cause: a replaced song, a new day. */
  .info {
    padding-left: 0.75rem;
    border-left: 2px solid var(--accent);
    font-size: 0.875rem;
    line-height: 1.4;
  }
  .loading {
    color: var(--muted);
  }
  .empty {
    display: grid;
    justify-items: start;
    gap: 0.75rem;
  }
  .empty h2 {
    font-size: 1.65rem;
    font-weight: 600;
    letter-spacing: -0.03em;
    line-height: 1.1;
  }
  .empty > p:not(.error) {
    max-width: 25rem;
    color: var(--muted);
    font-size: 0.875rem;
  }
  .empty .button {
    min-height: var(--target);
    margin-top: 0.25rem;
    border-color: var(--control-border);
    font-size: 0.875rem;
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
    position: relative;
    height: 3px;
    background-color: var(--border);
    border-radius: 2px;
    transition: background-color 420ms var(--ease-out);
  }
  .unlocked .step-line {
    background-color: var(--accent);
  }
  /* The step in play: a glint crosses it once as it unlocks, then it glows and slowly breathes. */
  .current .step-line {
    background-color: var(--accent);
    background-image: linear-gradient(
      100deg,
      transparent 38%,
      var(--paper) 50%,
      transparent 62%
    );
    background-repeat: no-repeat;
    background-size: 300% 100%;
    /* At rest the bright band waits past the right end. */
    background-position: 0 0;
    animation: glint 1100ms var(--ease-out) 200ms backwards;
  }
  .current .step-line::after {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: inherit;
    box-shadow: 0 0 0.6rem 1px rgb(var(--sky-warm) / 0.75);
    animation: breathe 3.2s ease-in-out infinite alternate;
  }
  .still .current .step-line::after {
    animation-play-state: paused;
  }
  @keyframes glint {
    from {
      background-position: 100% 0;
    }
  }
  @keyframes breathe {
    from {
      opacity: 0.3;
    }
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
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    border-top: 1px solid var(--border);
    padding: 1.25rem 0 max(1.25rem, env(safe-area-inset-bottom));
    color: var(--muted);
    font-size: 0.6875rem;
  }
  .switches {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    /* The buttons' own padding stays out of the page's edge. */
    margin: 0 -0.5rem;
  }
  /* A line of its own under the footer's row. */
  .cleared {
    flex-basis: 100%;
    color: var(--paper);
    font-size: 0.75rem;
  }
  @media (prefers-reduced-motion: reduce) {
    header,
    .sections,
    .stage,
    .stage::before,
    main,
    .panel,
    footer,
    .current .step-line,
    .current .step-line::after {
      animation: none;
    }
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
    .panel {
      gap: 1rem;
    }
    .stage {
      height: calc(var(--record-r) * 2.3);
    }
  }
  @media (min-width: 40rem) and (min-aspect-ratio: 6/5) {
    .page {
      grid-template-columns: minmax(0, 1.14fr) minmax(0, 1fr);
      column-gap: clamp(2rem, 6vw, 6rem);
    }
    .page {
      grid-template-rows: auto auto minmax(0, 1fr) auto;
    }
    header,
    .sections,
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
      grid-row: 3;
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
      grid-row: 3;
      max-width: 29rem;
      padding: 1.25rem 0;
    }
    .panel {
      gap: 0.85rem;
    }
    .intro {
      gap: 0.75rem;
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
  @media (min-width: 40rem) and (min-aspect-ratio: 6/5) and (max-height: 800px) {
    .intro .eyebrow,
    .intro > p:last-child {
      display: none;
    }
    .intro h2 {
      font-size: 2.25rem;
    }
  }
  @media (max-height: 600px) and (min-width: 40rem) and (min-aspect-ratio: 6/5) {
    main {
      padding: 0.75rem 0;
    }
    .panel {
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
  }
  @media (max-width: 39.99rem) and (max-height: 650px) {
    .intro {
      display: none;
    }
    /* The tabs took a row: the header gives some of it back, so Skip still fits a 320×568 screen. */
    header {
      padding: 0.625rem 0;
    }
    .stage {
      height: calc(var(--record-r) * 2.3);
    }
    .panel {
      gap: 0.75rem;
    }
  }
  @media (max-width: 39.99rem) {
    /* The record and primary controls share one phone screen. The notes and
       long reveals scroll inside the game region when the content needs it. */
    .intro {
      display: none;
    }
    .panel {
      gap: 0.75rem;
    }
    footer {
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
      gap: 0.5rem;
      padding: 0.5rem 0 max(0.5rem, env(safe-area-inset-bottom));
    }
    footer > span:first-child {
      display: none;
    }
    .switches {
      flex-wrap: nowrap;
      margin: 0 -0.5rem 0 0;
    }
    .cleared {
      grid-column: 1 / -1;
    }
  }
  @media (max-width: 22.5rem) {
    footer {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
