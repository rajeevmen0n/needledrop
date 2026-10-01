<script lang="ts">
  // Find a song in the pool, to change its genres or remove it: a search box
  // and its results, nothing more. The pool is meant to hold hundreds of
  // songs, so it is neither loaded nor listed: the server searches it, and the
  // page shows the first screenful of what the text finds. No song is asked
  // for until something is typed; the totals are a line to read.
  //
  // The one way to rows without typing is the quiet "show them" after the
  // number of songs whose preview check failed, which is there only while
  // there are any: nobody would know what to search for.
  //
  // Genres are changed in place; Remove asks first. Either asks the same
  // search again, so the text and the totals stay in step.
  import Confirm from '../components/Confirm.svelte'
  import { desk } from './desk.svelte'
  import { finder } from './finder.svelte'
  import GenreBoxes from './GenreBoxes.svelte'
  import {
    deezerLink,
    failedText,
    foundText,
    moreText,
    poolNotes,
    poolsText,
    removalText,
    sectionsPlaying,
    songName,
    totalsText,
  } from './model'
  import type { Genre, SongRow } from './model'

  const uid = $props.id()

  let heading = $state<HTMLHeadingElement>()
  let question = $state<Confirm>()
  /** The song the question on screen is about. */
  let target = $state<SongRow | null>(null)
  /** The genres asked for, per song, while the change is on its way. */
  let saving = $state<Record<number, Genre[]>>({})
  /** Why a song's last change failed, per song. */
  let errors = $state<Record<number, string>>({})
  /** What the last change did: said aloud for a retag, shown for a removal. */
  let spoken = $state('')
  let removed = $state('')

  const page = $derived(finder.page)
  // An idle search lists nothing, whatever the answer holds.
  const rows = $derived(page && !finder.idle ? page.songs : [])
  const found = $derived(page ? foundText(finder.asked, page.total) : '')
  const more = $derived(page && !finder.idle ? moreText(page.songs.length, page.total) : '')

  function type(text: string) {
    removed = ''
    finder.type(text)
  }

  function showFailed(on: boolean) {
    removed = ''
    finder.showFailed(on)
  }

  async function retag(song: SongRow, genres: Genre[]) {
    const id = song.trackId
    if (saving[id]) return
    saving[id] = genres
    delete errors[id]
    spoken = ''
    const answer = await finder.retag(id, genres)
    delete saving[id]
    if (typeof answer === 'string') {
      errors[id] = answer
      return
    }
    spoken = `${songName(answer)} is now in ${poolsText(answer.genres)}.`
  }

  function ask(song: SongRow) {
    target = song
    question?.show()
  }

  async function remove(): Promise<string | null> {
    if (!target) return null
    const song = target
    const failure = await finder.remove(song.trackId)
    if (failure === null) {
      delete errors[song.trackId]
      removed = `Removed ${songName(song)} from the pool.`
    }
    return failure
  }
</script>

