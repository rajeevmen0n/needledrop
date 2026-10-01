// The game as the page sees it: the server's state, the loaded clip, and the
// moves. The server decides everything; this only mirrors what it answers.

import { ApiError, audioUrl, getDaily, postGuess } from './api'
import type { Answer, Attempt, Daily, Move, Status, Track } from './api'
import { ClipPlayer } from './audio'
import type { ClipProgress } from './audio'
import { clipLabel } from './clip'

/** One row of the attempts list. */
export type Slot =
  | { kind: 'skip' }
  | { kind: 'wrong'; title: string; artist: string }
  | { kind: 'correct'; title: string; artist: string }
  | { kind: 'current' }
  | { kind: 'empty' }

function describe(err: unknown): string {
  return err instanceof ApiError ? err.message : 'Something went wrong. Try again.'
}

export class Game {
  /** The server's last answer. `null` until the first load succeeds. */
  daily = $state<Daily | null>(null)
  /** Why the first load failed, as a sentence for the player. */
  loadError = $state<string | null>(null)
  /** True while a skip or guess is on its way. */
  submitting = $state(false)
  /** Why the last skip or guess failed. */
  moveError = $state<string | null>(null)
  /** What the last move did, in words, for a status line. */
  notice = $state('')
  /** True while the clip is being fetched and decoded. */
  clipLoading = $state(false)
  /** Why the clip cannot be played. */
  clipError = $state<string | null>(null)
  playing = $state(false)
  /** Moves made since the page loaded; 0 means the state came from a reload. */
  moves = $state(0)

  status: Status = $derived(this.daily?.status ?? 'playing')
  finished = $derived(this.status !== 'playing')
  attempts: Attempt[] = $derived(this.daily?.attempts ?? [])
  ladder: number[] = $derived(this.daily?.ladder ?? [])
  answer: Answer | null = $derived(this.daily?.answer ?? null)
  /** Length of the whole preview in seconds: the last ladder step. */
  totalSeconds = $derived(this.ladder.at(-1) ?? 0)
  /** The clip length unlocked now. */
  clipSeconds = $derived(this.daily?.clipSeconds ?? 0)
  /** The turn being played, or the turn the game ended on. 1-based. */
  turn = $derived(Math.min(this.attempts.length + 1, Math.max(this.ladder.length, 1)))
  /** On the last turn a skip or a wrong guess ends the game. */
  lastTurn = $derived(!this.finished && this.attempts.length >= this.ladder.length - 1)
  /** What a skip or wrong guess would unlock; `null` on the last turn and once finished. */
  nextClipSeconds = $derived(
    this.finished || this.lastTurn ? null : (this.ladder[this.attempts.length + 1] ?? null),
  )
  /** One entry per ladder step, for the attempts list. */
  slots: Slot[] = $derived(
    this.ladder.map((_, i): Slot => {
      const attempt = this.attempts[i]
      if (attempt) return attempt
      if (i > this.attempts.length || this.status === 'lost') return { kind: 'empty' }
      // A win does not append an attempt: the slot after the last one is the winning turn.
      if (this.status === 'won' && this.answer) {
        return { kind: 'correct', title: this.answer.title, artist: this.answer.artist }
      }
      return { kind: 'current' }
    }),
  )

  /** The audio player, for a visualiser to read `player.analyser` from. */
  readonly player = new ClipPlayer()

  // Settles to whether the newest clip loaded; play() waits on it.
  private clip: Promise<boolean> = Promise.resolve(false)
  private clipId = 0
  private loading = false
  // True from a press of play until the clip is scheduled or given up on.
  private starting = false

  constructor() {
    this.player.onstatechange = (playing) => {
      this.playing = playing
    }
  }

  /** Fetches today's game and its clip. Call on page load, and again to retry. */
  async load(): Promise<void> {
    if (this.loading) return
    this.loading = true
    this.loadError = null
    try {
      this.apply(await getDaily())
    } catch (err) {
      this.loadError = describe(err)
    } finally {
      this.loading = false
    }
  }

