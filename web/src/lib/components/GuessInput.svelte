<script lang="ts">
  // The guess: a song search (SongCombobox) and the button that sends what
  // was picked. A guess is always a song picked from the list, never free
  // text, because the server takes a track id.
  import { searchTracks } from '../api'
  import type { Track } from '../api'
  import SongCombobox from './SongCombobox.svelte'

  interface Props {
    /** Sends the guess. Resolves to whether the server accepted the move. */
    onguess: (track: Track) => Promise<boolean>
    /** True while a move is on its way. */
    busy?: boolean
  }

  let { onguess, busy = false }: Props = $props()

  const uid = $props.id()
  const inputId = `${uid}-input`
  const helpId = `${uid}-help`

  let field = $state<SongCombobox<Track>>()
  let picked = $state<Track | null>(null)

  async function onsubmit(event: SubmitEvent) {
    event.preventDefault()
    if (!picked || busy || field?.isComposing()) return
    const accepted = await onguess(picked)
    // On a failed request the pick stays, so one more press retries it.
    if (accepted) field?.clear()
    // The Guess button may have taken the focus; the next guess starts in the field.
    field?.focus()
  }
</script>

<form class="guess" {onsubmit} novalidate aria-busy={busy}>
  <label for={inputId}>What's the song?</label>
  <div class="row">
    <SongCombobox
      bind:this={field}
      bind:picked
      id={inputId}
      search={searchTracks}
      describedby={helpId}
      disabled={busy}
    />
    <button class="button solid" type="submit" disabled={!picked || busy}
      >Guess</button
    >
  </div>
  <p class="help" id={helpId}>
    {picked
      ? 'Press Guess to send it.'
      : 'Type a title or artist, then pick a song from the list.'}
  </p>
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
  .button {
    flex: none;
    padding: 0 1.25rem;
    min-width: 5.2rem;
  }
  .help {
    min-height: 1.5em;
    font-size: var(--text-small);
    color: var(--muted);
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
