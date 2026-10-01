//! MP3 frame walking: cut a clip of a given duration from a preview by slicing at frame boundaries.
//!
//! The server must never send more audio than the player has unlocked, and it
//! must do so without transcoding. An MP3 stream is a sequence of
//! self-describing frames, so the first N frames are themselves a playable
//! file. [`Mp3::parse`] finds the frame boundaries once per song and
//! [`Mp3::prefix`] hands out the leading frames for a clip length.
//!
//! Nothing here touches the network or the disk.

use bytes::Bytes;

/// Extra frames [`Mp3::prefix`] sends beyond the requested duration.
///
/// Two things eat into the start of a decoded MP3. The decoder outputs about
/// 50 ms of priming silence (encoder delay plus its own delay, roughly two
/// frames) before the first real sample, and the client skips that. And a
/// Layer III frame may keep part of its data in the *following* frames' slack
/// space (the bit reservoir), so the last frame or two of a hard cut can decode
/// incompletely. Four frames (~104 ms at 44.1 kHz) cover both, so the browser
/// still has the full requested duration of real audio to trim to.
pub const PADDING_FRAMES: usize = 4;

/// Why a byte buffer could not be used as an MP3 stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Mp3Error {
    /// The buffer has no bytes at all.
    #[error("the MP3 data is empty")]
    Empty,
    /// No run of valid MPEG Layer III frames was found.
    #[error("no MPEG Layer III audio frames found")]
    NoAudioFrames,
}

/// A parsed MP3 stream: the audio frames only, plus where each one ends.
///
/// Cloning is cheap; the audio is reference-counted.
#[derive(Debug, Clone)]
pub struct Mp3 {
    /// The audio frames, back to back: no ID3 tags, no Xing/Info header frame,
    /// no trailing partial frame.
    audio: Bytes,
    /// End offset of each frame within `audio`. Never empty, strictly
    /// increasing, and the last entry equals `audio.len()`.
    frame_ends: Vec<usize>,
    sample_rate: u32,
    samples_per_frame: u32,
}

impl Mp3 {
    /// Finds the audio frames in `data`. Meant to run once per song.
    ///
    /// Skips leading ID3v2 tags, then walks the frame headers. The walk stops
    /// at the first thing that is not a complete frame of the same kind, which
    /// covers an ID3v1 `TAG` trailer, trailing garbage and a truncated last
    /// frame. A Xing/Info/VBRI header frame at the front is left out: it
    /// declares the length of the whole stream, and decoders trust it over
    /// the truncated data that follows.
    ///
    /// The first frame is only accepted when another valid frame (or the end
    /// of the data) follows it, so a stray `0xFF` in junk before the audio is
    /// not mistaken for a frame.
    pub fn parse(data: impl Into<Bytes>) -> Result<Self, Mp3Error> {
        let data: Bytes = data.into();
        if data.is_empty() {
            return Err(Mp3Error::Empty);
        }

        let (first_start, first) =
            find_first_frame(&data, skip_id3v2(&data)).ok_or(Mp3Error::NoAudioFrames)?;

        // Frame boundaries as absolute offsets; `starts[i]..ends[i]` is frame `i`.
        let mut ends = Vec::new();
        let mut pos = first_start;
        while let Some(header) = FrameHeader::parse(&data[pos..]) {
            let end = pos + header.len;
            // A frame of another kind is not part of this stream, and a frame
            // that runs past the end of the data is a truncated download.
            if !header.same_stream(&first) || end > data.len() {
                break;
            }
            ends.push(end);
            pos = end;
        }

        let mut audio_start = first_start;
        if !ends.is_empty() && first.is_vbr_header(&data[first_start..first_start + first.len]) {
            audio_start = ends.remove(0);
        }
        let audio_end = *ends.last().ok_or(Mp3Error::NoAudioFrames)?;

        Ok(Self {
            audio: data.slice(audio_start..audio_end),
            frame_ends: ends.into_iter().map(|end| end - audio_start).collect(),
            sample_rate: first.sample_rate,
            samples_per_frame: first.samples_per_frame,
        })
    }

    /// Number of audio frames.
    pub fn frame_count(&self) -> usize {
        self.frame_ends.len()
    }

    /// Sample rate in Hz (44 100 for Deezer previews).
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Samples each frame decodes to: 1152 for MPEG-1, 576 for MPEG-2 and 2.5.
    #[cfg(test)]
    pub fn samples_per_frame(&self) -> u32 {
        self.samples_per_frame
    }

    /// Length of the audio in milliseconds, rounded down.
    pub fn duration_ms(&self) -> u32 {
        let samples = self.frame_count() as u64 * u64::from(self.samples_per_frame);
        u32::try_from(samples * 1000 / u64::from(self.sample_rate)).unwrap_or(u32::MAX)
    }

    /// Every audio frame, without tags. Same as a [`prefix`](Self::prefix)
    /// longer than the stream.
    #[cfg(test)]
    pub fn audio(&self) -> Bytes {
        self.audio.clone()
    }

