<script lang="ts">
  // "Clear my data": a quiet button and the question it asks first. It knows
  // nothing about the game; `onclear` does the deleting, and the page says
  // afterwards that it was done.
  import Confirm from './Confirm.svelte'

  interface Props {
    /** Deletes the data. Resolves to `null` when it is done, or to why it failed. */
    onclear: () => Promise<string | null>
    /** True while it should not be asked for: a move is on its way. */
    disabled?: boolean
  }

  let { onclear, disabled = false }: Props = $props()

  let question = $state<Confirm>()
</script>

<button
  class="quiet-button"
  type="button"
  {disabled}
  onclick={() => question?.show()}>Clear my data</button
>
<Confirm
  bind:this={question}
  title="Clear your data?"
  confirm="Clear my data"
  working="Clearing…"
  action={onclear}
>
  <p>
    This deletes every game, streak and stat kept for this browser, in all four
    sections, today's tries included.
  </p>
  <p>It cannot be undone. Today's songs stay the same.</p>
</Confirm>