  /**
   * Picks up changes made elsewhere (another tab, a new day) without touching
   * the page when nothing changed. Failures are ignored: the next move will
   * report them.
   */
  async refresh(): Promise<void> {
    if (this.loading || this.submitting || !this.daily) return
    this.loading = true
    try {
      const daily = await getDaily()
      if (!this.submitting) this.apply(daily)
    } catch {
      // Stale but usable.
    } finally {
      this.loading = false
    }
  }

  /**
   * Plays the unlocked clip from the start. Call it from a click or key
   * handler: browsers refuse to start audio anywhere else.
   */
  async play(): Promise<void> {
    // A second press while the first still waits for the clip would start it twice.
    if (!this.daily || this.starting) return
    this.starting = true
    try {
      // Synchronously inside the gesture, before anything is awaited.
      const unlocked = this.player.unlock()
      // After a failure the play button is the retry button.
      if (this.clipError) void this.loadClip()
      const clip = this.clip
      const [loaded] = await Promise.all([clip, unlocked])
      // A move replaced the clip while this one was loading; don't play the old turn's length.
      if (!loaded || clip !== this.clip) return
      await this.player.play(this.clipSeconds)
    } catch {
      this.clipError = "Couldn't play the clip. Press play to try again."
    } finally {
      this.starting = false
    }
  }

  stop(): void {
    this.player.stop()
  }

  /** The play button: starts the clip, or stops it if it is playing. */
  toggle(): void {
    if (this.playing) this.stop()
    else void this.play()
  }

  /** Where the playing clip is. Not reactive: read it from an animation frame. */
  progress(): ClipProgress {
    return this.player.progress()
  }

  /** Gives up the turn. Resolves to whether the server accepted the move. */
  skip(): Promise<boolean> {
    return this.move({ skip: true })
  }

  /** Guesses a song picked from the search. Resolves to whether the server accepted the move. */
  guess(track: Track): Promise<boolean> {
    return this.move({ trackId: track.id })
  }

  private async move(move: Move): Promise<boolean> {
    // `submitting` is the guard against a double tap sending two moves.
    if (this.submitting || !this.daily || this.finished) return false
    this.submitting = true
    this.moveError = null
    try {
      const daily = await postGuess(move)
      this.moves++
      this.apply(daily)
      this.notice = describeMove(daily)
      return true
    } catch (err) {
      if (err instanceof ApiError && err.code === 'finished') {
        // The game ended somewhere else (another tab). Show how.
        await this.reloadDaily()
      } else {
        this.moveError = describe(err)
      }
      return false
    } finally {
      this.submitting = false
    }
  }

  private async reloadDaily(): Promise<void> {
    try {
      this.apply(await getDaily())
    } catch (err) {
      this.moveError = describe(err)
    }
  }

  private apply(daily: Daily): void {
    const previous = this.daily
    this.daily = daily
    // The clip only changes with the day or the unlocked length.
    if (!previous || previous.day !== daily.day || audioUrl(previous) !== audioUrl(daily)) {
      if (previous && previous.day !== daily.day) this.notice = ''
      void this.loadClip()
    }
  }

  private loadClip(): Promise<boolean> {
    const daily = this.daily
    if (!daily) return Promise.resolve(false)
    const id = ++this.clipId
    this.clipLoading = true
    this.clipError = null
    this.clip = this.player.load(audioUrl(daily)).then(
      () => {
        if (id === this.clipId) this.clipLoading = false
        return true
      },
      () => {
        if (id === this.clipId) {
          this.clipLoading = false
          this.clipError = "Couldn't load the clip. Press play to try again."
        }
        return false
      },
    )
    return this.clip
  }
}

function describeMove(daily: Daily): string {
  if (daily.status === 'won') return 'Correct.'
  if (daily.status === 'lost') return 'No tries left.'
  const last = daily.attempts.at(-1)
  const unlocked = `You can now play ${clipLabel(daily.clipSeconds)}.`
  if (last?.kind === 'wrong') return `Not ${last.title} by ${last.artist}. ${unlocked}`
  return `Skipped. ${unlocked}`
}

/** The one game on the page. */
export const game = new Game()