<section class="panel" aria-labelledby="{uid}-heading">
  <div class="panel-head">
    <h2 id="{uid}-heading" bind:this={heading} tabindex="-1">Find a song in the pool</h2>
    {#if page}
      <button class="quiet-button" type="button" onclick={() => finder.again()} disabled={finder.searching}
        >Refresh</button
      >
    {/if}
  </div>
  {#if page}
    <p class="totals numeric">
      <span>{totalsText(page.counts)}</span>
      {#if page.counts.previewFailed > 0 || finder.failed}
        <span class="failures"
          >{failedText(page.counts) || 'None failed the preview check'}
          <button type="button" onclick={() => showFailed(!finder.failed)}
            >{finder.failed ? 'hide them' : 'show them'}</button
          ></span
        >
      {/if}
    </p>
    <div class="search">
      <label for="{uid}-input">Search the pool</label>
      <input
        id="{uid}-input"
        type="search"
        value={finder.text}
        oninput={(event) => type(event.currentTarget.value)}
        placeholder="Title, artist, album or track ID"
        aria-describedby="{uid}-help"
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        enterkeyhint="search"
      />
      <p class="hint" id="{uid}-help">
        Finds songs that are in the pool already, to change their genres or
        remove them. Nothing is listed until you type.
      </p>
    </div>
    {#each poolNotes(page.counts) as note}<p class="note">{note}</p>{/each}
    {#if finder.error}
      <div class="failed-search" role="alert">
        <p class="error">{finder.error}</p>
        <button class="button outline" type="button" onclick={() => finder.again()} disabled={finder.searching}
          >Try again</button
        >
      </div>
    {/if}
    <div class="results" aria-busy={finder.searching}>
      <p class="found" role="status">
        {#if finder.searching}<span class="working">Searching…</span>{:else}{found}{#if more}{' '}<span class="more"
              >{more}</span
            >{/if}{/if}
      </p>
      {#if finder.failed && !finder.searching && rows.length > 0}
        <p class="hint">
          A song without a preview is skipped on the day it fails and tried
          again later. Remove the ones that stay that way.
        </p>
      {/if}
      <p class="news" role="status">{removed}</p>
      <p class="sr-only" role="status">{spoken}</p>
      {#if rows.length > 0}
        <ul class="songs">
          {#each rows as song (song.trackId)}
            {@const pending = saving[song.trackId]}
            <li class:failed={song.previewFailedOn !== null}>
              <div class="song">
                <span class="title">{song.title}</span>
                <span class="meta"
                  >{song.artist}{song.album ? ` · ${song.album}` : ''} ·
                  <a href={deezerLink(song.trackId)} target="_blank" rel="noreferrer"
                    >track <span class="numeric">{song.trackId}</span></a
                  ></span
                >
                {#if song.previewFailedOn}
                  <p class="error">No preview at the daily check on {song.previewFailedOn}</p>
                {/if}
              </div>
              <div class="genres">
                {#if pending}<span class="working">Saving…</span>{/if}
                <GenreBoxes
                  legend="Genres of {song.title}"
                  genres={pending ?? song.genres}
                  busy={pending !== undefined}
                  onchange={(next) => retag(song, next)}
                />
              </div>
              <button class="quiet-button remove" type="button" onclick={() => ask(song)}>
                Remove<span class="sr-only"> {song.title}</span>
              </button>
              {#if errors[song.trackId]}<p class="error row-error" role="alert">{errors[song.trackId]}</p>{/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {:else if finder.error}
    <p class="error" role="alert">{finder.error}</p>
    <div>
      <button class="button outline" type="button" onclick={() => finder.again()} disabled={finder.searching}
        >{finder.searching ? 'Asking…' : 'Try again'}</button
      >
    </div>
  {:else}
    <p class="working" role="status">Loading the pool's counts…</p>
  {/if}
</section>

<Confirm
  bind:this={question}
  title={target ? `Remove ${songName(target)}?` : 'Remove this song?'}
  confirm="Remove the song"
  working="Removing…"
  action={remove}
  ondone={() => heading?.focus()}
>
  {#if target}
    {#each removalText(target, sectionsPlaying(desk.state, target.trackId)) as line}<p>{line}</p>{/each}
  {/if}
</Confirm>

<style>
  .search {
    display: grid;
    gap: var(--space-2);
    max-width: 36rem;
  }

  label {
    font-size: var(--text-small);
    font-weight: var(--weight-medium);
  }

  .search input {
    width: 100%;
    height: var(--control);
    padding: 0 var(--space-4);
    border: var(--line) solid var(--control-border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--paper);
    font-size: 16px;
    appearance: none;
  }

  .search input::placeholder {
    color: var(--muted);
    opacity: 1;
  }

  .search input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
    box-shadow: none;
  }

  /* The totals are a line to read, not controls. */
  .totals {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1) var(--space-4);
    font-size: 0.875rem;
  }

  .failures {
    color: var(--muted);
  }

  /* The one way to rows without typing, kept minor: a text button in the line. */
  .failures button {
    /* A 44px target that does not make the line taller. */
    min-height: 0;
    margin: -0.75rem 0;
    padding: 0.75rem 0.375rem;
    color: var(--paper);
    font-size: inherit;
    text-decoration: underline;
    text-decoration-color: var(--control-border);
    text-decoration-thickness: var(--line);
    text-underline-offset: 0.25em;
  }

  .note {
    font-size: var(--text-small);
  }

  .failed-search {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2) var(--space-4);
  }

  .results {
    display: grid;
    gap: var(--space-3);
  }

  .found {
    min-height: 1.5em;
    font-size: 0.875rem;
  }

  .found:empty {
    display: none;
  }

  .more {
    color: var(--muted);
  }

  .songs {
    display: grid;
  }

  [aria-busy='true'] .songs {
    opacity: 0.6;
  }

  .songs li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 0 var(--space-4);
    padding: var(--space-2) 0 var(--space-2) var(--space-3);
    border-top: var(--line) solid var(--border);
    /* The rule at the left is there for every row, so a marked one does not shift. */
    border-left: 2px solid transparent;
  }

  .songs li:last-child {
    border-bottom: var(--line) solid var(--border);
  }

  /* A failed preview check: an ivory rule and the "!" line. */
  .songs li.failed {
    border-left-color: var(--paper);
  }

  .song .error {
    margin-top: var(--space-1);
    font-size: var(--text-small);
    font-weight: var(--weight-regular);
  }

  .genres {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .genres .working {
    font-size: var(--text-small);
  }

  .remove {
    padding: 0 var(--space-3);
    text-decoration: underline;
    text-decoration-color: var(--control-border);
    text-decoration-thickness: var(--line);
    text-underline-offset: 0.25em;
  }

  .row-error {
    grid-column: 1 / -1;
    padding-bottom: var(--space-2);
  }

  @media (max-width: 44rem) {
    .songs li {
      grid-template-columns: minmax(0, 1fr) auto;
      padding-top: var(--space-3);
    }

    .songs .song {
      grid-column: 1 / -1;
    }

    .genres {
      flex-direction: row-reverse;
      justify-content: flex-end;
    }
  }
</style>
