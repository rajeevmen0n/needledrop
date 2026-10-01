// The game as the page sees it: the four sections' games as the server last
// described them, which one is on screen, its loaded clip, and the moves. The
// server decides everything; this only mirrors what it answers.
//
// There is one `Game` and one audio player. Each section keeps what was last
// heard about it, so coming back to a tab shows its game at once; the fields
// the components read (`daily`, `status`, `submitting`, …) are always those of
// the section on screen.

import { ApiError, audioUrl, clearPlayer, getDaily, getToday, postGuess } from './api'
import type { Answer, Attempt, Daily, Move, Section, Stats, Status, Today, Track } from './api'
import { ClipPlayer } from './audio'
import type { ClipProgress } from './audio'
import { clipLabel } from './clip'
import { SECTIONS, isSection, tabState } from './sections'
import type { TabInfo, TabState } from './sections'

/** One row of the attempts list. */
export type Slot =
  | { kind: 'skip' }
  | { kind: 'wrong'; title: string; artist: string }
  | { kind: 'correct'; title: string; artist: string }
  | { kind: 'current' }
  | { kind: 'empty' }

/** One tab: a section and what is known of its game today. */
export interface Tab {
  section: Section
  state: TabState
}

function describe(err: unknown): string {
  return err instanceof ApiError ? err.message : 'Something went wrong. Try again.'
}

function codeOf(err: unknown): string {
  return err instanceof ApiError ? err.code : ''
}

/** What was last heard about one section's game. */
class SectionGame {
  /** The server's last answer. `null` until a load succeeds. */
  daily = $state<Daily | null>(null)
  /** True while the game is being asked for in a way the page shows. */
  loading = $state(false)
  /** Why the load failed, as a sentence for the player. */
  loadError = $state<string | null>(null)
  /** The section has no song today. */
  noSong = $state(false)
  /** True while a skip or guess is on its way. */
  submitting = $state(false)
  /** Why the last skip or guess failed. */
  moveError = $state<string | null>(null)
  /** What the last move did, in words, for a status line. */
  notice = $state('')
  /** Something the player should know that no move of theirs caused: a replaced song, a new day. */
  info = $state<string | null>(null)

  /** The newest request for this game; an older one that answers late is dropped. */
  request = 0
  /** Goes up whenever `daily` is replaced, so a slower answer can tell it is the older one. */
  version = 0

  /** Back to "nothing heard yet". A move still on its way finishes on its own. */
  forget(): void {
    this.daily = null
    this.loading = false
    this.loadError = null
    this.noSong = false
    this.moveError = null
    this.notice = ''
    this.info = null
    this.request++
    this.version++
  }
}

export class Game {
  /** The section on screen. */
  section = $state<Section>('general')
  /** The server's day, as last heard. Everything on screen is about this day. */
  day = $state<string | null>(null)
  /** The day's number, 1 on launch day. */
  number = $state<number | null>(null)
  /** True while the clip is being fetched and decoded. */
  clipLoading = $state(false)
  /** Why the clip cannot be played. */
  clipError = $state<string | null>(null)
  playing = $state(false)
  /** True from the move that ends the game on screen until the player looks elsewhere: the reveal runs once. */
  fresh = $state(false)
  /** True while Clear my data is on its way. */
  clearing = $state(false)

  private readonly games: Record<Section, SectionGame> = {
    general: new SectionGame(),
    pop: new SectionGame(),
    rock: new SectionGame(),
    'hip-hop': new SectionGame(),
  }
  /** What each tab shows: the overview's word, or a loaded game's when that is newer. */
  private known = $state<Record<Section, TabInfo | null>>({
    general: null,
    pop: null,
    rock: null,
    'hip-hop': null,
  })
  private current = $derived(this.games[this.section])

  /** The four tabs, in order. */
  tabs: Tab[] = $derived(
    SECTIONS.map((section) => ({ section, state: tabState(this.known[section]) })),
  )

  // --- the section on screen ---------------------------------------------------

  /** The server's last answer about the section on screen. `null` until its first load succeeds. */
  daily = $derived(this.current.daily)
  loading = $derived(this.current.loading)
  loadError = $derived(this.current.loadError)
  /** The section on screen has no song today. */
  noSong = $derived(this.current.noSong)
  /** True while a move in this section, or Clear my data, is on its way. */
  submitting = $derived(this.current.submitting || this.clearing)
  moveError = $derived(this.current.moveError)
  notice = $derived(this.current.notice)
  info = $derived(this.current.info)

