// The game as the page sees it: the four sections' games and the random song
// as the server last described them, which one is on screen, its loaded clip,
// and the moves. The server decides everything; this only mirrors what it
// answers.
//
// There is one `Game` and one audio player. Each tab keeps what was last heard
// about it, so coming back to a tab shows its game at once; the fields the
// components read (`view`, `status`, `submitting`, …) are always those of the
// tab on screen.
//
// Random mode is the fifth tab. To the record, the rail, the controls and the
// tries it is a game like the others (`GameView`); what differs is where it
// comes from. It has no day: a new day leaves it alone. It belongs to a
// session, which is the player's and the server's to keep: every browser tab
// of the player is in the same one, and the server ends it after half an hour
// without playing. A song is followed by the next one for as long as the
// player likes. Which songs can follow is the player's choice of pool (all of
// them, or one genre), kept by the server with the session; choosing never
// replaces the song being played.

import {
  ApiError,
  audioUrl,
  clearPlayer,
  getDaily,
  getRandom,
  getToday,
  isRandomSong,
  postGuess,
  postRandomGuess,
  postRandomNext,
  startRandom,
} from './api'
import type {
  Answer,
  Attempt,
  Daily,
  GameView,
  Move,
  RandomScore,
  RandomSong,
  Section,
  Stats,
  Status,
  Today,
  Track,
} from './api'
import { ClipPlayer } from './audio'
import type { ClipProgress } from './audio'
import { clipLabel } from './clip'
import { emptyLine, poolNews, poolOf, remedyFor, sameSession } from './random'
import { RANDOM, SECTIONS, TABS, isSection, tabState } from './sections'
import type { Tab, TabInfo, TabState } from './sections'

/** One row of the attempts list. */
export type Slot =
  | { kind: 'skip' }
  | { kind: 'wrong'; title: string; artist: string }
  | { kind: 'correct'; title: string; artist: string }
  | { kind: 'current' }
  | { kind: 'empty' }

/** One tab of the row: which it is and what is known of its game. */
export interface TabMark {
  tab: Tab
  state: TabState
}

const SONG_CHANGED = 'This song was changed in another tab, so that move did not count.'
const SESSION_ENDED = 'That session ended after a while away. This is a new one.'

function describe(err: unknown): string {
  return err instanceof ApiError ? err.message : 'Something went wrong. Try again.'
}

function codeOf(err: unknown): string {
  return err instanceof ApiError ? err.code : ''
}

/** What was last heard about one tab's game. */
class TabGame<View extends GameView> {
  /** The server's last answer. `null` until a load succeeds. */
  view = $state<View | null>(null)
  /** True while the game is being asked for in a way the page shows. */
  loading = $state(false)
  /** Why the load failed, as a sentence for the player. */
  loadError = $state<string | null>(null)
  /** There is no song to play: none today in a section, none to draw in random mode. */
  noSong = $state(false)
  /** True while a skip or guess, or a random session's start or next song, is on its way. */
  submitting = $state(false)
  /** Why the last skip or guess failed. */
  moveError = $state<string | null>(null)
  /** What the last move did, in words, for a status line. */
  notice = $state('')
  /** Something the player should know that no move of theirs caused: a replaced song, a new day. */
  info = $state<string | null>(null)

  /** The newest request for this game; an older one that answers late is dropped. */
  request = 0
  /** Goes up whenever `view` is replaced, so a slower answer can tell it is the older one. */
  version = 0

