// Pure helpers for clip lengths and decoded samples. No DOM and no imports, so
// `node --test` can load this file directly (see web/test/).

/** Samples quieter than this count as silence: about -54 dBFS. */
export const SILENCE_THRESHOLD = 0.002

/** Never skip more than this, however quiet the start of the song is. */
export const MAX_SILENCE_SKIP = 0.06

/**
 * Seconds of leading silence in a decoded clip.
 *
 * MP3 decoders put 25-50 ms of priming silence in front of the audio. Left in,
 * it would eat a third of the 0.1 second clip. The cap keeps a song that really
 * does open quietly from losing more than 60 ms.
 */
export function findAudibleStart(
  channels: readonly Float32Array[],
  sampleRate: number,
  threshold: number = SILENCE_THRESHOLD,
  maxSeconds: number = MAX_SILENCE_SKIP,
): number {
  const length = Math.min(...channels.map((channel) => channel.length))
  if (!Number.isFinite(length) || sampleRate <= 0) return 0
  const limit = Math.min(length, Math.floor(maxSeconds * sampleRate))
  for (let i = 0; i < limit; i++) {
    for (const channel of channels) {
      if (Math.abs(channel[i]) > threshold) return i / sampleRate
    }
  }
  return limit / sampleRate
}

/** How long a clip can actually play: the asked length, or what is left of the buffer. */
export function playableSeconds(asked: number, bufferSeconds: number, startOffset: number): number {
  return Math.max(0, Math.min(asked, bufferSeconds - startOffset))
}

// 0.30000000000000004 must not reach the page.
function tidy(seconds: number): number {
  return Math.round(seconds * 10) / 10
}

/** "0.3 seconds", "1 second", "30 seconds". */
export function clipLabel(seconds: number): string {
  const value = tidy(seconds)
  return `${value} ${value === 1 ? 'second' : 'seconds'}`
}

/** "0.3 s". */
export function clipShort(seconds: number): string {
  return `${tidy(seconds)} s`
}

/**
 * Position of a time on a bar where every ladder step is equally wide, 0 to 1.
 *
 * On a linear 30 second bar the first three steps (0.1, 0.3, 1) would share
 * 3% of the width; equal steps keep each one visible.
 */
export function ladderFraction(seconds: number, ladder: readonly number[]): number {
  if (ladder.length === 0 || !(seconds > 0)) return 0
  let from = 0
  for (let i = 0; i < ladder.length; i++) {
    const to = ladder[i]
    if (seconds <= to) {
      const within = to > from ? (seconds - from) / (to - from) : 1
      return (i + within) / ladder.length
    }
    from = to
  }
  return 1
}
