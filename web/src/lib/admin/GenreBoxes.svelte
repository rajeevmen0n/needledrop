<script lang="ts">
  // The three genre tags of a song as tick boxes. It knows nothing about the
  // pool: it shows the tags it is given and says which set a click asks for.
  import { GENRES, poolLabel, toggleGenre } from './model'
  import type { Genre } from './model'

  interface Props {
    /** The tags to show as ticked. */
    genres: readonly Genre[]
    /** Called with the tags after a box was clicked. */
    onchange: (genres: Genre[]) => void
    /** Whose genres these are, for a screen reader: "Genres of Billie Jean". */
    legend: string
    /** Show the legend instead of keeping it for screen readers only. */
    labelled?: boolean
    /** True while a change is on its way: the boxes keep the focus but take no clicks. */
    busy?: boolean
  }

  let { genres, onchange, legend, labelled = false, busy = false }: Props = $props()
</script>

<fieldset aria-busy={busy} class:busy>
  <legend class:sr-only={!labelled}>{legend}</legend>
  <div class="boxes">
    {#each GENRES as genre}
      <label>
        <!-- Not disabled while busy: a disabled box would drop the keyboard focus. -->
        <input
          type="checkbox"
          checked={genres.includes(genre)}
          aria-disabled={busy}
          onclick={(event) => {
            if (busy) event.preventDefault()
          }}
          onchange={() => onchange(toggleGenre(genres, genre))}
        />
        <span>{poolLabel(genre)}</span>
      </label>
    {/each}
  </div>
</fieldset>

<style>
  fieldset {
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
  }

  legend {
    padding: 0;
    color: var(--muted);
    font-size: var(--text-small);
  }

  .boxes {
    display: flex;
    flex-wrap: wrap;
    gap: 0 var(--space-1);
  }

  label {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    min-height: var(--target);
    padding: 0 var(--space-2) 0 0;
    font-size: 0.875rem;
    cursor: pointer;
    user-select: none;
    -webkit-tap-highlight-color: transparent;
  }

  /* An ivory tick on graphite: champagne stays with playing. */
  input {
    flex: none;
    appearance: none;
    width: 1.375rem;
    height: 1.375rem;
    margin: 0;
    border: var(--line) solid var(--control-border);
    border-radius: 4px;
    background: var(--surface) center / 0.875rem no-repeat;
    cursor: pointer;
  }

  input:checked {
    border-color: var(--paper);
    background-color: var(--paper);
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 14 14'%3E%3Cpath d='M2.5 7.4 5.6 10.5 11.5 3.9' fill='none' stroke='%2315120d' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
  }

  input:not(:checked) + span {
    color: var(--muted);
  }

  .busy label {
    cursor: progress;
  }

  .busy input {
    opacity: 0.6;
  }

  @media (forced-colors: active) {
    input {
      appearance: auto;
    }
  }
</style>
