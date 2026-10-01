<script lang="ts">
  // The admin page. Task 14 builds it here: the clock, today's picks with
  // re-roll, adding a song by search, and the pool. Until then this is only
  // the place where it goes; main.ts mounts it for /admin and never the game.
</script>

<svelte:head><title>Admin · Needledrop</title></svelte:head>

<main>
  <h1>Needledrop admin</h1>
  <p>Admin is not built yet.</p>
  <p><a href="/">Back to the game</a></p>
</main>

<style>
  main {
    display: grid;
    gap: var(--space-3);
    max-width: 34rem;
    margin: 0 auto;
    padding: max(var(--space-6), env(safe-area-inset-top)) var(--gutter)
      var(--space-6);
  }

  h1 {
    font-size: var(--text-large);
    font-weight: var(--weight-heavy);
    letter-spacing: -0.03em;
  }

  p {
    color: var(--muted);
  }

  a {
    display: inline-flex;
    align-items: center;
    min-height: var(--target);
    color: var(--paper);
  }
</style>