  status: Status = $derived(this.daily?.status ?? 'playing')
  finished = $derived(this.status !== 'playing')
  attempts: Attempt[] = $derived(this.daily?.attempts ?? [])
  ladder: number[] = $derived(this.daily?.ladder ?? [])
  answer: Answer | null = $derived(this.daily?.answer ?? null)
  /** The player's record in the section on screen. */
  stats: Stats | null = $derived(this.daily?.stats ?? null)
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
  // True from a press of play until the clip is scheduled or given up on.
  private starting = false
  private opened = false
  private todayRequest = 0
  /** Goes up when everything heard so far stops being true: a new day, a new player. */
  private epoch = 0

  constructor() {
    this.player.onstatechange = (playing) => {
      this.playing = playing
    }
  }

  // --- which game is on screen --------------------------------------------------

  /** Opens the page on a section: asks for the overview and that section's game. Call on page load. */
  open(section: Section): void {
    this.opened = true
    this.section = section
    void this.fetchToday()
    void this.fetchGame(section)
  }

  /**
   * Puts another section on screen. The clip that is playing stops. A section
   * seen before shows what it showed (and is quietly asked about again, in
   * case it changed elsewhere); a new one is loaded.
   */
  select(section: Section): void {
    if (!this.opened) return this.open(section)
    if (section === this.section) return
    this.dropClip()
    this.fresh = false
    // What the last move did there is old news by the time the tab is opened again.
    this.current.notice = ''
    this.section = section
    const game = this.games[section]
    if (game.daily) {
      void this.loadClip()
      void this.fetchGame(section, true)
      return
    }
    // The overview said so in advance; the request below has the last word.
    if (tabState(this.known[section]) === 'none') game.noSong = true
    if (!game.loading) void this.fetchGame(section, game.noSong)
  }

  /** Asks again for the overview and the game on screen. The "Try again" of a failed load. */
  async load(): Promise<void> {
    if (this.current.loading) return
    void this.fetchToday()
    await this.fetchGame(this.section)
  }

  /**
   * Picks up changes made elsewhere (another tab, a new day, the admin)
   * without touching the page when nothing changed. Failures are ignored: the
   * next move will report them.
   */
  refresh(): void {
    if (!this.opened || this.clearing) return
    void this.fetchToday()
    const game = this.current
    if (game.submitting || game.loading) return
    void this.fetchGame(this.section, game.daily !== null || game.noSong)
  }

  // --- the clip -----------------------------------------------------------------

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
      // A move or another tab replaced the clip while this one was loading; don't play the old one.
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

  // --- moves --------------------------------------------------------------------

  /** Gives up the turn. Resolves to whether the server accepted the move. */
  skip(): Promise<boolean> {
    return this.move({ skip: true })
  }

  /** Guesses a song picked from the search. Resolves to whether the server accepted the move. */
  guess(track: Track): Promise<boolean> {
    return this.move({ trackId: track.id })
  }

  private async move(move: Move): Promise<boolean> {
    const section = this.section
    const game = this.games[section]
    // `submitting` is the guard against a double tap sending two moves.
    if (game.submitting || this.clearing || game.daily?.status !== 'playing') return false
    game.submitting = true
    game.moveError = null
    game.info = null
    try {
      const daily = await postGuess(section, move)
      this.receive(section, daily)
      game.notice = describeMove(daily)
      if (daily.status !== 'playing' && section === this.section) this.fresh = true
      return true
    } catch (err) {
      switch (codeOf(err)) {
        case 'finished':
          // The game ended somewhere else (another tab). Show how.
          await this.fetchGame(section)
          break
        case 'changed':
          // The admin replaced the song while the move was on its way: it was not made.
          game.info = 'The song in this section was changed, so that move did not count. The game starts again.'
          void this.fetchToday()
          await this.fetchGame(section)
          break
        case 'no_song':
          this.songGone(section)
          void this.fetchToday()
          break
        default:
          game.moveError = describe(err)
      }
      return false
    } finally {
      game.submitting = false
    }
  }

  // --- clear my data ------------------------------------------------------------

  /** True while any section has a move on its way. */
  get busy(): boolean {
    return SECTIONS.some((section) => this.games[section].submitting)
  }

