<script lang="ts">
  // The server's day: where it is, Simulate next day, and Reset to day 1
  // behind a question. Both move the day for every player at once.
  import Confirm from '../components/Confirm.svelte'
  import { desk } from './desk.svelte'
  import { dayText, offsetText } from './model'
  import type { AdminState } from './model'

  interface Props {
    /** The server's day as it last answered. */
    today: AdminState
  }

  let { today }: Props = $props()

  const uid = $props.id()

  let question = $state<Confirm>()
  /** Why the last thing asked for here failed. */
  let error = $state<string | null>(null)

  const busy = $derived(desk.dayWork !== null)

  async function refresh() {
    error = null
    await desk.loadState()
    error = desk.stateError
  }

  async function next() {
    error = null
    error = await desk.nextDay()
  }
</script>

<section class="panel" aria-labelledby="{uid}-heading">
  <div class="panel-head">
    <h2 id="{uid}-heading">Clock</h2>
    <button class="quiet-button" type="button" onclick={refresh} disabled={busy}
      >{desk.dayWork === 'load' ? 'Refreshing…' : 'Refresh'}</button
    >
  </div>
  <p class="sr-only">{dayText(today)}</p>
  <dl class="figures wide">
    <div>
      <dt class="label">Day number</dt>
      <dd class="value">{today.number}</dd>
    </div>
    <div>
      <dt class="label">The server's day</dt>
      <dd class="value">{today.day}</dd>
    </div>
    <div>
      <dt class="label">Real date (UTC)</dt>
      <dd class="value">{today.realDay}</dd>
    </div>
    <div>
      <dt class="label">Offset</dt>
      <dd class="value">{offsetText(today.offset)}</dd>
    </div>
  </dl>
  <div class="actions">
    <button class="button outline" type="button" onclick={next} disabled={busy} aria-busy={desk.dayWork === 'next-day'}
      >{desk.dayWork === 'next-day' ? 'Moving to the next day…' : 'Simulate next day'}</button
    >
    <button class="button outline" type="button" onclick={() => question?.show()} disabled={busy}
      >Reset to day 1</button
    >
  </div>
  <p class="hint">
    Both move the day for every player at once. The next day draws four new
    songs; nothing is deleted and streaks carry on.
  </p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <p class="news" role="status">{desk.clockNews}</p>
</section>

<Confirm
  bind:this={question}
  title="Reset to day 1?"
  confirm="Reset to day 1"
  working="Resetting…"
  action={() => desk.reset()}
>
  <p>
    The server's day goes back to the launch date. This deletes every player's
    games, on every day and in every section, and every pick. Streaks and stats
    go with the games.
  </p>
  <p>It cannot be undone. The song pool is kept.</p>
</Confirm>

<style>
  /* The figure is drawn first and its name under it; the markup keeps name before value. */
  dt {
    order: 2;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
</style>
