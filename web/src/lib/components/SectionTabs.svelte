<script lang="ts">
  // The four sections as WAI-ARIA tabs with manual activation: the arrows,
  // Home and End move the focus, Enter or Space opens the tab. Opening a tab
  // stops the clip and may wait for the server, so it never follows the focus.
  //
  // Each tab is a real link to the section's own address, so it can be opened
  // in a new browser tab or copied; a plain click is handed to `onselect`.
  // Every state has its own mark and words, so none is told apart by colour.
  import {
    sectionLabel,
    sectionPath,
    tabForKey,
    tabId,
    tabStateWords,
  } from '../sections'
  import type { Section, TabState } from '../sections'

  interface Props {
    tabs: { section: Section; state: TabState }[]
    selected: Section
    /** The `id` of the tab panel these tabs control. */
    panel: string
    onselect: (section: Section) => void
  }

  let { tabs, selected, panel, onselect }: Props = $props()

  let list = $state<HTMLElement>()

  function onclick(event: MouseEvent, section: Section) {
    // A modified click is the browser's: a new tab, a new window, a download.
    if (
      event.button !== 0 ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey
    )
      return
    event.preventDefault()
    onselect(section)
  }

  function onkeydown(event: KeyboardEvent, index: number) {
    if (event.key === ' ') {
      // A link opens on Enter by itself; a tab also opens on Space.
      event.preventDefault()
      onselect(tabs[index].section)
      return
    }
    if (event.metaKey || event.ctrlKey || event.altKey) return
    const next = tabForKey(event.key, index, tabs.length)
    if (next === null) return
    event.preventDefault()
    list?.querySelectorAll<HTMLElement>('[role="tab"]')[next]?.focus()
  }
</script>

<div
  class="tabs"
  role="tablist"
  aria-label="Today's games"
  bind:this={list}
  data-sky-calm
>
  {#each tabs as tab, i (tab.section)}
    {@const words = tabStateWords(tab.state)}
    <a
      role="tab"
      id={tabId(tab.section)}
      href={sectionPath(tab.section)}
      class={tab.state}
      aria-selected={tab.section === selected}
      aria-controls={panel}
      tabindex={tab.section === selected ? 0 : -1}
      title={words ? `${sectionLabel(tab.section)}: ${words}` : undefined}
      draggable="false"
      onclick={(event) => onclick(event, tab.section)}
      onkeydown={(event) => onkeydown(event, i)}
    >
      <span class="mark" aria-hidden="true">
        <svg viewBox="0 0 16 16" width="14" height="14">
          {#if tab.state === 'won'}
            <path d="m3.5 8.5 3 3 6-7" />
          {:else if tab.state === 'lost'}
            <path d="m4 4 8 8m0-8-8 8" />
          {:else if tab.state === 'playing'}
            <circle cx="8" cy="8" r="5.5" />
            <circle class="dot" cx="8" cy="8" r="2.5" />
          {:else if tab.state === 'unplayed'}
            <circle cx="8" cy="8" r="5.5" />
          {:else if tab.state === 'none'}
            <path d="M3.5 8h9" />
          {/if}
        </svg>
      </span>
      <!-- No space before the comma: the two spans are one name to a screen reader. -->
      <span class="name">{sectionLabel(tab.section)}</span
      >{#if words}<span class="sr-only">, {words}</span>{/if}
    </a>
  {/each}
</div>

<style>
  .tabs {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    border-bottom: var(--line) solid var(--border);
  }

  a {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.375rem;
    min-width: 0;
    min-height: var(--target);
    padding: 0 0.25rem;
    color: var(--muted);
    font-size: var(--text-small);
    font-weight: var(--weight-medium);
    text-decoration: none;
    white-space: nowrap;
    touch-action: manipulation;
    -webkit-tap-highlight-color: transparent;
    user-select: none;
    -webkit-user-select: none;
    transition: color 200ms var(--ease-out);
  }

  a[aria-selected='true'] {
    color: var(--paper);
  }

  /* The selected tab's line lies over the row's own rule. */
  a::after {
    content: '';
    position: absolute;
    right: 0.25rem;
    bottom: calc(var(--line) * -1);
    left: 0.25rem;
    height: 2px;
    border-radius: 1px;
    background: var(--accent);
    transform: scaleX(0);
    transition: transform 260ms var(--ease-out);
  }

  a[aria-selected='true']::after {
    transform: scaleX(1);
  }

  /* The ring sits inside the tab, so the neighbours and the rule do not cut it. */
  a:focus-visible {
    outline-offset: -3px;
    box-shadow: none;
    border-radius: 6px;
  }

  @media (hover: hover) {
    a:hover {
      color: var(--paper);
    }
  }

  .mark {
    flex: none;
    display: grid;
    place-items: center;
    width: 1rem;
    height: 1rem;
    border-radius: 50%;
  }

  svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .dot {
    fill: var(--accent);
    stroke: none;
  }

  /* A win is the one filled mark: champagne, as the winning try is in the list. */
  .won .mark {
    background: var(--accent);
    color: var(--ink);
  }

  .won svg {
    stroke-width: 2;
  }

  .lost .mark {
    color: var(--paper);
  }

  @media (max-width: 22.5rem) {
    a {
      gap: 0.25rem;
      padding: 0;
      font-size: 0.75rem;
    }

    a::after {
      right: 0;
      left: 0;
    }

    .mark {
      width: 0.875rem;
      height: 0.875rem;
    }

    svg {
      width: 12px;
      height: 12px;
    }
  }

  /* With room to spare the tabs keep to their own width, at the page's left edge. */
  @media (min-width: 40rem) {
    .tabs {
      display: flex;
      gap: 0.5rem;
    }

    a {
      padding: 0 1rem;
      font-size: 0.875rem;
    }

    a:first-child {
      margin-left: -1rem;
    }

    a::after {
      right: 1rem;
      left: 1rem;
    }
  }
</style>
