<script lang="ts">
  // The end of the game: the result, the song printed large, and the whole
  // clip to play. After a move the lines print in one after another; on a
  // reload they are simply there.
  //
  // What it is given as children is the way on, random mode's "Next song". It
  // comes straight after the song, so that on a phone it is reached without
  // scrolling past the cover; in the side layout it sits beside the play button.
  // `choice` is what the way on depends on, random mode's genre: it follows
  // the way on in reading order, under it in every layout.
  import { onMount } from 'svelte'
  import type { Snippet } from 'svelte'
  import type { Game } from '../game.svelte'
  import Play from './Play.svelte'

  let { game, children, choice }: { game: Game; children?: Snippet; choice?: Snippet } = $props()

  /** Titles longer than this are set a size down, so they do not fill the screen. */
  const LONG_TITLE = 22

  let heading = $state<HTMLHeadingElement>()
  // The sequence runs only for the player who just made the last move, and
  // only once: coming back to this tab later shows the settled text.
  const fresh = $derived(game.fresh)

  const result = $derived.by(() => {
    if (game.status !== 'won') return 'No tries left. The song was'
    if (game.turn === 1) return 'You got it on the first try.'
    return `You got it on try ${game.turn} of ${game.ladder.length}.`
  })

  onMount(() => {
    // The guess field just disappeared and took the keyboard focus with it.
    // After a reload nothing was focused, so leave the page alone.
    if (fresh) heading?.focus()
  })
</script>

<section
  class="reveal"
  class:fresh
  class:onwards={!!children}
  aria-labelledby="reveal-heading"
>
  <h2 id="reveal-heading" tabindex="-1" bind:this={heading}>
    <span class="mark" class:won={game.status === 'won'} aria-hidden="true">
      <svg viewBox="0 0 16 16" width="16" height="16">
        {#if game.status === 'won'}
          <path d="m3 8.5 3.5 3.5L13 4.5" />
        {:else}
          <path d="m3.5 3.5 9 9m0-9-9 9" />
        {/if}
      </svg>
    </span>
    {result}
  </h2>

  {#if game.answer}
    {@const answer = game.answer}
    <div class="song">
      <p class="title print" class:long={answer.title.length > LONG_TITLE}>{answer.title}</p>
      <p class="artist print">{answer.artist}</p>
    </div>
  {/if}
  {#if children}
    <div class="onward print">{@render children()}</div>
  {/if}
  {#if choice}
    <div class="choice print">{@render choice()}</div>
  {/if}
  {#if game.answer}
    {@const answer = game.answer}
    <div class="release print">
      {#if answer.cover}
        <img src={answer.cover} alt="Cover of {answer.album}" width="112" height="112" />
      {/if}
      <div>
        <p class="album">{answer.album}</p>
        <a href={answer.link} target="_blank" rel="noopener noreferrer">Listen on Deezer</a>
      </div>
    </div>
  {/if}

  <div class="listen print">
    <Play {game} />
  </div>
</section>

<style>
  .reveal {
    display: grid;
    gap: var(--space-5);
  }

  h2 {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    font-size: var(--text-large);
    font-weight: var(--weight-medium);
    line-height: 1.2;
  }

  /* The heading takes the focus when the game ends; it is not a control, so no ring. */
  h2:focus {
    outline: none;
    box-shadow: none;
  }

  .mark {
    flex: none;
    display: grid;
    place-items: center;
    width: 1.75rem;
    height: 1.75rem;
    border: var(--line) solid var(--paper);
    border-radius: 50%;
  }

  .mark.won {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--ink);
  }

  svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 2.4;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .song {
    display: grid;
    gap: var(--space-3);
  }

  /* The revealed song takes the display role previously held by the question. */
  .title {
    font-size: var(--text-display);
    font-weight: var(--weight-heavy);
    font-stretch: calc(var(--width-condensed) * 1%);
    line-height: var(--leading-display);
    letter-spacing: -0.01em;
    overflow-wrap: anywhere;
    text-wrap: balance;
  }

  .title.long {
    font-size: calc(var(--text-display) * 0.66);
    line-height: 0.94;
  }

  .artist {
    font-size: var(--text-title);
    font-weight: var(--weight-medium);
    line-height: 1.15;
    overflow-wrap: anywhere;
  }

  .release {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }

  .release > div {
    display: grid;
    gap: var(--space-1);
    min-width: 0;
    overflow-wrap: anywhere;
  }

  img {
    flex: none;
    width: 7rem;
    height: 7rem;
    object-fit: cover;
    background: var(--ink);
    box-shadow: var(--lift);
  }

  a {
    display: inline-flex;
    align-items: center;
    min-height: var(--target);
    font-weight: var(--weight-medium);
  }

  .onward {
    display: flex;
  }

  /* The choice belongs to the way on above it, so it sits closer to it than to what follows. */
  .choice {
    min-width: 0;
    margin-top: calc(var(--space-2) * -1);
  }

  /* Beside the record there is room: the way on moves down, next to listening again. */
  @media (min-width: 40rem) and (min-aspect-ratio: 6/5) {
    .onwards {
      grid-template-columns: minmax(0, 1fr) auto;
    }

    .onwards > * {
      grid-column: 1 / -1;
    }

    .onwards > .listen {
      grid-column: 1;
      grid-row: 4;
    }

    .onwards > .onward {
      grid-column: 2;
      grid-row: 4;
      align-self: center;
    }

    /* Under the row the way on is in. */
    .onwards > .choice {
      grid-row: 5;
      margin-top: 0;
    }
  }

  /* The reveal is uncovered in sequence as the artwork arrives on the label. */
  .fresh .print {
    animation: print 700ms var(--ease-out) both;
  }

  .fresh .title {
    animation-delay: calc(var(--reveal-step) * 4);
    animation-duration: 900ms;
  }

  .fresh .artist {
    animation-delay: calc(var(--reveal-step) * 7);
  }

  /* The way on follows the song it comes after, so it can be pressed without waiting for the rest. */
  .fresh .onward,
  .fresh .choice {
    animation-delay: calc(var(--reveal-step) * 8);
  }

  .fresh .release {
    animation-delay: calc(var(--reveal-step) * 9);
  }

  .fresh .listen {
    animation-delay: calc(var(--reveal-step) * 11);
  }

  @keyframes print {
    from {
      clip-path: inset(-1em 100% -1em -1em);
    }

    to {
      clip-path: inset(-1em);
    }
  }
</style>
