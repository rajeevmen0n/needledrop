<script lang="ts">
  // The end of the game: the result, the song, and the whole clip to play.
  import { onMount } from 'svelte'
  import { clipLabel } from '../clip'
  import type { Game } from '../game.svelte'
  import Timeline from './Timeline.svelte'

  let { game }: { game: Game } = $props()

  let heading = $state<HTMLHeadingElement>()

  const result = $derived.by(() => {
    if (game.status !== 'won') return 'No tries left. The song was:'
    if (game.turn === 1) return 'You got it on the first try.'
    return `You got it on try ${game.turn} of ${game.ladder.length}.`
  })

  onMount(() => {
    // The guess field just disappeared and took the keyboard focus with it.
    // After a reload nothing was focused, so leave the page alone.
    if (game.moves > 0) heading?.focus()
  })
</script>

<section class="reveal" aria-labelledby="reveal-heading">
  <h2 id="reveal-heading" tabindex="-1" bind:this={heading}>
    <span class="mark" aria-hidden="true">{game.status === 'won' ? '✓' : '✕'}</span>
    {result}
  </h2>

  {#if game.answer}
    {@const answer = game.answer}
    <div class="song">
      {#if answer.cover}
        <img src={answer.cover} alt="Cover of {answer.album}" width="160" height="160" />
      {/if}
      <div class="details">
        <p class="title">{answer.title}</p>
        <p class="artist">{answer.artist}</p>
        <p class="muted">{answer.album}</p>
        <p><a href={answer.link} target="_blank" rel="noopener noreferrer">Listen on Deezer</a></p>
      </div>
    </div>
  {/if}

  <button class="primary" type="button" onclick={() => game.toggle()}>
    {game.playing ? 'Stop' : `Play the full ${clipLabel(game.clipSeconds)}`}
  </button>
  {#if game.clipError}
    <p class="error" role="alert">{game.clipError}</p>
  {/if}
  <Timeline {game} />

  <p class="muted">A new song arrives every day at midnight UTC.</p>
</section>

<style>
  .reveal {
    display: grid;
    gap: var(--space-4);
  }

  h2 {
    font-size: var(--text-large);
    line-height: 1.25;
  }

  .song {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-4);
  }

  img {
    flex: none;
    width: 10rem;
    height: 10rem;
    border: var(--border);
    border-radius: var(--radius);
    object-fit: cover;
  }

  .details {
    display: grid;
    align-content: start;
    gap: var(--space-1);
    min-width: 10rem;
    flex: 1;
    overflow-wrap: anywhere;
  }

  .title {
    font-size: var(--text-title);
    font-weight: 700;
    line-height: 1.2;
  }

  .artist {
    font-size: var(--text-large);
  }

  a {
    display: inline-flex;
    align-items: center;
    min-height: var(--control-height);
    font-weight: 600;
  }
</style>
