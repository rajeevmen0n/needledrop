<script lang="ts">
  // What random mode draws its next songs from: every song, or one genre.
  // It knows nothing about the game: it shows the pool it is given and says
  // which one was chosen.
  //
  // A radio group with manual activation, as the tab row has: the arrows,
  // Home and End move the focus, and Space, Enter or a click chooses. A choice
  // asks the server, so it never follows the focus.
  //
  // The chosen option is ivory with an ivory line on the row's rule: it is a
  // setting, not a way to play, so it carries no champagne.
  import { POOLS, POOL_LEGEND, poolName } from '../random'
  import type { Pool } from '../random'
  import { tabForKey } from '../sections'

  interface Props {
    /** The pool that stands. */
    pool: Pool
    /**
     * Called with the pool that was chosen, the one that stands included.
     * While the promise it returns is pending, that option shows as chosen;
     * afterwards the picker shows `pool` again, whatever became of the choice.
     */
    onchoose: (pool: Pool) => Promise<unknown> | void
    /** True while no choice can be taken (a move is on its way). The options keep the focus. */
    disabled?: boolean
    /** A few words beside the label: when the choice takes effect. */
    note?: string
  }

  let { pool, onchoose, disabled = false, note = '' }: Props = $props()

  const uid = $props.id()

  let group = $state<HTMLElement>()
  /** The choice on its way to the server. */
  let asked = $state<Pool | null>(null)

  const shown = $derived(asked ?? pool)

  async function choose(next: Pool) {
    if (disabled || asked !== null) return
    asked = next
    try {
      await onchoose(next)
    } finally {
      asked = null
    }
  }

  function onkeydown(event: KeyboardEvent, index: number) {
    if (event.metaKey || event.ctrlKey || event.altKey) return
    // Up and down move as left and right do, as in any radio group.
    const key =
      event.key === 'ArrowDown'
        ? 'ArrowRight'
        : event.key === 'ArrowUp'
          ? 'ArrowLeft'
          : event.key
    const next = tabForKey(key, index, POOLS.length)
    if (next === null) return
    event.preventDefault()
    group?.querySelectorAll<HTMLElement>('[role="radio"]')[next]?.focus()
  }
</script>

<div class="picker">
  <p class="head">
    <span id="{uid}-label">{POOL_LEGEND}</span>
    {#if note}<span class="note" id="{uid}-note">{note}</span>{/if}
  </p>
  <div
    class="options"
    role="radiogroup"
    aria-labelledby="{uid}-label"
    aria-describedby={note ? `${uid}-note` : undefined}
    aria-busy={asked !== null}
    bind:this={group}
  >
    {#each POOLS as option, i (option)}
      <!-- Not disabled while a request is on its way: a disabled button would drop the keyboard focus. -->
      <button
        type="button"
        role="radio"
        aria-checked={option === shown}
        aria-disabled={disabled || asked !== null}
        tabindex={option === shown ? 0 : -1}
        onclick={() => choose(option)}
        onkeydown={(event) => onkeydown(event, i)}>{poolName(option)}</button
      >
    {/each}
  </div>
</div>

<style>
  .picker {
    display: grid;
    gap: 0.25rem;
    min-width: 0;
    max-width: 25rem;
  }

  /* The label and its note share a line, as the heading of the tries does; the note drops below when it must. */
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 0 1rem;
    font-size: var(--text-small);
    font-weight: var(--weight-medium);
    line-height: 1.3;
  }

  .note {
    color: var(--muted);
    font-weight: var(--weight-regular);
  }

  /* Four equal steps on one rule: the longest word has a whole step at every width. */
  .options {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    border-bottom: var(--line) solid var(--border);
  }

  button {
    position: relative;
    min-width: 0;
    min-height: var(--target);
    padding: 0 0.125rem;
    color: var(--muted);
    font-size: var(--text-small);
    font-weight: var(--weight-medium);
    line-height: 1.15;
    white-space: nowrap;
    transition: color 200ms var(--ease-out);
  }

  button[aria-checked='true'] {
    color: var(--paper);
  }

  /* The chosen step's line lies over the rule. Ivory: champagne is for playing. */
  button::after {
    content: '';
    position: absolute;
    right: 0.25rem;
    bottom: calc(var(--line) * -1);
    left: 0.25rem;
    height: 2px;
    border-radius: 1px;
    background: var(--paper);
    transform: scaleX(0);
    transition: transform 260ms var(--ease-out);
  }

  button[aria-checked='true']::after {
    transform: scaleX(1);
  }

  /* The ring sits inside the step, so the neighbours and the rule do not cut it. */
  button:focus-visible {
    outline-offset: -3px;
    box-shadow: none;
    border-radius: 6px;
  }

  button[aria-disabled='true'] {
    cursor: progress;
  }

  @media (hover: hover) {
    button:not([aria-disabled='true']):hover {
      color: var(--paper);
    }
  }

  /* Forced colours repaint a background; a system colour keeps the line. */
  @media (forced-colors: active) {
    button::after {
      background: Highlight;
    }
  }
</style>
