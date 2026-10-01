// Web Audio clip player. Framework-free: it knows nothing about the game.
//
// Graph: source -> clip gain (fades) -> output gain -> analyser -> speakers.
// Each play gets its own source and clip gain, so stopping one clip can never
// disturb the fades of the next.

import { findAudibleStart, playableSeconds } from './clip'

/** Fade at each end of a clip. Without it a 0.1 second clip starts and ends with a click. */
const FADE_SECONDS = 0.005

/**
 * Clips start this far in the future. A start time of "now" is already in the
 * past when the audio thread reads it, and the fade-in would begin part-way up.
 */
const LEAD_SECONDS = 0.03

/** Previews are 44.1 kHz, so decoding at that rate does not resample them. */
const DECODE_RATE = 44100

export interface ClipProgress {
  /** Seconds played so far. */
  elapsed: number
  /** Seconds this play will last. */
  duration: number
  /** `elapsed / duration`, 0 to 1. */
  fraction: number
}

interface Playback {
  source: AudioBufferSourceNode
  gain: GainNode
  startsAt: number
  duration: number
}

const NOT_PLAYING: ClipProgress = { elapsed: 0, duration: 0, fraction: 0 }

export class ClipPlayer {
  /** Called with `true` when a clip starts and `false` when it ends or is stopped. */
  onstatechange: ((playing: boolean) => void) | null = null

  private context: AudioContext | null = null
  private output: GainNode | null = null
  private tap: AnalyserNode | null = null
  private decoder: OfflineAudioContext | null = null

  private buffer: AudioBuffer | null = null
  private startOffset = 0
  private current: Playback | null = null

  // Both go up on every call, so an older call that is still awaiting can tell it was replaced.
  private loadId = 0
  private playId = 0

  /** True while a clip is sounding (or about to, within the lead time). */
  get playing(): boolean {
    return this.current !== null
  }

  /** True once a clip is loaded and can be played. */
  get ready(): boolean {
    return this.buffer !== null
  }

  /** Seconds of sound in the loaded clip, after the leading silence. 0 when nothing is loaded. */
  get available(): number {
    return this.buffer ? playableSeconds(Infinity, this.buffer.duration, this.startOffset) : 0
  }

  /** Seconds of leading silence skipped in the loaded clip. */
  get skipped(): number {
    return this.startOffset
  }

  /** Everything that plays passes through this node. `null` until the first `unlock()` or `play()`. */
  get analyser(): AnalyserNode | null {
    return this.tap
  }

  /**
   * Creates the AudioContext, or wakes it up. Browsers only allow this inside
   * a user gesture, so call it synchronously from a click or key handler.
   */
  unlock(): Promise<void> {
    const context = this.ensureContext()
    // Not 'suspended': iOS Safari also has an 'interrupted' state.
    return context.state === 'running' ? Promise.resolve() : context.resume()
  }

  /**
   * Fetches and decodes a clip, replacing the loaded one. Stops playback.
   * Rejects when the request or the decoding fails.
   */
  async load(url: string): Promise<void> {
    const id = ++this.loadId
    this.stop()
    this.buffer = null
    this.startOffset = 0

    const res = await fetch(url, { cache: 'no-store' })
    if (!res.ok) throw new Error(`Audio request failed with status ${res.status}`)
    const bytes = await res.arrayBuffer()
    // An offline context decodes without a user gesture, so the clip is ready before the first tap.
    this.decoder ??= new OfflineAudioContext(1, 1, DECODE_RATE)
    const buffer = await this.decoder.decodeAudioData(bytes)
    if (id !== this.loadId) return

    const channels: Float32Array[] = []
    for (let c = 0; c < buffer.numberOfChannels; c++) channels.push(buffer.getChannelData(c))
    this.startOffset = findAudibleStart(channels, buffer.sampleRate)
    this.buffer = buffer
  }

  /**
   * Plays the loaded clip from its first audible sample for `seconds`, or for
   * as much as there is. Call it from a user gesture. Calling it while a clip
   * plays restarts from the beginning. Resolves once the clip is scheduled.
   */
  async play(seconds: number): Promise<void> {
    this.stop()
    const id = ++this.playId
    const context = this.ensureContext()
    // resume() has to be called inside the gesture; waiting for it afterwards is fine.
    if (context.state !== 'running') await context.resume()
    if (id !== this.playId) return

    const buffer = this.buffer
    const output = this.output
    if (!buffer || !output) throw new Error('No clip is loaded')
    const duration = playableSeconds(seconds, buffer.duration, this.startOffset)
    if (duration <= 0) throw new Error('The loaded clip is empty')

    const fade = Math.min(FADE_SECONDS, duration / 2)
    const startsAt = context.currentTime + LEAD_SECONDS
    const endsAt = startsAt + duration

    const gain = context.createGain()
    gain.gain.value = 0
    gain.gain.setValueAtTime(0, startsAt)
    gain.gain.linearRampToValueAtTime(1, startsAt + fade)
    gain.gain.setValueAtTime(1, endsAt - fade)
    gain.gain.linearRampToValueAtTime(0, endsAt)

    const source = context.createBufferSource()
    source.buffer = buffer
    source.connect(gain)
    gain.connect(output)

    const playback: Playback = { source, gain, startsAt, duration }
    source.onended = () => {
      source.disconnect()
      gain.disconnect()
      // After stop() or a restart this is no longer the current clip, and the state was already reported.
      if (this.current !== playback) return
      this.current = null
      this.onstatechange?.(false)
    }
    // The audio clock times both ends to the sample; no timers involved.
    source.start(startsAt, this.startOffset, duration)

    this.current = playback
    this.onstatechange?.(true)
  }

  /** Stops the clip, with a short fade so the cut does not click. Safe to call when nothing plays. */
  stop(): void {
    // Also cancels a play() still waiting for the context to wake up.
    this.playId++
    const playback = this.current
    const context = this.context
    if (!playback || !context) return
    this.current = null

    const { source, gain, startsAt, duration } = playback
    const now = context.currentTime
    if (now < startsAt) {
      source.stop()
    } else if (startsAt + duration - now > FADE_SECONDS * 2) {
      // Dropping the scheduled fade-out leaves the gain at 1; decay from there instead.
      gain.gain.cancelScheduledValues(now)
      gain.gain.setTargetAtTime(0, now, FADE_SECONDS / 3)
      source.stop(now + FADE_SECONDS * 2)
    }
    // Otherwise the scheduled fade-out is already running; let it finish.
    this.onstatechange?.(false)
  }

  /**
   * Where the playing clip is, read from the audio clock. Cheap enough to call
   * every animation frame. All zeros when nothing plays.
   */
  progress(): ClipProgress {
    const playback = this.current
    if (!playback || !this.context) return NOT_PLAYING
    const elapsed = Math.min(Math.max(this.context.currentTime - playback.startsAt, 0), playback.duration)
    return { elapsed, duration: playback.duration, fraction: elapsed / playback.duration }
  }

  private ensureContext(): AudioContext {
    if (this.context) return this.context
    const context = new AudioContext({ latencyHint: 'interactive' })
    const output = context.createGain()
    const tap = context.createAnalyser()
    tap.fftSize = 2048
    // In series rather than a side branch: some browsers only run an analyser that reaches the speakers.
    output.connect(tap)
    tap.connect(context.destination)
    this.context = context
    this.output = output
    this.tap = tap
    return context
  }
}
