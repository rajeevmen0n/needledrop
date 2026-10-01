// Cuts an MP3 at frame boundaries, the way the Rust server's `mp3.rs` does, so
// the mock serves the same kind of truncated stream the browser has to decode.
// MPEG-1 Layer III only, which is what Deezer previews are.

const BITRATES_KBPS = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320]
const SAMPLE_RATES = [44100, 48000, 32000]
const SAMPLES_PER_FRAME = 1152

/** Extra frames after the cut, as the server sends: bit reservoir and encoder delay. */
const PADDING_FRAMES = 4

export interface Mp3 {
  /** The clip covering `ms` milliseconds plus the padding frames. */
  prefix(ms: number): Buffer
  frames: number
  seconds: number
}

function id3v2Length(data: Buffer): number {
  if (data.length < 10 || data.toString('latin1', 0, 3) !== 'ID3') return 0
  // Four 7-bit bytes.
  const size = (data[6] << 21) | (data[7] << 14) | (data[8] << 7) | data[9]
  return 10 + size
}

/** Walks the frame headers. `null` when the data does not start with MPEG-1 Layer III frames. */
export function parseMp3(data: Buffer): Mp3 | null {
  const start = id3v2Length(data)
  // ends[i] is where frame i stops; the frames are contiguous from `start`.
  const ends: number[] = []
  let sampleRate = 0
  let pos = start
  while (pos + 4 <= data.length) {
    const sync = data[pos] === 0xff && (data[pos + 1] & 0xfe) === 0xfa
    const bitrate = BITRATES_KBPS[data[pos + 2] >> 4]
    const rate = SAMPLE_RATES[(data[pos + 2] >> 2) & 0x3]
    if (!sync || !bitrate || !rate) break
    const padding = (data[pos + 2] >> 1) & 0x1
    const length = Math.floor((144000 * bitrate) / rate) + padding
    if (pos + length > data.length) break
    pos += length
    ends.push(pos)
    sampleRate = rate
  }
  if (ends.length === 0) return null

  return {
    frames: ends.length,
    seconds: (ends.length * SAMPLES_PER_FRAME) / sampleRate,
    prefix(ms) {
      const wanted = Math.ceil((ms * sampleRate) / 1000 / SAMPLES_PER_FRAME) + PADDING_FRAMES
      const count = Math.min(Math.max(wanted, 1), ends.length)
      return data.subarray(start, ends[count - 1])
    },
  }
}
