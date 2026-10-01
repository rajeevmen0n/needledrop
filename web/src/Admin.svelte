<script lang="ts">
  // The admin page: the clock, today's picks with the re-roll, adding a song
  // from Deezer, and finding a song in the pool to change or remove it. main.ts mounts it for /admin and never the game, and fetches
  // it only there, so nothing here is in the player's bundle.
  // The deployment's reverse proxy protects this page and /api/admin/.
  import './lib/admin/admin.css'
  import AddSong from './lib/admin/AddSong.svelte'
  import ClockPanel from './lib/admin/ClockPanel.svelte'
  import { desk } from './lib/admin/desk.svelte'
  import FindSong from './lib/admin/FindSong.svelte'
  import { finder } from './lib/admin/finder.svelte'
  import PicksPanel from './lib/admin/PicksPanel.svelte'

  // The day (which can take seconds: the server makes any pick that is missing) and the pool's counts, side by side.
  void desk.loadState()
  finder.open()
</script>

<svelte:head><title>Admin · Needledrop</title></svelte:head>

<div class="admin">
  <header>
    <div class="top">
      <h1>Needledrop <span>admin</span></h1>
      <a class="back" href="/">Back to the game</a>
    </div>
  </header>
  <main>
    {#if desk.state}
      <ClockPanel today={desk.state} />
      <PicksPanel today={desk.state} />
    {:else}
      <section class="panel" aria-labelledby="day-heading">
        <div class="panel-head"><h2 id="day-heading">The day</h2></div>
        {#if desk.stateError && desk.dayWork === null}
          <p class="error" role="alert">{desk.stateError}</p>
          <div>
            <button class="button outline" type="button" onclick={() => desk.loadState()}>Try again</button>
          </div>
        {:else}
          <p class="working" role="status">
            Loading the day and its picks… The server first picks any song that
            is missing, which can take a few seconds.
          </p>
        {/if}
      </section>
    {/if}
    <AddSong />
    <FindSong />
  </main>
</div>

<style>
  header {
    display: grid;
    gap: var(--space-2);
  }

  .top {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0 var(--space-4);
  }

  h1 {
    font-size: var(--text-title);
    font-weight: var(--weight-heavy);
    letter-spacing: -0.03em;
    line-height: 1.1;
  }

  h1 span {
    color: var(--muted);
    font-weight: var(--weight-regular);
  }

  .back {
    display: inline-flex;
    align-items: center;
    min-height: var(--target);
    font-size: 0.875rem;
  }

  main {
    display: grid;
    gap: var(--space-6);
    min-width: 0;
  }
</style>