  /**
   * Deletes every game stored for this browser and starts over as a new
   * player. Resolves to `null` when it is done, or to why it failed, in which
   * case nothing on screen changes.
   */
  async clearData(): Promise<string | null> {
    // A move answered after the deletion would bring the old player back with that game.
    if (this.clearing || this.busy) return 'A move is still on its way. Try again in a moment.'
    this.clearing = true
    try {
      await clearPlayer()
    } catch (err) {
      return describe(err)
    } finally {
      this.clearing = false
    }
    this.forget()
    void this.fetchToday()
    void this.fetchGame(this.section)
    return null
  }

  // --- what the server says -------------------------------------------------------

  /**
   * Asks for a section's game. `quiet` is for a game that is already on
   * screen: nothing shows that it is being asked for, and a failure changes
   * nothing.
   */
  private async fetchGame(section: Section, quiet = false): Promise<void> {
    const game = this.games[section]
    const id = ++game.request
    const epoch = this.epoch
    const version = game.version
    if (!quiet) {
      game.loading = true
      game.loadError = null
    }
    try {
      const daily = await getDaily(section)
      if (epoch !== this.epoch || id !== game.request) return
      // A move made in the meantime is newer than this answer.
      if (quiet && (game.submitting || version !== game.version)) return
      this.receive(section, daily)
    } catch (err) {
      if (epoch !== this.epoch || id !== game.request) return
      if (codeOf(err) === 'no_song') this.songGone(section)
      else if (quiet) return // Stale but usable.
      else if (game.daily) game.moveError = describe(err)
      else game.loadError = describe(err)
    } finally {
      if (id === game.request) game.loading = false
    }
  }

  /** Asks for the overview behind the tabs. A failure leaves the tabs as they were. */
  private async fetchToday(): Promise<void> {
    const id = ++this.todayRequest
    const versions = SECTIONS.map((section) => this.games[section].version)
    let today: Today
    try {
      today = await getToday()
    } catch {
      return
    }
    if (id !== this.todayRequest) return
    const turned = this.noteDay(today.day)
    this.number = today.number
    today.sections.forEach((entry) => {
      if (!isSection(entry.section)) return
      const index = SECTIONS.indexOf(entry.section)
      // A game that arrived while this was on its way is the newer word on its section.
      if (!turned && this.games[entry.section].version !== versions[index]) return
      this.known[entry.section] = {
        status: entry.status,
        attempts: entry.attempts,
        song: entry.song,
      }
    })
    if (turned) void this.fetchGame(this.section)
  }

  /** Takes in a section's game, from a load or a move. */
  private receive(section: Section, daily: Daily): void {
    const turned = this.noteDay(daily.day)
    const game = this.games[section]
    const previous = game.daily
    game.daily = daily
    game.version++
    game.noSong = false
    game.loadError = null
    this.number = daily.number
    // The tab follows the game without asking the overview again.
    this.known[section] = {
      status: daily.status,
      attempts: daily.attempts.length,
      song: 'picked',
    }
    // The clip only changes with the day or the unlocked length.
    if (section === this.section && (!previous || audioUrl(previous) !== audioUrl(daily))) {
      void this.loadClip()
    }
    if (turned) {
      void this.fetchToday()
      if (section !== this.section) void this.fetchGame(this.section)
    }
  }

  /**
   * Notes the day of a response. When it is not the day on screen, midnight
   * (or the admin's clock) has passed: everything heard so far is dropped,
   * and the caller asks again. Returns whether that happened.
   */
  private noteDay(day: string): boolean {
    if (this.day === day) return false
    const first = this.day === null
    this.day = day
    if (first) return false
    this.forget()
    this.current.info = "The day has changed. This is today's game."
    return true
  }

  /** Drops everything heard about the games; requests still on their way are ignored when they answer. */
  private forget(): void {
    this.epoch++
    this.dropClip()
    this.fresh = false
    for (const section of SECTIONS) {
      this.games[section].forget()
      this.known[section] = null
    }
  }

  /** The server says the section has no song today. */
  private songGone(section: Section): void {
    const game = this.games[section]
    game.daily = null
    game.version++
    game.noSong = true
    game.loadError = null
    this.known[section] = { status: 'playing', attempts: 0, song: 'none' }
    if (section === this.section) this.dropClip()
  }

  /** Stops the sound and forgets the loaded clip: what is in the player belongs to another game. */
  private dropClip(): void {
    this.clipId++
    this.player.stop()
    this.clip = Promise.resolve(false)
    this.clipLoading = false
    this.clipError = null
  }

  /** Loads the clip of the game on screen. */
  private loadClip(): Promise<boolean> {
    const daily = this.current.daily
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
