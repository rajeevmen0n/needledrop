<script lang="ts">
  // A question before something that cannot be undone, as a modal <dialog>:
  // the browser keeps the focus inside it, closes it on Escape and gives the
  // focus back to whatever opened it. It knows nothing about what it confirms.
  //
  // The focus starts on Cancel, so Enter pressed out of habit changes nothing.
  // While the action is on its way the dialog stays open and cannot be
  // dismissed; if it fails, the reason is shown here and the page is as it was.
  import type { Snippet } from 'svelte'

  interface Props {
    /** The question. */
    title: string
    /** What the button that goes ahead says, and what it says while the action is on its way. */
    confirm: string
    working: string
    /** Does it. Resolves to `null` when it is done, or to why it failed, as a sentence. */
    action: () => Promise<string | null>
    /** Called after the action succeeded and the dialog closed. */
    ondone?: () => void
    /** What will happen, in a sentence or two. */
    children: Snippet
  }

  let { title, confirm, working, action, ondone, children }: Props = $props()

  const uid = $props.id()

  let dialog = $state<HTMLDialogElement>()
  let cancel = $state<HTMLButtonElement>()
  let busy = $state(false)
  let error = $state<string | null>(null)

  /** Opens the dialog. Call it from the control that asks for the action. */
  export function show(): void {
    if (!dialog || dialog.open) return
    error = null
    dialog.showModal()
    cancel?.focus()
  }

  function close(): void {
    if (!busy) dialog?.close()
  }

  async function go(): Promise<void> {
    if (busy) return
    busy = true
    error = null
    const failure = await action().catch(() => 'Something went wrong. Try again.')
    busy = false
    if (failure !== null) {
      error = failure
      return
    }
    dialog?.close()
    ondone?.()
  }
</script>

<!-- A click on the backdrop lands on the dialog itself; its content is all inside .sheet. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<dialog
  bind:this={dialog}
  aria-labelledby="{uid}-title"
  aria-describedby="{uid}-body"
  onclick={(event) => {
    if (event.target === dialog) close()
  }}
  oncancel={(event) => {
    // Escape while the action is on its way would leave it running unseen.
    if (busy) event.preventDefault()
  }}
>
  <div class="sheet">
    <h2 id="{uid}-title">{title}</h2>
    <div class="body" id="{uid}-body">{@render children()}</div>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      <button
        class="button outline"
        type="button"
        bind:this={cancel}
        onclick={close}
        disabled={busy}>Cancel</button
      >
      <button
        class="button outline go"
        type="button"
        onclick={go}
        disabled={busy}
        aria-busy={busy}>{busy ? working : confirm}</button
      >
    </div>
  </div>
</dialog>

<style>
  dialog {
    width: min(26rem, calc(100vw - var(--gutter) * 2));
    max-height: calc(100vh - 2rem);
    padding: 0;
    overflow-y: auto;
    border: var(--line) solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--paper);
    box-shadow: var(--lift);
  }

  dialog::backdrop {
    background: rgb(0 0 0 / 0.72);
  }

  .sheet {
    display: grid;
    gap: var(--space-4);
    padding: var(--space-5);
  }

  h2 {
    font-size: var(--text-large);
    font-weight: var(--weight-medium);
    line-height: 1.2;
  }

  .body {
    display: grid;
    gap: var(--space-2);
    color: var(--muted);
    font-size: 0.875rem;
  }

  .error {
    font-size: 0.875rem;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }

  .button {
    min-height: var(--target);
    padding: 0 var(--space-4);
    border-color: var(--control-border);
    font-size: 0.875rem;
  }

  /* The way ahead is ivory, not champagne: champagne is for playing, never for deleting. */
  .go:not(:disabled) {
    border-color: var(--paper);
  }
</style>