  /** Back to "nothing heard yet". A move still on its way finishes on its own. */
  forget(): void {
    this.view = null
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
  /** The tab on screen: a section, or random mode. */
  tab = $state<Tab>('general')
  /** The server's day, as last heard. The four sections on screen are about this day. */
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
  /** True while random mode's next song is being drawn. */
  drawing = $state(false)

  private readonly games = {
    general: new TabGame<Daily>(),
    pop: new TabGame<Daily>(),
    rock: new TabGame<Daily>(),
    'hip-hop': new TabGame<Daily>(),
    random: new TabGame<RandomSong>(),
  }
  /** What each section's tab shows: the overview's word, or a loaded game's when that is newer. */
  private known = $state<Record<Section, TabInfo | null>>({
    general: null,
    pop: null,
    rock: null,
    'hip-hop': null,
  })
  private current = $derived(this.games[this.tab])
  /**
   * The pool whose draw last found nothing while no random song is on screen
   * to say which one the session has. A start asks for it again, and a song
   * that arrives takes its place. `null`: there is a song, or the page does
   * not know which pool the server tried.
   */
  private wanted = $state<Section | null>(null)

  /** The five tabs, in order. Random mode's never changes: it has no game of the day. */
  tabs: TabMark[] = $derived(
    TABS.map((tab) => ({
      tab,
      state: tab === RANDOM ? 'endless' : tabState(this.known[tab]),
    })),
  )

  // --- the tab on screen -------------------------------------------------------

  /** Random mode is on screen. */
  random = $derived(this.tab === RANDOM)
  /** The server's last answer about the game on screen. `null` until its first load succeeds. */
  view: Daily | RandomSong | null = $derived(this.current.view)
  loading = $derived(this.current.loading)
  loadError = $derived(this.current.loadError)
  /** The tab on screen has no song to play. */
  noSong = $derived(this.current.noSong)
  /** True while a move in this tab, or Clear my data, is on its way. */
  submitting = $derived(this.current.submitting || this.clearing)
  moveError = $derived(this.current.moveError)
  notice = $derived(this.current.notice)
  info = $derived(this.current.info)

  status: Status = $derived(this.view?.status ?? 'playing')
  finished = $derived(this.status !== 'playing')
  attempts: Attempt[] = $derived(this.view?.attempts ?? [])
  ladder: number[] = $derived(this.view?.ladder ?? [])
  answer: Answer | null = $derived(this.view?.answer ?? null)
  /** The player's record in the section on screen; `null` in random mode. */
  stats: Stats | null = $derived(this.view && !isRandomSong(this.view) ? this.view.stats : null)
  /** The random song on screen, which carries the session's score; `null` on a section's tab. */
  song: RandomSong | null = $derived(this.view && isRandomSong(this.view) ? this.view : null)
  /** The random session's score; `null` on a section's tab. */
  score: RandomScore | null = $derived(this.song)
  /**
   * The pool random mode's next songs are drawn from, as the page shows it:
   * the one the random song names; without a song, the one whose draw found
   * nothing here; else the whole pool. It is the same on every tab.
   */
  pool: Section = $derived(poolOf(this.games.random.view?.pool ?? this.wanted))
  /**
   * Which game is on screen: a section on a day, or one random song. When it
   * changes, what is shown next is another game and not this one moving on.
   */
  scene = $derived(
    this.tab === RANDOM
      ? `${RANDOM} ${this.games.random.view?.round ?? ''}`
      : `${this.tab} ${this.day ?? ''}`,
  )
  /** Length of the whole preview in seconds: the last ladder step. */
  totalSeconds = $derived(this.ladder.at(-1) ?? 0)
  /** The clip length unlocked now. */
  clipSeconds = $derived(this.view?.clipSeconds ?? 0)
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
  /** The overview and daily games being asked for right now; see `startSession` for who waits for them. */
  private readonly asking = new Set<Promise<unknown>>()
  /** Goes up when everything heard about the day's games stops being true: a new day, a new player. */
  private epoch = 0
  /** The same for the random game, which a new day leaves alone: a new player. */
  private randomEpoch = 0

  constructor() {
    this.player.onstatechange = (playing) => {
      this.playing = playing
    }
  }

  // --- which game is on screen --------------------------------------------------

  /** Opens the page on a tab: asks for the overview and that tab's game. Call on page load. */
  open(tab: Tab): void {
    this.opened = true
    this.tab = tab
    void this.fetchToday()
    void this.fetchTab(tab)
  }

  /**
   * Puts another tab on screen. The clip that is playing stops. A tab seen
   * before shows what it showed (and is quietly asked about again, in case it
   * changed elsewhere); a new one is loaded.
   */
  select(tab: Tab): void {
    if (!this.opened) return this.open(tab)
    if (tab === this.tab) return
    this.dropClip()
    this.fresh = false
    // What the last move did there is old news by the time the tab is opened again.
    this.current.notice = ''
    this.tab = tab
    const game = this.games[tab]
    if (game.view) {
      void this.loadClip()
      void this.fetchTab(tab, true)
      return
    }
    // The overview said so in advance; the request below has the last word.
    if (tab !== RANDOM && tabState(this.known[tab]) === 'none') game.noSong = true
    if (!game.loading && !game.submitting) void this.fetchTab(tab, game.noSong)
  }

  /**
   * Asks again for the overview and the game on screen. The "Try again" of a
   * failed load and the "Check again" of a tab without a song.
   */
  async load(): Promise<void> {
    if (this.current.loading) return
    void this.fetchToday()
    const random = this.games.random
    if (this.tab === RANDOM && random.noSong) {
      // The draw after a finished song found nothing: asking again is that draw again.
      if (random.view) await this.next()
      // No session could be begun: asking again is for the pool the page shows as chosen.
      else await this.fetchRandom(false, this.pool)
      return
    }
    await this.fetchTab(this.tab)
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
    void this.fetchTab(this.tab, game.view !== null || game.noSong)
  }

  // --- the clip -----------------------------------------------------------------

  /**
   * Plays the unlocked clip from the start. Call it from a click or key
   * handler: browsers refuse to start audio anywhere else.
   */
  async play(): Promise<void> {
    // A second press while the first still waits for the clip would start it twice.
    if (!this.view || this.starting) return
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

  /** Settles when the clip of the game on screen has loaded, or has failed to. */
  async clipSettled(): Promise<void> {
    await this.clip
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
    const section = this.tab
    if (section === RANDOM) return this.moveRandom(move)
    const game = this.games[section]
    // `submitting` is the guard against a double tap sending two moves.
    if (game.submitting || this.clearing || game.view?.status !== 'playing') return false
    game.submitting = true
    game.moveError = null
    game.info = null
    try {
      const daily = await postGuess(section, move)
      this.receive(section, daily)
      game.notice = describeMove(daily)
      if (daily.status !== 'playing' && section === this.tab) this.fresh = true
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

  /** A skip or guess on the random song. It names the round, so it cannot land on another song. */
  private async moveRandom(move: Move): Promise<boolean> {
    const game = this.games.random
    const song = game.view
    if (game.submitting || this.clearing || song?.status !== 'playing') return false
    game.submitting = true
    game.moveError = null
    game.info = null
    let failure: unknown
    try {
      const after = await postRandomGuess(song.round, move)
      this.receiveRandom(after)
      // How a song ended is the reveal's to say; the status line is kept for what follows it.
      game.notice = after.status === 'playing' ? describeMove(after) : ''
      if (after.status !== 'playing' && this.tab === RANDOM) this.fresh = true
      return true
    } catch (err) {
      failure = err
    } finally {
      game.submitting = false
    }
    await this.randomRefused(failure)
    return false
  }

  /**
   * Random mode: draws the next song, once the one on screen is over. Resolves
   * to whether there is a new song to play.
   */
  async next(): Promise<boolean> {
    const game = this.games.random
    const song = game.view
    // `submitting` is the guard against a double tap drawing two songs.
    if (game.submitting || this.clearing || !song || song.status === 'playing') return false
    game.submitting = true
    this.drawing = true
    game.moveError = null
    game.info = null
    let failure: unknown
    try {
      const drawn = await postRandomNext(song.round)
      this.receiveRandom(drawn)
      game.notice = `Next song. You can play ${clipLabel(drawn.clipSeconds)}.`
      return true
    } catch (err) {
      failure = err
    } finally {
      game.submitting = false
      this.drawing = false
    }
    await this.randomRefused(failure)
    return false
  }

  /**
   * Random mode: chooses the pool the next songs are drawn from. It is a
   * start that names the pool, so it follows a start's rules (one at a time,
   * after the page's first requests, and Clear my data waits for it), and it
   * is never a way out of a song: a session that is going keeps its song, its
   * tries and its score, and the choice waits for the next draw. Where the
   * last draw found nothing, that draw is asked for again. Resolves to whether
   * the choice stands.
   */
  async choosePool(pool: Section): Promise<boolean> {
    const game = this.games.random
    if (game.submitting || game.loading || this.clearing) return false
    const before = game.view
    const empty = game.noSong
    // The session draws from it already, and nothing is waiting for another try.
    if (before && !empty && this.pool === pool) return true
    const id = ++game.request
    const epoch = this.randomEpoch
    game.submitting = true
    // Where there is no song, the choice is also the next attempt to find one.
    game.loading = empty
    game.moveError = null
    game.loadError = null
    game.info = null
    let song: RandomSong
    try {
      // As in `startSession`: a new browser's first answers each bring a player.
      await Promise.allSettled([...this.asking])
      if (epoch !== this.randomEpoch) return false
      song = await startRandom(pool)
      if (epoch !== this.randomEpoch) return false
    } catch (err) {
      if (epoch !== this.randomEpoch) return false
      if (remedyFor(codeOf(err)) === 'empty') {
        // No session was going (the one on screen, if any, had ended), and
        // that pool has nothing for a new one.
        this.nothingToDraw(pool)
        game.notice = emptyLine(pool)
      } else if (game.view) {
        // The choice was not made: the page goes on showing the pool that stands.
        game.moveError = describe(err)
      } else game.loadError = describe(err)
      return false
    } finally {
      game.submitting = false
      if (id === game.request) game.loading = false
    }
    // The session on screen had ended while the page was open: this start began another.
    const ended = before !== null && !sameSession(before, song)
    this.receiveRandom(song)
    if (ended) game.info = SESSION_ENDED
    const open = song.status === 'playing'
    game.notice =
      open && before?.round !== song.round
        ? `${poolNews(this.pool, false)} You can play ${clipLabel(song.clipSeconds)}.`
        : poolNews(this.pool, open)
    if (empty && !open) {
      // The finished song is still there, and so is the draw that found nothing: again, from this pool.
      game.noSong = true
      await this.next()
      if (game.noSong) game.notice = emptyLine(this.pool)
    }
    return true
  }

  /** The server refused a random move or draw: does what that calls for (see `remedyFor`). */
  private async randomRefused(err: unknown): Promise<void> {
    const game = this.games.random
    switch (remedyFor(codeOf(err))) {
      case 'reload':
        await this.fetchRandom()
        break
      case 'tell':
        // Another tab of this player has moved on to another song.
        game.info = SONG_CHANGED
        await this.fetchRandom()
        break
      case 'restart':
        // The session on screen is over: nothing was played for half an hour
        // (or the data was cleared elsewhere, or the admin reset everything).
        game.info = SESSION_ENDED
        await this.startSession()
        break
      case 'empty':
        // What is on screen stays as it is; "Check again" asks for the draw again.
        game.noSong = true
        break
      default:
        game.moveError = describe(err)
    }
  }

  // --- clear my data ------------------------------------------------------------

  /** True while any tab has a move on its way. */
  get busy(): boolean {
    return TABS.some((tab) => this.games[tab].submitting)
  }

  /**
   * Deletes every game stored for this browser, the random one included, and
   * starts over as a new player. Resolves to `null` when it is done, or to why
   * it failed, in which case nothing on screen changes.
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
    this.forgetDay()
    this.forgetRandom()
    void this.fetchToday()
    // On the Random tab this starts a new session: the old one went with the data.
    void this.fetchTab(this.tab)
    return null
  }

  // --- what the server says -------------------------------------------------------

  /** Asks for a tab's game: a section's, or the random song. `quiet` as in `fetchGame`. */
  private fetchTab(tab: Tab, quiet = false): Promise<void> {
    return tab === RANDOM ? this.fetchRandom(quiet) : this.fetchGame(tab, quiet)
  }

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
      const daily = await this.asked(getDaily(section))
      if (epoch !== this.epoch || id !== game.request) return
      // A move made in the meantime is newer than this answer.
      if (quiet && (game.submitting || version !== game.version)) return
      this.receive(section, daily)
    } catch (err) {
      if (epoch !== this.epoch || id !== game.request) return
      if (codeOf(err) === 'no_song') this.songGone(section)
      else if (quiet) return // Stale but usable.
      else if (game.view) game.moveError = describe(err)
      else game.loadError = describe(err)
    } finally {
      if (id === game.request) game.loading = false
    }
  }

  /**
   * Asks for the session the player is in, whichever browser tab began it.
   * When the server has none (they never played, or it ended while they were
   * away), one is started: without a word when nothing was on screen, and
   * saying so when a game was. `quiet` as in `fetchGame`; `pool` is the one
   * to name if a session has to be started.
   */
  private async fetchRandom(quiet = false, pool?: Section): Promise<void> {
    const game = this.games.random
    const id = ++game.request
    const epoch = this.randomEpoch
    const version = game.version
    if (!quiet) {
      game.loading = true
      game.loadError = null
    }
    try {
      const song = await getRandom()
      if (epoch !== this.randomEpoch || id !== game.request) return
      // A move made in the meantime is newer than this answer.
      if (quiet && (game.submitting || version !== game.version)) return
      this.receiveRandom(song)
    } catch (err) {
      if (epoch !== this.randomEpoch || id !== game.request) return
      if (remedyFor(codeOf(err)) === 'restart') {
        // What is on screen, if anything, is a session that has ended.
        if (game.view) game.info = SESSION_ENDED
        void this.startSession(pool)
      } else if (quiet) return // Stale but usable.
      else if (game.view) game.moveError = describe(err)
      else game.loadError = describe(err)
    } finally {
      if (id === game.request) game.loading = false
    }
  }

  /**
   * Asks for the session to play in: a new one, with a first song and the run
   * and the totals at zero, or the one another browser tab has begun in the
   * meantime. One request at a time, so a tab opened twice in a hurry asks once.
   *
   * It names a pool only when the page has one to name that the server may
   * not have: `pool`, or the one whose draw found nothing here. Otherwise the
   * server goes on with the player's last choice.
   */
  private async startSession(pool?: Section): Promise<void> {
    const game = this.games.random
    if (game.submitting) return
    const id = ++game.request
    const epoch = this.randomEpoch
    const asked = pool ?? this.wanted ?? undefined
    // A start is a move as far as Clear my data is concerned: its answer sets the player cookie.
    game.submitting = true
    game.loading = true
    game.loadError = null
    try {
      // A browser the server has not met is given a player by every answer,
      // and keeps the one that arrives last. A start sent beside the page's
      // first requests could so lose its game to another answer's player.
      // Once those are answered, every request names the same player.
      await Promise.allSettled([...this.asking])
      if (epoch !== this.randomEpoch) return
      const song = await startRandom(asked)
      if (epoch !== this.randomEpoch) return
      this.receiveRandom(song)
    } catch (err) {
      if (epoch !== this.randomEpoch) return
      if (remedyFor(codeOf(err)) === 'empty') {
        // Unnamed, it was the pool of the session that ended, if one was on screen.
        this.nothingToDraw(asked ?? (game.view ? this.pool : null))
      } else if (game.view) game.moveError = describe(err)
      else game.loadError = describe(err)
    } finally {
      game.submitting = false
      if (id === game.request) game.loading = false
    }
  }

  /**
   * A new session found nothing to draw, so there is no random song: the page
   * says so. `pool` is the one that had nothing, when the page knows it; it is
   * shown as the choice and asked for again by the next start.
   */
  private nothingToDraw(pool: Section | null): void {
    const game = this.games.random
    game.view = null
    game.version++
    game.noSong = true
    this.wanted = pool
    if (this.tab === RANDOM) this.dropClip()
  }

  /** Notes a request as being on its way until it is answered. */
  private asked<T>(request: Promise<T>): Promise<T> {
    this.asking.add(request)
    const done = () => void this.asking.delete(request)
    request.then(done, done)
    return request
  }

  /** Asks for the overview behind the tabs. A failure leaves the tabs as they were. */
  private async fetchToday(): Promise<void> {
    const id = ++this.todayRequest
    const versions = SECTIONS.map((section) => this.games[section].version)
    let today: Today
    try {
      today = await this.asked(getToday())
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
    const tab = this.tab
    if (turned && tab !== RANDOM) void this.fetchGame(tab)
  }

  /** Takes in a section's game, from a load or a move. */
  private receive(section: Section, daily: Daily): void {
    const turned = this.noteDay(daily.day)
    const game = this.games[section]
    const previous = game.view
    game.view = daily
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
    if (section === this.tab && (!previous || audioUrl(previous) !== audioUrl(daily))) {
      void this.loadClip()
    }
    if (turned) {
      void this.fetchToday()
      const tab = this.tab
      if (tab !== RANDOM && tab !== section) void this.fetchGame(tab)
    }
  }

  /** Takes in the random song, from a load, a move, a start or a draw. */
  private receiveRandom(song: RandomSong): void {
    const game = this.games.random
    const previous = game.view
    game.view = song
    game.version++
    game.noSong = false
    game.loadError = null
    // The song says which pool the session has; nothing is left to ask for.
    this.wanted = null
    // What the last move did is old news once the song has moved on; a caller with news says it after this.
    if (!previous || audioUrl(previous) !== audioUrl(song)) game.notice = ''
    if (this.tab !== RANDOM) return
    // Another song: whatever ended the last one is not news about this one.
    if (previous?.round !== song.round) this.fresh = false
    // The clip only changes with the song or the unlocked length.
    if (!previous || audioUrl(previous) !== audioUrl(song)) void this.loadClip()
  }

  /**
   * Notes the day of a response. When it is not the day on screen, midnight
   * (or the admin's clock) has passed: everything heard about the day's games
   * is dropped, and the caller asks again. Returns whether that happened.
   */
  private noteDay(day: string): boolean {
    if (this.day === day) return false
    const first = this.day === null
    this.day = day
    if (first) return false
    this.forgetDay()
    if (this.tab !== RANDOM) this.current.info = "The day has changed. This is today's game."
    return true
  }

  /**
   * Drops everything heard about the day's four games; requests still on
   * their way are ignored when they answer. The random game has no day and
   * stays as it is, with its clip when it is the one on screen.
   */
  private forgetDay(): void {
    this.epoch++
    if (this.tab !== RANDOM) {
      this.dropClip()
      this.fresh = false
    }
    for (const section of SECTIONS) {
      this.games[section].forget()
      this.known[section] = null
    }
  }

  /** Drops the random game: the player it belonged to is gone. */
  private forgetRandom(): void {
    this.randomEpoch++
    if (this.tab === RANDOM) {
      this.dropClip()
      this.fresh = false
    }
    this.games.random.forget()
    this.wanted = null
  }

  /** The server says the section has no song today. */
  private songGone(section: Section): void {
    const game = this.games[section]
    game.view = null
    game.version++
    game.noSong = true
    game.loadError = null
    this.known[section] = { status: 'playing', attempts: 0, song: 'none' }
    if (section === this.tab) this.dropClip()
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
    const view = this.current.view
    if (!view) return Promise.resolve(false)
    const id = ++this.clipId
    this.clipLoading = true
    this.clipError = null
    this.clip = this.player.load(audioUrl(view)).then(
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

function describeMove(view: GameView): string {
  if (view.status === 'won') return 'Correct.'
  if (view.status === 'lost') return 'No tries left.'
  const last = view.attempts.at(-1)
  const unlocked = `You can now play ${clipLabel(view.clipSeconds)}.`
  if (last?.kind === 'wrong') return `Not ${last.title} by ${last.artist}. ${unlocked}`
  return `Skipped. ${unlocked}`
}

/** The one game on the page. */
export const game = new Game()
