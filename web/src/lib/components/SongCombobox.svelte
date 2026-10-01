<script lang="ts" generics="T extends Track">
  // Song search as a WAI-ARIA combobox (list autocomplete): the text field,
  // its clear button and the list of results. Focus never leaves the text
  // field: the highlighted row is named by aria-activedescendant.
  //
  // It knows nothing about what a song is picked for. The game's guess
  // (GuessInput) and the admin's "Add a song" both use it, each with its own
  // search; the label, the help text and any button belong to them.
  import type { Track } from '../api'

  interface Props {
    /** The `id` of the text field, so a label outside can name it. */
    id: string
    /** Finds the songs for a text. A rejection shows as a failed search. */
    search: (query: string, signal: AbortSignal) => Promise<T[]>
    /** The song the text names: set by picking a row, gone as soon as the text is edited. */
    picked?: T | null
    placeholder?: string
    /** The `id` of the text that describes the field. */
    describedby?: string
    /** True while the field should not be used: something is on its way. */
    disabled?: boolean
    /**
     * Keep what was typed, and its results, when a row is picked: the list
     * closes and opens again on a click or an arrow key, so another row of the
     * same search can be picked next. `picked` is not set; `onpick` gets the row.
     */
    keep?: boolean
    /** Called with each row that is picked. */
    onpick?: (track: T) => void
    /** The second line of a row. The artist, unless given. */
    detail?: (track: T) => string
    /** A short mark at the end of a row, or nothing. */
    note?: (track: T) => string | null
    /** Whether a row is shown as lesser. It can still be picked. */
    dim?: (track: T) => boolean
    /** What to say when a search fails. */
    failure?: (err: unknown) => string
  }

  let {
    id,
    search,
    picked = $bindable(null),
    placeholder = 'Search for a song',
    describedby,
    disabled = false,
    keep = false,
    onpick,
    detail = (track) => track.artist,
    note,
    dim,
    failure = () => "Couldn't search. Check your connection and keep typing.",
  }: Props = $props()

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
  const optionId = (index: number) => `${uid}-option-${index}`

  type Phase = 'idle' | 'searching' | 'done' | 'failed'

  let input = $state<HTMLInputElement>()
  let text = $state('')
  let results = $state<T[]>([])
  let phase = $state<Phase>('idle')
  let failed = $state('')
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
    if (phase === 'failed') return failed
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

  const label = (track: T) => `${track.title}, ${track.artist}`

  /** Empties the field and puts the focus in it. */
  export function clear(): void {
    cancelSearch()
    picked = null
    text = ''
    results = []
    active = -1
    phase = 'idle'
    open = false
    input?.focus()
  }

  export function focus(): void {
    input?.focus()
  }

  /** True while an input method is composing: Enter belongs to it, not to a form. */
  export function isComposing(): boolean {
    return composing
  }

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
    timer = setTimeout(() => void find(query), DEBOUNCE_MS)
  }

  async function find(query: string) {
    const controller = new AbortController()
    request = controller
    try {
      const found = await search(query, controller.signal)
      // A newer keystroke replaced this request while it was in flight.
      if (request !== controller) return
      results = found
      active = found.length > 0 ? 0 : -1
      phase = 'done'
    } catch (err) {
      if (request !== controller) return
      failed = failure(err)
      phase = 'failed'
    }
  }

  function pick(track: T) {
    if (keep) {
      // The search stays as it is; only the list goes out of the way.
      open = false
      input?.focus()
      onpick?.(track)
      return
    }
    cancelSearch()
    picked = track
    text = label(track)
    results = []
    active = -1
    phase = 'idle'
    open = false
    input?.focus()
    onpick?.(track)
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
          // Picks the row. In a form, a second Enter then submits it.
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

<div class="field">
  <input
    bind:this={input}
    bind:value={text}
    {id}
    type="text"
    role="combobox"
    aria-autocomplete="list"
    aria-expanded={expanded}
    aria-controls={listId}
    aria-activedescendant={expanded && active >= 0
      ? optionId(active)
      : undefined}
    aria-describedby={describedby}
    {placeholder}
    autocomplete="off"
    autocapitalize="off"
    autocorrect="off"
    spellcheck="false"
    enterkeyhint="search"
    {oninput}
    {onkeydown}
    {disabled}
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
    onclick={() => {
      // A list closed by a pick that kept the search opens again where it was.
      if (keep && !open && phase !== 'idle') open = true
    }}
  />
  {#if text}
    <button
      class="clear"
      type="button"
      aria-label="Clear search"
      {disabled}
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
        {@const mark = note?.(track)}
        <!-- The keys are handled on the text field: rows never take the focus in this pattern. -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <li
          id={optionId(i)}
          role="option"
          aria-selected={i === active}
          class:active={i === active}
          class:dim={dim?.(track)}
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
            <span class="artist">{detail(track)}</span>
          </span>
          {#if mark}<span class="note">{mark}</span>{/if}
        </li>
      {/each}
    </ul>
    {#if hint}
      <p class="hint" aria-hidden="true">{hint}</p>
    {/if}
  </div>
  <p class="sr-only" role="status">{spoken}</p>
</div>

<style>
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
  .hint {
    font-size: var(--text-small);
    color: var(--muted);
  }
  .hint {
    padding: 1rem;
  }
  /* A row's mark says in words what the dimming only suggests. */
  .note {
    flex: none;
    margin-left: auto;
    padding: 0.125rem 0.4rem;
    border: 1px solid var(--control-border);
    border-radius: 4px;
    color: var(--paper);
    font-size: 0.75rem;
    line-height: 1.3;
    white-space: nowrap;
  }
  li.dim .title {
    color: var(--muted);
    font-weight: 400;
  }
  li.dim img {
    opacity: 0.45;
  }
</style>
