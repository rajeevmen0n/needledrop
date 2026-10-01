<script lang="ts">
  // Add a song: search Deezer with the game's drop-down, choose a release,
  // tick its genres, add it. Every release is listed, playable or not, and
  // marked when it is in the pool already. What was added then shows under
  // "Find a song in the pool", where it can be changed.
  //
  // The check that a song can be played lives here, not on the server: its
  // add never looks at the preview. Choosing a release without one shows why
  // and adds nothing.
  import SongCombobox from '../components/SongCombobox.svelte'
  import { saveSong, searchReleases, sentence } from './api'
  import { finder } from './finder.svelte'
  import GenreBoxes from './GenreBoxes.svelte'
  import { deezerLink, hitNote, poolsText, refusal, savedText } from './model'
  import type { AdminHit, Genre } from './model'

  const uid = $props.id()

  let field = $state<SongCombobox<AdminHit>>()
  /** The release the genres are being ticked for. */
  let chosen = $state<AdminHit | null>(null)
  let genres = $state<Genre[]>([])
  /** The genres of the last song added: the next new one starts from them, which suits adding a batch of one genre. */
  let last = $state<Genre[]>([])
  /** Why the release that was picked is not offered for adding. */
  let refused = $state<string | null>(null)
  let adding = $state(false)
  let error = $state<string | null>(null)
  /** What the last add did, and for which song. */
  let result = $state('')
  let saved = $state<number | null>(null)

  /** The genres the chosen release has in the pool, when it is there already: the search says so for each result. */
  // The news of an add is put away once that song has been removed again below.
  const news = $derived(result !== '' && finder.removed !== saved ? result : '')
  // …and it points at the pool search only while that is still showing the song.
  const below = $derived(news !== '' && finder.text === String(saved))
  const existing = $derived(chosen?.inPool ? (chosen.genres ?? []) : null)

  function onpick(hit: AdminHit) {
    result = ''
    error = null
    refused = refusal(hit)
    if (refused) {
      chosen = null
      return
    }
    chosen = hit
    // A song in the pool starts from the tags it has: adding it replaces them.
    genres = hit.inPool ? (hit.genres ?? []) : last
  }

  async function add() {
    if (!chosen || adding) return
    const was = existing
    adding = true
    error = null
    let row
    try {
      row = await saveSong(chosen.id, genres)
    } catch (err) {
      error = sentence(err)
      return
    } finally {
      adding = false
    }
    result = savedText(row, was)
    saved = row.trackId
    if (was === null) last = row.genres
    // The pool search shows the song, so it is seen to have landed and can be corrected at once.
    finder.show(row.trackId)
    chosen = null
    field?.clear()
  }

  function cancel() {
    chosen = null
    error = null
    field?.focus()
  }
</script>

<section class="panel" aria-labelledby="{uid}-heading">
  <div class="panel-head">
    <h2 id="{uid}-heading">Add a song from Deezer</h2>
  </div>
  <div class="search">
    <label for="{uid}-input">Search Deezer</label>
    <SongCombobox
      bind:this={field}
      id="{uid}-input"
      search={searchReleases}
      placeholder="Title or artist"
      describedby="{uid}-help"
      keep
      {onpick}
      detail={(hit) => (hit.album ? `${hit.artist} · ${hit.album}` : hit.artist)}
      note={hitNote}
      dim={(hit) => !hit.playable}
      failure={sentence}
    />
    <p class="hint" id="{uid}-help">
      Every release is listed, with its album. One marked “No preview” cannot be
      played, so it cannot be added.
    </p>
  </div>
  {#if refused}<p class="error" role="alert">{refused}</p>{/if}
  {#if chosen}
    <div class="chosen">
      <div class="release">
        {#if chosen.cover}
          <img src={chosen.cover} alt="" width="56" height="56" />
        {:else}
          <span class="no-cover"></span>
        {/if}
        <div class="song">
          <span class="title">{chosen.title}</span>
          <span class="meta">{chosen.artist}{chosen.album ? ` · ${chosen.album}` : ''}</span>
          <span class="meta"
            ><a href={deezerLink(chosen.id)} target="_blank" rel="noreferrer"
              >track <span class="numeric">{chosen.id}</span> on Deezer</a
            ></span
          >
        </div>
      </div>
      {#if existing}
        <p class="already">
          This song is in the pool already, in {poolsText(existing)}. Saving it only replaces its genres.
        </p>
      {/if}
      <GenreBoxes legend="Genres" labelled {genres} busy={adding} onchange={(next) => (genres = next)} />
      <p class="hint">
        Every song is in General. Tick the genre sections it should also play in, or none. As ticked: {poolsText(
          genres,
        )}.{#if !existing && last.length > 0}
          The boxes start as they were for the last song added.{/if}
      </p>
      <div class="actions">
        <button class="button solid" type="button" onclick={add} disabled={adding} aria-busy={adding}>
          {#if adding}{existing ? 'Saving…' : 'Adding…'}{:else}{existing ? 'Replace its genres' : 'Add to the pool'}{/if}
        </button>
        <button class="button outline" type="button" onclick={cancel} disabled={adding}>Cancel</button>
      </div>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
    </div>
  {/if}
  <p class="news" role="status">{news}{below ? ' The pool search below shows it.' : ''}</p>
</section>

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

  .chosen {
    display: grid;
    gap: var(--space-3);
    max-width: 36rem;
    padding-left: var(--space-4);
    border-left: var(--line) solid var(--control-border);
  }

  .release {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-width: 0;
  }

  img,
  .no-cover {
    flex: none;
    width: 3.5rem;
    height: 3.5rem;
    border-radius: 2px;
    background: var(--border);
  }

  .already {
    font-size: 0.875rem;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
</style>
