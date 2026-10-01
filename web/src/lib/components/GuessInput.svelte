<script lang="ts">
  // Song search as a WAI-ARIA combobox (list autocomplete). Focus never leaves
  // the text field: the highlighted row is named by aria-activedescendant.
  // A guess is always a song picked from the list, never free text, because
  // the server takes a track id.
  import { searchTracks } from '../api'
  import type { Track } from '../api'

  interface Props {
    /** Sends the guess. Resolves to whether the server accepted the move. */
    onguess: (track: Track) => Promise<boolean>
    /** True while a move is on its way. */
    busy?: boolean
  }

  let { onguess, busy = false }: Props = $props()

  const DEBOUNCE_MS = 200
  const MIN_CHARS = 2

  const uid = $props.id()
  const listId = `${uid}-list`
  const helpId = `${uid}-help`
  const optionId = (index: number) => `${uid}-option-${index}`

  type Phase = 'idle' | 'searching' | 'done' | 'failed'

  let input = $state<HTMLInputElement>()
  let text = $state('')
  let picked = $state<Track | null>(null)
  let results = $state<Track[]>([])
  let phase = $state<Phase>('idle')
  let open = $state(false)
  /** Index of the highlighted row, -1 for none. */
  let active = $state(-1)

  let timer: ReturnType<typeof setTimeout> | undefined
  let request: AbortController | undefined

  const expanded = $derived(open && results.length > 0)
  const hint = $derived.by(() => {
    if (phase === 'searching') return 'Searching…'
    if (phase === 'failed') return "Couldn't search. Check your connection and keep typing."
    if (phase === 'done' && results.length === 0) return 'No songs found'
    return ''
  })
  // Sighted players see the list grow; screen readers get the count instead.
  const spoken = $derived.by(() => {
    if (phase === 'done' && results.length > 0) {
      return results.length === 1 ? '1 song found' : `${results.length} songs found`
    }
    return hint
  })

  const label = (track: Track) => `${track.title} — ${track.artist}`

  function cancelSearch() {
    clearTimeout(timer)
    request?.abort()
    request = undefined
  }

  function oninput() {
    // The text no longer names the picked song.
    picked = null
    cancelSearch()
    // Rows for the previous text go at once, so an old list is never shown under new text.
    results = []
    active = -1
    const query = text.trim()
    if (query.length < MIN_CHARS) {
      phase = 'idle'
      open = false
      return
    }
    phase = 'searching'
    open = true
    timer = setTimeout(() => void search(query), DEBOUNCE_MS)
  }

  async function search(query: string) {
    const controller = new AbortController()
    request = controller
    try {
      const found = await searchTracks(query, controller.signal)
      // A newer keystroke replaced this request while it was in flight.
      if (request !== controller) return
      results = found
      active = found.length > 0 ? 0 : -1
      phase = 'done'
    } catch {
      if (request !== controller) return
      phase = 'failed'
    }
  }

  function pick(track: Track) {
    cancelSearch()
    picked = track
    text = label(track)
    results = []
    active = -1
    phase = 'idle'
    open = false
    input?.focus()
  }

  function clear() {
    cancelSearch()
    picked = null
    text = ''
    results = []
    active = -1
    phase = 'idle'
    open = false
  }

  function highlight(index: number) {
    active = index
    document.getElementById(optionId(index))?.scrollIntoView({ block: 'nearest' })
  }

  function onkeydown(event: KeyboardEvent) {
    // Enter and the arrows belong to the input method while it is composing.
    if (event.isComposing) return
    switch (event.key) {
      case 'ArrowDown':
      case 'ArrowUp': {
        if (results.length === 0) return
        event.preventDefault()
        const last = results.length - 1
        if (!open) {
          open = true
          highlight(active >= 0 ? active : event.key === 'ArrowDown' ? 0 : last)
        } else if (event.key === 'ArrowDown') {
          highlight(active >= last ? 0 : active + 1)
        } else {
          highlight(active <= 0 ? last : active - 1)
        }
        break
      }
      case 'Enter': {
        const track = expanded ? results[active] : undefined
        if (track) {
          // Picks the row. A second Enter then submits the guess.
          event.preventDefault()
          pick(track)
        }
        break
      }
      case 'Escape':
        if (open) {
          event.preventDefault()
          open = false
        } else if (text !== '') {
          event.preventDefault()
          clear()
        }
        break
    }
  }

  async function onsubmit(event: SubmitEvent) {
    event.preventDefault()
    if (!picked || busy) return
    const accepted = await onguess(picked)
    // On a failed request the pick stays, so one more press retries it.
    if (accepted) clear()
    // The Guess button may have taken the focus; the next guess starts in the field.
    input?.focus()
  }

  $effect(() => cancelSearch)
