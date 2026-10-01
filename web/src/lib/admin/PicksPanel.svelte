<script lang="ts">
  // Today's four answers, and the re-roll: for one section or for all four,
  // always behind a question, because it deletes every player's game there
  // today. There is deliberately no way to choose a song.
  import Confirm from '../components/Confirm.svelte'
  import { desk } from './desk.svelte'
  import { deezerLink, pickDetail, pickProblem, poolLabel, rerollQuestion } from './model'
  import type { AdminState, Pool } from './model'

  interface Props {
    /** The server's day as it last answered. */
    today: AdminState
  }

  let { today }: Props = $props()

  const uid = $props.id()

  let question = $state<Confirm>()
  /** What the question on screen is about: one section, or all four with `null`. */
  let target = $state<Pool | null>(null)
  /** Why asking again failed, and next to which section it was asked (`null`: none). */
  let error = $state<string | null>(null)
  let errorAt = $state<Pool | null>(null)

  const busy = $derived(desk.dayWork !== null)
  const asking = $derived(rerollQuestion(target))

  function ask(section: Pool | null) {
    target = section
    error = null
    question?.show()
  }

  /** Asks for the state again: the server tries the picks that are missing. */
  async function again(section: Pool) {
    error = null
    errorAt = section
    await desk.loadState()
    error = desk.stateError
  }
</script>

<section class="panel" aria-labelledby="{uid}-heading">
  <div class="panel-head">
    <h2 id="{uid}-heading">Today's picks</h2>
    <button class="button outline" type="button" onclick={() => ask(null)} disabled={busy}>Re-roll all</button>
  </div>
  <ul class="picks" aria-busy={busy}>
    {#each today.sections as entry (entry.section)}
      {@const label = poolLabel(entry.section)}
      <li class:problem={entry.status !== 'picked'}>
        <span class="section">{label}</span>
        <div class="song">
          {#if entry.status === 'picked' && entry.pick}
            <span class="title">{entry.pick.title}</span>
            <span class="meta"
              >{entry.pick.artist} ·
              <a href={deezerLink(entry.pick.trackId)} target="_blank" rel="noreferrer"
                >track <span class="numeric">{entry.pick.trackId}</span></a
              ></span
            >
          {:else}
            <span class="title trouble">{pickProblem(entry)}</span>
            <span class="meta">{pickDetail(entry)}</span>
          {/if}
          {#if error && errorAt === entry.section}<p class="error" role="alert">{error}</p>{/if}
        </div>
        <div class="tools">
          {#if entry.status !== 'picked'}
            <button class="button outline" type="button" onclick={() => again(entry.section)} disabled={busy}>
              {#if desk.dayWork === 'load'}Asking…{:else}{entry.status === 'unavailable'
                  ? 'Try again'
                  : 'Check again'}{/if}<span class="sr-only"> for {label}</span>
            </button>
          {/if}
          <button class="button outline" type="button" onclick={() => ask(entry.section)} disabled={busy}>
            Re-roll<span class="sr-only"> {label}</span>
          </button>
        </div>
      </li>
    {/each}
  </ul>
  {#if desk.dayWork === 'load'}
    <p class="working" role="status">Asking the server for the day and its picks…</p>
  {/if}
  <p class="news" role="status">{desk.picksNews}</p>
</section>

<Confirm
  bind:this={question}
  title={asking.title}
  confirm={asking.confirm}
  working="Re-rolling…"
  action={() => desk.reroll(target)}
>
  <p>{asking.body}</p>
  <p>It cannot be undone. A section with one song to choose from draws it again.</p>
</Confirm>

<style>
  .picks {
    display: grid;
  }

  li {
    display: grid;
    grid-template-columns: 5rem minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--space-2) var(--space-4);
    padding: var(--space-3) 0;
    border-top: var(--line) solid var(--border);
  }

  li:first-child {
    padding-top: 0;
    border-top: 0;
  }

  li:last-child {
    padding-bottom: 0;
  }

  .section {
    color: var(--muted);
    font-size: 0.875rem;
    font-weight: var(--weight-medium);
  }

  /* No song, or no answer from Deezer: ivory words with the "!" mark, never red. */
  .trouble {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .trouble::before {
    content: '!';
    flex: none;
    display: grid;
    place-items: center;
    width: 1.25em;
    height: 1.25em;
    border-radius: 50%;
    background: var(--paper);
    color: var(--ink);
    font-size: 0.875em;
    font-weight: var(--weight-heavy);
    line-height: 1;
  }

  .error {
    margin-top: var(--space-1);
  }

  .tools {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  [aria-busy='true'] .song {
    opacity: 0.6;
  }

  @media (max-width: 34rem) {
    li {
      grid-template-columns: minmax(0, 1fr) auto;
    }

    /* The section's name on a line of its own, above its song. */
    .section {
      grid-column: 1 / -1;
      margin-bottom: calc(var(--space-2) * -1 + 0.125rem);
    }

    li.problem {
      grid-template-columns: minmax(0, 1fr);
    }

    li.problem .tools {
      justify-content: flex-start;
    }
  }
</style>
