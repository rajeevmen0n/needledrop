<script lang="ts">
  import { getHealth } from './lib/api'

  // Placeholder: proves the client can reach the server. Replaced by the game.
  const health = getHealth()
</script>

<main>
  <h1>Needledrop</h1>
  {#await health}
    <p>Checking the server…</p>
  {:then result}
    <p>Server says: <code>{JSON.stringify(result)}</code></p>
  {:catch error}
    <p>Server unreachable: {error.message}</p>
  {/await}
</main>