    /// How many frames [`prefix`](Self::prefix) returns for a clip of `ms`
    /// milliseconds: enough frames to hold that many samples, plus
    /// [`PADDING_FRAMES`], capped at the whole stream.
    pub fn prefix_frames(&self, ms: u32) -> usize {
        let samples = (u64::from(ms) * u64::from(self.sample_rate)).div_ceil(1000);
        let frames = samples.div_ceil(u64::from(self.samples_per_frame));
        usize::try_from(frames)
            .unwrap_or(usize::MAX)
            .saturating_add(PADDING_FRAMES)
            .min(self.frame_count())
    }

    /// The leading frames covering the first `ms` milliseconds, as a
    /// standalone MP3 stream. No copy is made.
    ///
    /// The cut is frame-accurate and slightly generous (see
    /// [`PADDING_FRAMES`]); the client trims to the exact duration after
    /// decoding. It always starts at the first audio frame and never contains
    /// an ID3 tag or any other metadata block: a tag can carry the song
    /// title, which would hand the player the answer.
    ///
    /// A request at or beyond the stream's length returns every frame.
    pub fn prefix(&self, ms: u32) -> Bytes {
        match self.prefix_frames(ms).checked_sub(1) {
            Some(last) => self.audio.slice(..self.frame_ends[last]),
            // Unreachable while `frame_ends` is non-empty, which `parse` guarantees.
            None => Bytes::new(),
        }
    }
}

/// The fields of a 4-byte MPEG audio frame header that slicing needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameHeader {
    version: Version,
    sample_rate: u32,
    samples_per_frame: u32,
    /// Length of the whole frame in bytes, header included.
    len: usize,
    mono: bool,
    /// A 16-bit CRC follows the header.
    crc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    Mpeg1,
    Mpeg2,
    /// The unofficial low-sample-rate extension of MPEG-2.
    Mpeg25,
}

/// Layer III bitrates in kbps by header index, MPEG-1. Index 0 is "free
/// format" and 15 is invalid; both are rejected.
const BITRATES_V1: [u32; 15] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];
/// Layer III bitrates in kbps by header index, MPEG-2 and MPEG-2.5.
const BITRATES_V2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

impl FrameHeader {
    /// Parses the header at the start of `bytes`. `None` when it is not a
    /// usable Layer III header: no sync word, another layer, a free-format
    /// bitrate, or any reserved value.
    fn parse(bytes: &[u8]) -> Option<Self> {
        let &[b0, b1, b2, b3, ..] = bytes else {
            return None;
        };
        // Sync word: eleven set bits.
        if b0 != 0xFF || b1 & 0xE0 != 0xE0 {
            return None;
        }
        let version = match (b1 >> 3) & 0b11 {
            0b00 => Version::Mpeg25,
            0b10 => Version::Mpeg2,
            0b11 => Version::Mpeg1,
            _ => return None,
        };
        // Layer III only; Layers I and II have other frame-size formulas and
        // are not what any preview is encoded in.
        if (b1 >> 1) & 0b11 != 0b01 {
            return None;
        }
        let bitrate_index = usize::from(b2 >> 4);
        let bitrate_kbps = match version {
            Version::Mpeg1 => BITRATES_V1.get(bitrate_index),
            Version::Mpeg2 | Version::Mpeg25 => BITRATES_V2.get(bitrate_index),
        }
        .copied()
        .filter(|&kbps| kbps != 0)?;
        let base_rate = match (b2 >> 2) & 0b11 {
            0 => 44_100,
            1 => 48_000,
            2 => 32_000,
            _ => return None,
        };
        // Emphasis value 2 is reserved.
        if b3 & 0b11 == 0b10 {
            return None;
        }
        let padding = usize::from((b2 >> 1) & 1);

        // MPEG-2 and 2.5 halve and quarter the sample rates and carry one
        // granule (576 samples) per frame instead of two, which also halves
        // the bytes-per-frame coefficient: samples_per_frame / 8.
        let (sample_rate, samples_per_frame) = match version {
            Version::Mpeg1 => (base_rate, 1152),
            Version::Mpeg2 => (base_rate / 2, 576),
            Version::Mpeg25 => (base_rate / 4, 576),
        };
        let len = (samples_per_frame / 8 * bitrate_kbps * 1000 / sample_rate) as usize + padding;

        Some(Self {
            version,
            sample_rate,
            samples_per_frame,
            len,
            mono: b3 >> 6 == 0b11,
            crc: b1 & 1 == 0,
        })
    }

    /// Whether `other` can follow `self` in one stream. The bitrate may change
    /// from frame to frame (VBR); the version and sample rate may not, and the
    /// duration maths relies on that.
    fn same_stream(&self, other: &Self) -> bool {
        self.version == other.version && self.sample_rate == other.sample_rate
    }

    /// Whether `frame` (this header's whole frame) is a Xing, Info or VBRI
    /// header frame: a silent frame whose payload describes the full stream.
    fn is_vbr_header(&self, frame: &[u8]) -> bool {
        /// VBRI (Fraunhofer) is always 32 bytes after the header.
        const VBRI_AT: usize = 4 + 32;

        // Xing/Info sits right after the side information, whose size depends
        // on the version and channel count.
        let side_info = match (self.version, self.mono) {
            (Version::Mpeg1, false) => 32,
            (Version::Mpeg1, true) | (Version::Mpeg2 | Version::Mpeg25, false) => 17,
            (Version::Mpeg2 | Version::Mpeg25, true) => 9,
        };
        let xing_at = 4 + if self.crc { 2 } else { 0 } + side_info;

        let tag_at = |at: usize| frame.get(at..at + 4);
        matches!(tag_at(xing_at), Some(b"Xing" | b"Info"))
            || matches!(tag_at(VBRI_AT), Some(b"VBRI"))
    }
}

