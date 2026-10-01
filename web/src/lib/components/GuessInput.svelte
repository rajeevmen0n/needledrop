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

  const DEBOUNCE_MS = 300
  const MIN_CHARS = 2
  /** The list opens under the field when it has at least this much room there, in pixels. */
  const ROOM_BELOW = 232
  /** The list is never taller than this, nor shorter than about two rows. */
  const MAX_LIST = 400
  const MIN_LIST = 112
  /** Kept free between the list and the edge of the screen (or the keyboard). */
  const EDGE = 12

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
  /** Which side of the field the list opens on, and how tall it may be. */
  let above = $state(false)
  let room = $state(MAX_LIST)

  let timer: ReturnType<typeof setTimeout> | undefined
  let request: AbortController | undefined
  let composing = false

  const expanded = $derived(open && results.length > 0)
  const hint = $derived.by(() => {
    if (phase === 'searching') return 'Searching…'
    if (phase === 'failed')
      return "Couldn't search. Check your connection and keep typing."
    if (phase === 'done' && results.length === 0) return 'No songs found'
    return ''
  })
  // Sighted players see the list grow; screen readers get the count instead.
  const spoken = $derived.by(() => {
    if (phase === 'done' && results.length > 0) {
      return results.length === 1
        ? '1 song found'
        : `${results.length} songs found`
    }
    return hint
  })

  const label = (track: Track) => `${track.title}, ${track.artist}`

  /**
   * Puts the list where there is room for it. On a phone the on-screen keyboard
   * covers the bottom of the page; the visual viewport is what is left, so the
   * list opens above the field when the space under it is gone.
   */
  function place() {
    if (!input) return
    const field = input.getBoundingClientRect()
    const view = window.visualViewport
    const top = view ? view.offsetTop : 0
    const bottom = top + (view ? view.height : window.innerHeight)
    const over = field.top - top
    const under = bottom - field.bottom
    above = under < ROOM_BELOW && over > under
    room = Math.max(MIN_LIST, Math.min(MAX_LIST, (above ? over : under) - EDGE))
  }

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
    if (composing) {
      phase = 'idle'
      open = false
      return
    }
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
    input?.focus()
  }

  function highlight(index: number) {
    active = index
    document
      .getElementById(optionId(index))
      ?.scrollIntoView({ block: 'nearest' })
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
    if (!picked || busy || composing) return
    const accepted = await onguess(picked)
    // On a failed request the pick stays, so one more press retries it.
    if (accepted) clear()
    // The Guess button may have taken the focus; the next guess starts in the field.
    input?.focus()
  }

  $effect(() => cancelSearch)

  // While the list is open, follow the keyboard sliding in and out and the page scrolling.
  $effect(() => {
    if (!open) return
    place()
    const view = window.visualViewport
    view?.addEventListener('resize', place)
    view?.addEventListener('scroll', place)
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, { passive: true })
    return () => {
      view?.removeEventListener('resize', place)
      view?.removeEventListener('scroll', place)
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place)
    }
  })
</script>

<form class="guess" {onsubmit} novalidate aria-busy={busy}>
  <label for="{uid}-input">What's the song?</label>
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
        aria-activedescendant={expanded && active >= 0
          ? optionId(active)
          : undefined}
        aria-describedby={helpId}
        placeholder="Search for a song"
        autocomplete="off"
        autocapitalize="off"
        autocorrect="off"
        spellcheck="false"
        enterkeyhint="search"
        {oninput}
        {onkeydown}
        disabled={busy}
        oncompositionstart={() => {
          composing = true
          cancelSearch()
        }}
        oncompositionend={() => {
          composing = false
          oninput()
        }}
        onfocus={() => (open = phase !== 'idle')}
        onblur={() => (open = false)}
      />
      {#if text}
        <button
          class="clear"
          type="button"
          aria-label="Clear search"
          disabled={busy}
          onclick={clear}>×</button
        >
      {/if}
      <!-- preventDefault on mousedown keeps the focus in the field, so a tap on a row does not close the list first. -->
      <div
        class="popup"
        class:above
        style:max-height="{room}px"
        hidden={!open || (results.length === 0 && hint === '')}
      >
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
                <img
                  src={track.cover}
                  alt=""
                  width="40"
                  height="40"
                  loading="lazy"
                />
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
    <button class="button solid" type="submit" disabled={!picked || busy}
      >Guess</button
    >
  </div>
  <p class="help" id={helpId}>
    {picked
      ? 'Press Guess to send it.'
      : 'Type a title or artist, then pick a song from the list.'}
  </p>
  <p class="sr-only" role="status">{spoken}</p>
</form>

<style>
  .guess {
    display: grid;
    gap: 0.5rem;
  }
  label {
    font-size: 0.8125rem;
    font-weight: 600;
  }
  .row {
    display: flex;
    gap: 0.5rem;
  }
  .field {
    position: relative;
    flex: 1;
    min-width: 0;
  }
  input {
    display: block;
    width: 100%;
    height: var(--control);
    padding: 0 2.75rem 0 1rem;
    border: 1px solid var(--control-border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--paper);
    font-size: 16px;
    appearance: none;
  }
  input::placeholder {
    color: var(--muted);
    opacity: 1;
  }
  input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
    box-shadow: none;
  }
  .clear {
    position: absolute;
    right: 2px;
    top: 6px;
    width: 44px;
    height: 44px;
    font-size: 1.5rem;
    color: var(--muted);
    border-radius: 4px;
  }
  .clear:hover {
    color: var(--paper);
    background: #252525;
  }
  .button {
    flex: none;
    padding: 0 1.25rem;
    min-width: 5.2rem;
  }
  .popup {
    position: absolute;
    z-index: 5;
    inset: calc(100% + 0.5rem) 0 auto 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    background: var(--surface);
    color: var(--paper);
    box-shadow: var(--lift);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .popup.above {
    inset: auto 0 calc(100% + 0.5rem) 0;
  }
  li {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    min-height: var(--control);
    padding: 0.65rem 0.75rem 0.65rem 0.5rem;
    border-left: 3px solid transparent;
    cursor: pointer;
    touch-action: manipulation;
    -webkit-tap-highlight-color: transparent;
    user-select: none;
  }
  li.active {
    border-left-color: var(--accent);
    background: #2a2721;
  }
  img,
  .no-cover {
    flex: none;
    width: 2.5rem;
    height: 2.5rem;
    background: var(--border);
    border-radius: 2px;
  }
  .names {
    display: grid;
    min-width: 0;
    line-height: 1.3;
  }
  .title,
  .artist {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .title {
    font-weight: 600;
    font-size: 0.875rem;
  }
  .artist,
  .hint,
  .help {
    font-size: var(--text-small);
    color: var(--muted);
  }
  .hint {
    padding: 1rem;
  }
  .help {
    min-height: 1.5em;
  }
  @media (max-width: 360px) {
    .help {
      font-size: 0.6875rem;
    }
    .button {
      padding: 0 0.85rem;
    }
  }
</style>