</script>

<form class="guess" {onsubmit}>
  <label for="{uid}-input">Your guess</label>
  <div class="row">
    <div class="field">
      <input
        bind:this={input}
        bind:value={text}
        id="{uid}-input"
        type="text"
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={expanded}
        aria-controls={listId}
        aria-activedescendant={expanded && active >= 0 ? optionId(active) : undefined}
        aria-describedby={helpId}
        placeholder="Search for a song"
        autocomplete="off"
        autocapitalize="off"
        autocorrect="off"
        spellcheck="false"
        enterkeyhint="search"
        {oninput}
        {onkeydown}
        onfocus={() => (open = phase !== 'idle')}
        onblur={() => (open = false)}
      />
      <!-- preventDefault on mousedown keeps the focus in the field, so a tap on a row does not close the list first. -->
      <div class="popup" hidden={!open || (results.length === 0 && hint === '')}>
        <ul
          id={listId}
          role="listbox"
          aria-label="Songs"
          hidden={results.length === 0}
          onmousedown={(event) => event.preventDefault()}
        >
          {#each results as track, i}
            <!-- The keys are handled on the text field: rows never take the focus in this pattern. -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <li
              id={optionId(i)}
              role="option"
              aria-selected={i === active}
              class:active={i === active}
              onclick={() => pick(track)}
              onpointermove={() => (active = i)}
            >
              {#if track.cover}
                <img src={track.cover} alt="" width="40" height="40" loading="lazy" />
              {:else}
                <span class="no-cover"></span>
              {/if}
              <span class="names">
                <span class="title">{track.title}</span>
                <span class="artist">{track.artist}</span>
              </span>
            </li>
          {/each}
        </ul>
        {#if hint}
          <p class="hint" aria-hidden="true">{hint}</p>
        {/if}
      </div>
    </div>
    <button class="primary" type="submit" disabled={!picked || busy}>Guess</button>
  </div>
  <p class="help" id={helpId}>
    {picked ? 'Press Guess to send it.' : 'Type a title or an artist, then pick a song from the list.'}
  </p>
  <p class="sr-only" role="status">{spoken}</p>
</form>

<style>
  .guess {
    display: grid;
    gap: var(--space-2);
  }

  label {
    font-weight: 600;
  }

  .row {
    display: flex;
    gap: var(--space-2);
  }

  .field {
    position: relative;
    flex: 1;
    min-width: 0;
  }

  input {
    width: 100%;
    height: var(--control-height);
    padding: 0 var(--space-3);
    border: var(--border);
    border-radius: var(--radius);
    background: var(--color-surface);
    /* 16 px or more, or iOS zooms the page when the field takes focus. */
    font-size: max(1rem, 16px);
  }

  .popup {
    position: absolute;
    z-index: 1;
    inset: calc(100% + var(--space-1)) 0 auto 0;
    max-height: min(24rem, 55vh);
    overflow-y: auto;
    border: var(--border);
    border-radius: var(--radius);
    background: var(--color-surface);
    box-shadow: 0 6px 18px rgb(0 0 0 / 0.18);
  }

  li {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-height: var(--control-height);
    padding: var(--space-2) var(--space-3);
    /* A bar on the left marks the highlighted row, so it does not depend on colour alone. */
    border-left: 4px solid transparent;
    cursor: pointer;
  }

  li.active {
    border-left-color: var(--color-accent);
    background: var(--color-accent-soft);
  }

  img,
  .no-cover {
    flex: none;
    width: 2.5rem;
    height: 2.5rem;
    border-radius: calc(var(--radius) / 2);
    background: var(--color-bg);
  }

  .names {
    display: grid;
    min-width: 0;
  }

  .title,
  .artist {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title {
    font-weight: 600;
  }

  .artist,
  .hint,
  .help {
    color: var(--color-muted);
    font-size: var(--text-small);
  }

  .hint {
    padding: var(--space-3);
  }
</style>