/// Offset of the first byte after any ID3v2 tags at the start of `data`.
fn skip_id3v2(data: &[u8]) -> usize {
    let mut pos = 0;
    // A file can carry more than one tag in a row, so keep going.
    while let Some(&[b'I', b'D', b'3', major, revision, flags, s0, s1, s2, s3]) =
        data.get(pos..pos + 10)
    {
        // Version bytes are never 0xFF and size bytes never use their top bit;
        // anything else only happens to start with "ID3".
        if major == 0xFF || revision == 0xFF || (s0 | s1 | s2 | s3) & 0x80 != 0 {
            break;
        }
        // "Synchsafe": four bytes of seven bits each, so a tag header can
        // never contain a frame sync.
        let body = [s0, s1, s2, s3]
            .iter()
            .fold(0usize, |size, &byte| (size << 7) | usize::from(byte));
        // Flag bit 4 announces a 10-byte footer, which the size leaves out.
        let footer = if flags & 0x10 != 0 { 10 } else { 0 };
        pos += 10 + body + footer;
    }
    pos.min(data.len())
}

/// Finds the first believable frame at or after `from`: a complete frame
/// that is followed by a frame of the same stream, by an ID3v1 trailer, or by
/// nothing.
fn find_first_frame(data: &[u8], from: usize) -> Option<(usize, FrameHeader)> {
    (from..data.len()).find_map(|start| {
        let header = FrameHeader::parse(&data[start..])?;
        let rest = data.get(start + header.len..)?;
        let confirmed = rest.is_empty()
            || is_id3v1(rest)
            || FrameHeader::parse(rest).is_some_and(|next| next.same_stream(&header));
        confirmed.then_some((start, header))
    })
}

/// Whether `rest` is exactly an ID3v1 trailer: 128 bytes starting with `TAG`.
fn is_id3v1(rest: &[u8]) -> bool {
    rest.len() == 128 && rest.starts_with(b"TAG")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::LADDER_MS;

    /// MPEG-1 Layer III, 128 kbps, 44.1 kHz, joint stereo, no CRC: what Deezer serves.
    const V1_128K: [u8; 4] = [0xFF, 0xFB, 0x90, 0x64];
    /// The same with the padding bit set.
    const V1_128K_PADDED: [u8; 4] = [0xFF, 0xFB, 0x92, 0x64];
    /// MPEG-2 Layer III, 64 kbps, 22.05 kHz.
    const V2_64K: [u8; 4] = [0xFF, 0xF3, 0x80, 0x64];
    /// MPEG-2.5 Layer III, 8 kbps, 8 kHz, mono.
    const V25_8K_MONO: [u8; 4] = [0xFF, 0xE3, 0x18, 0xC4];

    const TITLE: &[u8] = b"Bohemian Rhapsody";

    /// One frame: `header` followed by filler up to `len` bytes. The length is
    /// passed in rather than computed so the tests do not lean on the code
    /// they check.
    fn frame(header: [u8; 4], len: usize, fill: u8) -> Vec<u8> {
        let mut bytes = header.to_vec();
        bytes.resize(len, fill);
        bytes
    }

    /// `count` frames shaped like a Deezer preview: 418 bytes with padding,
    /// 417 for every 25th frame, the filler byte counting up so frames differ.
    fn stream(count: usize) -> Vec<u8> {
        (0..count)
            .flat_map(|i| {
                let fill = (i % 200) as u8;
                if i % 25 == 24 {
                    frame(V1_128K, 417, fill)
                } else {
                    frame(V1_128K_PADDED, 418, fill)
                }
            })
            .collect()
    }

    /// Byte length of `stream(count)`.
    fn stream_len(count: usize) -> usize {
        count * 418 - count / 25
    }

    /// An ID3v2.4 tag with the given body, the size written synchsafe.
    fn id3v2(body: &[u8], footer: bool) -> Vec<u8> {
        let size = body.len();
        assert!(size < 1 << 28);
        let mut tag = vec![b'I', b'D', b'3', 4, 0, if footer { 0x10 } else { 0 }];
        tag.extend([21, 14, 7, 0].map(|shift| ((size >> shift) & 0x7F) as u8));
        tag.extend_from_slice(body);
        if footer {
            tag.extend_from_slice(b"3DI\x04\x00\x10\x00\x00\x00\x00");
        }
        tag
    }

    /// A 128-byte ID3v1 trailer carrying the song title.
    fn id3v1() -> Vec<u8> {
        let mut tag = b"TAG".to_vec();
        tag.extend_from_slice(TITLE);
        tag.resize(128, 0);
        tag
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    fn is_sync(bytes: &[u8]) -> bool {
        bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0
    }

    /// Start offset of every frame in `mp3.audio`.
    fn frame_starts(mp3: &Mp3) -> impl Iterator<Item = usize> + '_ {
        std::iter::once(0).chain(mp3.frame_ends.iter().copied().take(mp3.frame_count() - 1))
    }

    // --- frame headers ---------------------------------------------------

    #[test]
    fn frame_length_mpeg1_128k_44k() {
        let plain = FrameHeader::parse(&V1_128K).unwrap();
        assert_eq!(plain.len, 417);
        assert_eq!(plain.sample_rate, 44_100);
        assert_eq!(plain.samples_per_frame, 1152);
        assert_eq!(plain.version, Version::Mpeg1);
        assert!(!plain.mono);
        assert!(!plain.crc);

        assert_eq!(FrameHeader::parse(&V1_128K_PADDED).unwrap().len, 418);
    }

    #[test]
    fn frame_length_other_mpeg1_rates() {
        // 320 kbps at 48 kHz: 144 * 320000 / 48000.
        assert_eq!(
            FrameHeader::parse(&[0xFF, 0xFB, 0xE4, 0x64]).unwrap().len,
            960
        );
        // 128 kbps at 32 kHz: 144 * 128000 / 32000.
        let h = FrameHeader::parse(&[0xFF, 0xFB, 0x98, 0x64]).unwrap();
        assert_eq!((h.len, h.sample_rate), (576, 32_000));
        // CRC-protected frames (protection bit clear) have the same length.
        let h = FrameHeader::parse(&[0xFF, 0xFA, 0x90, 0x64]).unwrap();
        assert_eq!((h.len, h.crc), (417, true));
    }

    #[test]
    fn frame_length_mpeg2_and_mpeg25() {
        // MPEG-2: 72 * 64000 / 22050 = 208.98, rounded down.
        let v2 = FrameHeader::parse(&V2_64K).unwrap();
        assert_eq!(v2.version, Version::Mpeg2);
        assert_eq!(
            (v2.len, v2.sample_rate, v2.samples_per_frame),
            (208, 22_050, 576)
        );
        // With padding.
        assert_eq!(
            FrameHeader::parse(&[0xFF, 0xF3, 0x82, 0x64]).unwrap().len,
            209
        );
        // MPEG-2 at 24 kHz, 160 kbps (the top of its table): 72 * 160000 / 24000.
        let h = FrameHeader::parse(&[0xFF, 0xF3, 0xE4, 0x64]).unwrap();
        assert_eq!((h.len, h.sample_rate), (480, 24_000));

        // MPEG-2.5: 72 * 8000 / 8000.
        let v25 = FrameHeader::parse(&V25_8K_MONO).unwrap();
        assert_eq!(v25.version, Version::Mpeg25);
        assert_eq!(
            (v25.len, v25.sample_rate, v25.samples_per_frame),
            (72, 8_000, 576)
        );
        assert!(v25.mono);
        // MPEG-2.5 at 11.025 kHz, 32 kbps: 72 * 32000 / 11025 = 208.98.
        let h = FrameHeader::parse(&[0xFF, 0xE3, 0x40, 0x64]).unwrap();
        assert_eq!((h.len, h.sample_rate), (208, 11_025));
    }

    #[test]
    fn invalid_headers_are_rejected() {
        let cases: [(&str, [u8; 4]); 9] = [
            ("no sync", [0xFE, 0xFB, 0x90, 0x64]),
            ("short sync", [0xFF, 0xDB, 0x90, 0x64]),
            ("reserved version", [0xFF, 0xEB, 0x90, 0x64]),
            ("reserved layer", [0xFF, 0xF9, 0x90, 0x64]),
            ("layer II", [0xFF, 0xFD, 0x90, 0x64]),
            ("layer I", [0xFF, 0xFF, 0x90, 0x64]),
            ("free-format bitrate", [0xFF, 0xFB, 0x00, 0x64]),
            ("invalid bitrate", [0xFF, 0xFB, 0xF0, 0x64]),
            ("reserved sample rate", [0xFF, 0xFB, 0x9C, 0x64]),
        ];
        for (what, header) in cases {
            assert_eq!(FrameHeader::parse(&header), None, "{what}");
        }
        assert_eq!(
            FrameHeader::parse(&[0xFF, 0xFB, 0x90, 0x66]),
            None,
            "reserved emphasis"
        );
        assert_eq!(FrameHeader::parse(&[0xFF, 0xFB, 0x90]), None, "too short");
    }

    // --- parsing ---------------------------------------------------------

    #[test]
    fn parses_a_bare_stream() {
        let mp3 = Mp3::parse(stream(50)).unwrap();
        assert_eq!(mp3.frame_count(), 50);
        assert_eq!(mp3.sample_rate(), 44_100);
        assert_eq!(mp3.samples_per_frame(), 1152);
        assert_eq!(mp3.audio().len(), stream_len(50));
        // 50 * 1152 / 44100 s = 1306.1 ms.
        assert_eq!(mp3.duration_ms(), 1306);
        // 24 padded frames, one unpadded, 25 padded.
        assert_eq!(mp3.frame_ends[0], 418);
        assert_eq!(mp3.frame_ends[24], 24 * 418 + 417);
        assert_eq!(mp3.frame_ends[49], 50 * 418 - 2);
    }

    #[test]
    fn parses_mpeg2_and_mpeg25_streams() {
        let data: Vec<u8> = (0..10).flat_map(|_| frame(V2_64K, 208, 0x11)).collect();
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(
            (
                mp3.frame_count(),
                mp3.sample_rate(),
                mp3.samples_per_frame()
            ),
            (10, 22_050, 576)
        );
        // 10 * 576 / 22050 s.
        assert_eq!(mp3.duration_ms(), 261);
        // 100 ms is 2205 samples, 4 frames; plus padding.
        assert_eq!(mp3.prefix_frames(100), 8);
        assert_eq!(mp3.prefix(100).len(), 8 * 208);

        let data: Vec<u8> = (0..10).flat_map(|_| frame(V25_8K_MONO, 72, 0x11)).collect();
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(
            (mp3.frame_count(), mp3.sample_rate(), mp3.duration_ms()),
            (10, 8_000, 720)
        );
    }

    #[test]
    fn variable_bitrate_frames_are_walked_by_their_own_length() {
        // 128 kbps, then 320 kbps (1044 bytes at 44.1 kHz), then 128 again.
        let mut data = frame(V1_128K, 417, 1);
        data.extend(frame([0xFF, 0xFB, 0xE0, 0x64], 1044, 2));
        data.extend(frame(V1_128K, 417, 3));
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_ends, [417, 417 + 1044, 417 + 1044 + 417]);
    }

    #[test]
    fn sync_bytes_inside_a_frame_are_not_frame_starts() {
        // Payload made of header-like bytes: the walker must go by length.
        let mut data = Vec::new();
        for _ in 0..20 {
            let mut f = V1_128K.to_vec();
            while f.len() < 417 {
                f.extend_from_slice(&V1_128K_PADDED);
            }
            f.truncate(417);
            data.extend(f);
        }
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 20);
        assert!(mp3.frame_ends.iter().all(|end| end % 417 == 0));
    }

    #[test]
    fn skips_an_id3v2_tag() {
        // An empty tag: just the 10-byte header, as on Deezer previews.
        let mut data = id3v2(&[], false);
        assert_eq!(data.len(), 10);
        data.extend(stream(30));
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert_eq!(mp3.audio().len(), stream_len(30));
        assert_eq!(&mp3.audio()[..4], &V1_128K_PADDED);
    }

    #[test]
    fn skips_an_id3v2_tag_with_a_multi_byte_synchsafe_size() {
        // 300 bytes needs two synchsafe digits: 0x02 0x2C. Read as a plain
        // big-endian number that would be 556.
        let mut body = b"TIT2".to_vec();
        body.extend_from_slice(TITLE);
        body.resize(300, 0);
        let tag = id3v2(&body, false);
        assert_eq!(&tag[6..10], &[0, 0, 0x02, 0x2C]);

        let mut data = tag;
        data.extend(stream(30));
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert_eq!(mp3.audio().len(), stream_len(30));
        assert!(!contains(&mp3.audio(), TITLE));
        assert!(!contains(&mp3.audio(), b"ID3"));
    }

    #[test]
    fn id3v2_body_that_looks_like_audio_is_still_skipped() {
        // Album art is arbitrary binary. Put two complete, valid frames in the
        // tag body: a scan for sync words would start the audio there.
        let mut body = frame(V1_128K, 417, 0xAA);
        body.extend(frame(V1_128K, 417, 0xAA));
        body.extend_from_slice(&[0xFF, 0xFB]);
        let mut data = id3v2(&body, false);
        let tag_len = data.len();
        data.extend(stream(30));
        let total = data.len();

        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert_eq!(mp3.audio().len(), total - tag_len);
        assert!(!contains(&mp3.audio(), &[0xAA]));
    }

    #[test]
    fn honours_the_id3v2_footer_flag() {
        let mut data = id3v2(&[0x55; 40], true);
        assert_eq!(data.len(), 10 + 40 + 10);
        data.extend(stream(30));
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert_eq!(mp3.audio().len(), stream_len(30));
        assert!(!contains(&mp3.audio(), b"3DI"));
    }

    #[test]
    fn skips_consecutive_id3v2_tags() {
        let mut data = id3v2(&[0x55; 40], false);
        data.extend(id3v2(TITLE, false));
        data.extend(stream(30));
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert!(!contains(&mp3.audio(), TITLE));
    }

    #[test]
    fn id3v2_tag_longer_than_the_data_is_an_error() {
        let mut data = id3v2(&[0x55; 40], false);
        data[9] = 0x7F; // claims 127 bytes of body
        assert_eq!(Mp3::parse(data).unwrap_err(), Mp3Error::NoAudioFrames);
    }

    #[test]
    fn junk_before_the_first_frame_is_skipped() {
        // Includes a lone 0xFF and a header-shaped false sync with nothing
        // valid after it.
        let mut data = vec![0x00, 0xFF, 0x00, 0x00];
        data.extend_from_slice(&V1_128K);
        data.extend_from_slice(&[0x00; 30]);
        let junk_len = data.len();
        data.extend(stream(30));
        let total = data.len();

        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert_eq!(mp3.audio().len(), total - junk_len);
        assert_eq!(&mp3.audio()[..4], &V1_128K_PADDED);
    }

    #[test]
    fn id3v1_trailer_is_ignored() {
        let mut data = stream(30);
        data.extend(id3v1());
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert_eq!(mp3.audio().len(), stream_len(30));
        assert!(!contains(&mp3.audio(), b"TAG"));
        assert!(!contains(&mp3.audio(), TITLE));
    }

    #[test]
    fn trailing_garbage_is_ignored() {
        let mut data = stream(30);
        data.extend_from_slice(b"APETAGEX\x00\x00 not audio at all \xFF\xFF\xFF");
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert_eq!(mp3.audio().len(), stream_len(30));
        assert!(!contains(&mp3.audio(), b"APETAG"));
    }

    #[test]
    fn truncated_last_frame_is_dropped() {
        let mut data = stream(30);
        // A 31st frame cut short after 100 bytes.
        data.extend_from_slice(&frame(V1_128K_PADDED, 418, 0x77)[..100]);
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert_eq!(mp3.audio().len(), stream_len(30));
        assert!(!contains(&mp3.audio(), &[0x77]));

        // Cut inside the header itself.
        let mut data = stream(30);
        data.extend_from_slice(&V1_128K[..3]);
        assert_eq!(Mp3::parse(data).unwrap().frame_count(), 30);
    }

    #[test]
    fn a_frame_of_another_stream_ends_the_walk() {
        let mut data = stream(30);
        data.extend(frame(V2_64K, 208, 0x33));
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert_eq!(mp3.sample_rate(), 44_100);
    }

    #[test]
    fn single_frame_is_accepted_at_end_of_data_or_before_id3v1() {
        assert_eq!(Mp3::parse(frame(V1_128K, 417, 0)).unwrap().frame_count(), 1);

        let mut data = frame(V1_128K, 417, 0);
        data.extend(id3v1());
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!((mp3.frame_count(), mp3.audio().len()), (1, 417));
    }

    #[test]
    fn xing_and_info_header_frames_are_excluded() {
        for magic in [b"Xing", b"Info"] {
            // MPEG-1 stereo: 4 header bytes + 32 bytes of side information.
            let mut header_frame = frame(V1_128K, 417, 0);
            header_frame[36..40].copy_from_slice(magic);
            header_frame[40..57].copy_from_slice(TITLE);

            let mut data = id3v2(&[], false);
            data.extend(header_frame);
            data.extend(stream(30));

            let mp3 = Mp3::parse(data).unwrap();
            assert_eq!(mp3.frame_count(), 30);
            assert_eq!(mp3.audio().len(), stream_len(30));
            assert_eq!(*mp3.frame_ends.last().unwrap(), stream_len(30));
            assert!(!contains(&mp3.audio(), magic));
            assert!(!contains(&mp3.audio(), TITLE));
            assert_eq!(&mp3.prefix(100)[..4], &V1_128K_PADDED);
        }
    }

    #[test]
    fn xing_offset_depends_on_version_and_channels() {
        // MPEG-1 mono: 17 bytes of side information.
        let mono = [0xFF, 0xFB, 0x90, 0xC4];
        let mut header_frame = frame(mono, 417, 0);
        header_frame[21..25].copy_from_slice(b"Xing");
        let mut data = header_frame;
        data.extend((0..5).flat_map(|_| frame(mono, 417, 1)));
        assert_eq!(Mp3::parse(data).unwrap().frame_count(), 5);

        // MPEG-2 stereo: 17 bytes.
        let mut header_frame = frame(V2_64K, 208, 0);
        header_frame[21..25].copy_from_slice(b"Info");
        let mut data = header_frame;
        data.extend((0..5).flat_map(|_| frame(V2_64K, 208, 1)));
        assert_eq!(Mp3::parse(data).unwrap().frame_count(), 5);

        // MPEG-2.5 mono: 9 bytes.
        let mut header_frame = frame(V25_8K_MONO, 72, 0);
        header_frame[13..17].copy_from_slice(b"Xing");
        let mut data = header_frame;
        data.extend((0..5).flat_map(|_| frame(V25_8K_MONO, 72, 1)));
        assert_eq!(Mp3::parse(data).unwrap().frame_count(), 5);

        // With a CRC the side information starts two bytes later.
        let crc = [0xFF, 0xFA, 0x90, 0x64];
        let mut header_frame = frame(crc, 417, 0);
        header_frame[38..42].copy_from_slice(b"Info");
        let mut data = header_frame;
        data.extend((0..5).flat_map(|_| frame(crc, 417, 1)));
        assert_eq!(Mp3::parse(data).unwrap().frame_count(), 5);
    }

    #[test]
    fn vbri_header_frame_is_excluded() {
        let mut header_frame = frame(V1_128K, 417, 0);
        header_frame[36..40].copy_from_slice(b"VBRI");
        let mut data = header_frame;
        data.extend(stream(30));
        let mp3 = Mp3::parse(data).unwrap();
        assert_eq!(mp3.frame_count(), 30);
        assert!(!contains(&mp3.audio(), b"VBRI"));
    }

    #[test]
    fn xing_marker_in_a_later_frame_is_audio() {
        // Only the first frame can be a header frame.
        let mut data = stream(3);
        let mut odd = frame(V1_128K, 417, 0);
        odd[36..40].copy_from_slice(b"Xing");
        data.extend(odd);
        data.extend(stream(3));
        assert_eq!(Mp3::parse(data).unwrap().frame_count(), 7);
    }

    #[test]
    fn a_header_frame_alone_is_not_audio() {
        let mut header_frame = frame(V1_128K, 417, 0);
        header_frame[36..40].copy_from_slice(b"Info");
        assert_eq!(
            Mp3::parse(header_frame).unwrap_err(),
            Mp3Error::NoAudioFrames
        );
    }

    #[test]
    fn empty_and_garbage_input_are_errors() {
        assert_eq!(Mp3::parse(Vec::new()).unwrap_err(), Mp3Error::Empty);
        assert_eq!(Mp3::parse(&b""[..]).unwrap_err(), Mp3Error::Empty);

        let garbage: [&[u8]; 8] = [
            b"\x00",
            b"\xFF",
            b"\xFF\xFB\x90",
            b"<!DOCTYPE html><html><body>403 Forbidden</body></html>",
            &[0x00; 4096],
            &[0xFF; 4096],
            // A valid header with nothing behind it.
            &V1_128K,
            // Only a tag.
            b"ID3\x04\x00\x00\x00\x00\x00\x00",
        ];
        for bytes in garbage {
            assert_eq!(
                Mp3::parse(bytes).unwrap_err(),
                Mp3Error::NoAudioFrames,
                "{:?}",
                &bytes[..bytes.len().min(16)]
            );
        }

        // One frame followed by junk has nothing to confirm it.
        let mut data = frame(V1_128K, 417, 0);
        data.extend_from_slice(b"junk");
        assert_eq!(Mp3::parse(data).unwrap_err(), Mp3Error::NoAudioFrames);

        // Deterministic noise without a single pair of chained frames.
        let mut seed = 0x2545_F491u32;
        let noise: Vec<u8> = (0..20_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed >> 8) as u8
            })
            .collect();
        assert_eq!(Mp3::parse(noise).unwrap_err(), Mp3Error::NoAudioFrames);
    }

    // --- prefix ----------------------------------------------------------

    /// 1149 frames is the shortest stream holding 30 s at 44.1 kHz (30.015 s),
    /// wrapped in both kinds of tag, each carrying the title.
    fn tagged_thirty_seconds() -> (Mp3, usize) {
        let mut body = b"TIT2".to_vec();
        body.extend_from_slice(TITLE);
        let mut data = id3v2(&body, false);
        data.extend(stream(1149));
        data.extend(id3v1());
        let total = data.len();
        (Mp3::parse(data).unwrap(), total)
    }

    #[test]
    fn thirty_second_stream_has_the_expected_shape() {
        let (mp3, total) = tagged_thirty_seconds();
        assert_eq!(mp3.frame_count(), 1149);
        assert_eq!(mp3.duration_ms(), 30_014);
        assert_eq!(mp3.audio().len(), stream_len(1149));
        // What was dropped is exactly the two tags.
        assert_eq!(total - mp3.audio().len(), 10 + 4 + TITLE.len() + 128);
    }

    #[test]
    fn prefix_for_every_ladder_step() {
        let (mp3, _) = tagged_thirty_seconds();
        // ceil(ms * 44.1 / 1152) + 4, capped at 1149.
        let expected_frames = [8, 16, 43, 119, 311, 617, 1149];

        let mut previous = 0;
        for (ms, frames) in LADDER_MS.into_iter().zip(expected_frames) {
            let clip = mp3.prefix(ms);
            assert_eq!(mp3.prefix_frames(ms), frames, "{ms} ms");
            assert_eq!(clip.len(), stream_len(frames), "{ms} ms");
            assert_eq!(clip.len(), mp3.frame_ends[frames - 1], "{ms} ms");

            // It is the head of the audio, starting on a frame.
            assert!(is_sync(&clip), "{ms} ms");
            assert_eq!(&clip[..], &mp3.audio()[..clip.len()], "{ms} ms");
            // Walking it as a fresh file gives the same frames back, so the
            // cut is on a frame boundary.
            let reparsed = Mp3::parse(clip.clone()).unwrap();
            assert_eq!(reparsed.frame_count(), frames, "{ms} ms");
            assert_eq!(reparsed.audio().len(), clip.len(), "{ms} ms");

            // Enough real audio for the clip even after ~50 ms of priming delay.
            if frames < mp3.frame_count() {
                assert!(reparsed.duration_ms() >= ms + 50, "{ms} ms");
            }

            assert!(!contains(&clip, b"ID3"), "{ms} ms");
            assert!(!contains(&clip, b"TAG"), "{ms} ms");
            assert!(!contains(&clip, b"TIT2"), "{ms} ms");
            assert!(!contains(&clip, TITLE), "{ms} ms");

            assert!(clip.len() >= previous, "{ms} ms");
            previous = clip.len();
        }
    }

    #[test]
    fn shortest_clip_is_a_sliver_of_the_whole() {
        let (mp3, _) = tagged_thirty_seconds();
        let clip = mp3.prefix(100);
        assert_eq!(clip.len(), 8 * 418);
        // Under 1% of the song.
        assert!(clip.len() * 100 < mp3.audio().len());
    }

    #[test]
    fn prefix_grows_monotonically() {
        let (mp3, _) = tagged_thirty_seconds();
        let mut previous = 0;
        for ms in (0..=31_000).step_by(7) {
            let len = mp3.prefix(ms).len();
            assert!(len >= previous, "{ms} ms");
            previous = len;
        }
        assert_eq!(previous, mp3.audio().len());
    }

    #[test]
    fn prefix_frame_count_rounds_up() {
        let (mp3, _) = tagged_thirty_seconds();
        // Nothing asked for: only the padding.
        assert_eq!(mp3.prefix_frames(0), PADDING_FRAMES);
        // One sample's worth needs a whole frame.
        assert_eq!(mp3.prefix_frames(1), 1 + PADDING_FRAMES);
        // One frame is 26.12 ms.
        assert_eq!(mp3.prefix_frames(26), 1 + PADDING_FRAMES);
        assert_eq!(mp3.prefix_frames(27), 2 + PADDING_FRAMES);
    }

    #[test]
    fn prefix_beyond_the_duration_returns_everything() {
        let (mp3, _) = tagged_thirty_seconds();
        for ms in [mp3.duration_ms(), 30_015, 60_000, u32::MAX] {
            let clip = mp3.prefix(ms);
            assert_eq!(mp3.prefix_frames(ms), mp3.frame_count(), "{ms} ms");
            assert_eq!(clip, mp3.audio(), "{ms} ms");
            assert!(!contains(&clip, TITLE), "{ms} ms");
        }

        // A stream shorter than the padding.
        let short = Mp3::parse(stream(3)).unwrap();
        assert_eq!(short.prefix(0), short.audio());
        assert_eq!(short.prefix(100), short.audio());
    }

    #[test]
    fn prefix_shares_the_buffer() {
        let (mp3, _) = tagged_thirty_seconds();
        let whole = mp3.audio();
        let clip = mp3.prefix(100);
        assert_eq!(clip.as_ptr(), whole.as_ptr());
    }

    // --- a real preview ----------------------------------------------------

    /// Checks the walker against a real Deezer preview. The file is
    /// copyrighted audio and is git-ignored, so on a fresh checkout this test
    /// prints a note and passes. To get one, save any track's `preview` URL
    /// to `server/tests/fixtures/preview.mp3`.
    #[test]
    fn real_preview_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/preview.mp3");
        let Ok(file) = std::fs::read(path) else {
            eprintln!("note: {path} is missing; skipping the real-preview checks");
            return;
        };
        let file_len = file.len();
        let mp3 = Mp3::parse(file.clone()).unwrap();

        assert_eq!(mp3.sample_rate(), 44_100);
        assert_eq!(mp3.samples_per_frame(), 1152);
        assert!(
            (29_500..=30_500).contains(&mp3.duration_ms()),
            "duration {} ms",
            mp3.duration_ms()
        );

        // Every boundary is the start of a real frame.
        let audio = mp3.audio();
        for start in frame_starts(&mp3) {
            assert!(is_sync(&audio[start..]), "no sync at {start}");
            assert!(
                FrameHeader::parse(&audio[start..]).is_some(),
                "bad header at {start}"
            );
        }
        // CBR at 128 kbps: every frame is 417 or 418 bytes.
        let mut start = 0;
        for &end in &mp3.frame_ends {
            assert!(matches!(end - start, 417 | 418), "frame of {}", end - start);
            start = end;
        }

        // The file is a 10-byte empty ID3v2 tag, the frames, and at most one
        // partial frame where Deezer cut the preview by byte count.
        assert_eq!(&file[..3], b"ID3");
        assert_eq!(&audio[..], &file[10..10 + audio.len()]);
        let remainder = file_len - 10 - audio.len();
        assert!(remainder < 418, "{remainder} bytes after the last frame");
        if remainder >= 4 {
            let tail = FrameHeader::parse(&file[file_len - remainder..]);
            assert!(
                tail.is_some_and(|header| header.len > remainder),
                "the remainder is not a truncated frame"
            );
        }

        // The first clip is eight frames, a few kilobytes of a 480 kB file.
        assert_eq!(mp3.prefix_frames(100), 8);
        assert!(mp3.prefix(100).len() <= 8 * 418);

        let mut previous = 0;
        for ms in LADDER_MS {
            let clip = mp3.prefix(ms);
            assert!(is_sync(&clip));
            assert!(!contains(&clip, b"ID3"));
            assert!(clip.len() >= previous);
            assert_eq!(Mp3::parse(clip.clone()).unwrap().audio().len(), clip.len());
            previous = clip.len();
            println!(
                "prefix({ms}) = {} frames, {} bytes",
                mp3.prefix_frames(ms),
                clip.len()
            );
        }
        println!(
            "{} frames, {} ms, {} audio bytes of {file_len}, remainder {remainder}",
            mp3.frame_count(),
            mp3.duration_ms(),
            audio.len()
        );
    }
}
