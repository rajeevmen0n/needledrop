//! The current song: which track is played on a given UTC day, with its preview loaded and parsed.
//!
//! For now every day plays the one track named in the config. The routes only
//! ever ask [`Daily::song_for`] for "the song for date D", so the real daily
//! pick (one track per day from the playlist pool, with a history) can replace
//! [`Daily::pick`] without touching them.
//!
//! The preview and the track's metadata are cached on disk under
//! `<data_dir>/audio/`, so a restart does not go back to Deezer. During
//! development the server restarts on every source change.

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use jiff::civil::Date;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::{
    deezer::{Deezer, DeezerError, Track},
    game::TrackMeta,
    mp3::{Mp3, Mp3Error},
};

/// How long a failed load keeps further attempts away from Deezer. Every
/// request needs the song, so without this a Deezer outage would turn each
/// incoming request into an outgoing one.
const RETRY_AFTER: Duration = Duration::from_secs(10);

/// Today's date in UTC, the day the game is keyed to.
pub fn today_utc() -> Date {
    jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date()
}

/// What the player is shown once the game is over. Every field identifies the
/// song, so none of this may leave the server before then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Album cover URL, 500 px when Deezer has it.
    pub cover: String,
    /// The track's page on deezer.com.
    pub link: String,
}

/// A song ready to be played: its audio, what a guess is matched against, and
/// what is revealed at the end.
#[derive(Debug, Clone)]
pub struct Song {
    pub meta: TrackMeta,
    pub answer: Answer,
    pub mp3: Mp3,
}

impl Song {
    pub fn new(track: &Track, mp3: Mp3) -> Self {
        let album = &track.album;
        let cover = [&album.cover_big, &album.cover_medium, &album.cover_small]
            .into_iter()
            .find(|url| !url.is_empty())
            .cloned()
            .unwrap_or_default();
        let link = if track.link.is_empty() {
            format!("https://www.deezer.com/track/{}", track.id)
        } else {
            track.link.clone()
        };
        Self {
            meta: track.meta(),
            answer: Answer {
                title: track.title.clone(),
                artist: track.artist.name.clone(),
                album: album.title.clone(),
                cover,
                link,
            },
            mp3,
        }
    }
}

/// Why there is no song to play.
///
/// The messages name the track, so they are for the log only and must never
/// be sent to a client.
#[derive(Debug, thiserror::Error)]
pub enum DailyError {
    #[error("looking up track {track_id}: {source}")]
    Lookup {
        track_id: u64,
        #[source]
        source: DeezerError,
    },
    #[error("track {0} is not readable or has no preview")]
    Unplayable(u64),
    #[error("downloading the preview of track {track_id}: {source}")]
    Download {
        track_id: u64,
        #[source]
        source: DeezerError,
    },
    /// Deezer's CDN answered with something else, such as an error page for
    /// an expired URL.
    #[error("the preview of track {track_id} is not an MP3: {source}")]
    BadAudio {
        track_id: u64,
        #[source]
        source: Mp3Error,
    },
    #[error("the last attempt to load the song failed; trying again in {0:?}")]
    CoolingDown(Duration),
}

/// Holds the song being played and loads it when needed.
pub struct Daily {
    deezer: Deezer,
    /// `<data_dir>/audio`.
    audio_dir: PathBuf,
    track_id: u64,
    retry_after: Duration,
    /// Locked for the whole of a load, so that requests arriving together
    /// wait for one download instead of each starting their own.
    slot: Mutex<Slot>,
}

#[derive(Default)]
struct Slot {
    loaded: Option<Loaded>,
    failed_at: Option<Instant>,
}

struct Loaded {
    track_id: u64,
    song: Arc<Song>,
}

impl Daily {
    /// Nothing is loaded yet; the first [`song_for`](Self::song_for) does that.
    pub fn new(deezer: Deezer, data_dir: &Path, track_id: u64) -> Self {
        Self {
            deezer,
            audio_dir: data_dir.join("audio"),
            track_id,
            retry_after: RETRY_AFTER,
            slot: Mutex::new(Slot::default()),
        }
    }

    /// Overrides [`RETRY_AFTER`].
    #[cfg(test)]
    pub fn with_retry_after(mut self, retry_after: Duration) -> Self {
        self.retry_after = retry_after;
        self
    }

    /// The Deezer track ID played on `day`.
    ///
    /// This is where the daily pick will go. Until then it is the configured
    /// track, whatever the day.
    fn pick(&self, _day: Date) -> u64 {
        self.track_id
    }

    /// The song for `day`, loading it first if it is not in memory: from the
    /// disk cache when it is there, from Deezer otherwise.
    ///
    /// A failed load is not fatal. It is logged, the error is returned, and
    /// the next call after a short pause ([`RETRY_AFTER`]) tries again.
    pub async fn song_for(&self, day: Date) -> Result<Arc<Song>, DailyError> {
        let track_id = self.pick(day);
        let mut slot = self.slot.lock().await;

        if let Some(loaded) = &slot.loaded
            && loaded.track_id == track_id
        {
            return Ok(Arc::clone(&loaded.song));
        }
        if let Some(failed_at) = slot.failed_at {
            let waited = failed_at.elapsed();
            if waited < self.retry_after {
                return Err(DailyError::CoolingDown(self.retry_after - waited));
            }
        }

        match self.load(track_id).await {
            Ok(song) => {
                let song = Arc::new(song);
                *slot = Slot {
                    loaded: Some(Loaded {
                        track_id,
                        song: Arc::clone(&song),
                    }),
                    failed_at: None,
                };
                Ok(song)
            }
            Err(error) => {
                tracing::error!(%day, %error, "could not load the song; will retry on a later request");
                slot.failed_at = Some(Instant::now());
                Err(error)
            }
        }
    }

    async fn load(&self, track_id: u64) -> Result<Song, DailyError> {
        if let Some(song) = self.load_cached(track_id).await {
            log_loaded(track_id, &song, "the disk cache");
            return Ok(song);
        }

        // The preview URL in the track expires after about 15 minutes, so the
        // lookup and the download belong together; neither is kept.
        let track = self
            .deezer
            .track(track_id)
            .await
            .map_err(|source| DailyError::Lookup { track_id, source })?;
        if !track.is_playable() {
            return Err(DailyError::Unplayable(track_id));
        }
        let bytes = self
            .deezer
            .download_preview(&track.preview)
            .await
            .map_err(|source| DailyError::Download { track_id, source })?;
        // Parse before caching: a body that is not an MP3 is a failed
        // download, and writing it to disk would make the failure permanent.
        let mp3 = Mp3::parse(bytes.clone())
            .map_err(|source| DailyError::BadAudio { track_id, source })?;

        if let Err(error) = self.store(track_id, &track, &bytes).await {
            // The song is in memory, so the game works; only the next restart
            // has to download again.
            tracing::warn!(%error, dir = %self.audio_dir.display(), "could not cache the preview on disk");
        }
        let song = Song::new(&track, mp3);
        log_loaded(track_id, &song, "Deezer");
        Ok(song)
    }

    fn mp3_path(&self, track_id: u64) -> PathBuf {
        self.audio_dir.join(format!("{track_id}.mp3"))
    }

    fn json_path(&self, track_id: u64) -> PathBuf {
        self.audio_dir.join(format!("{track_id}.json"))
    }

    /// The song from `<audio_dir>/<id>.json` and `<id>.mp3`, or `None` when
    /// either is missing or unusable, in which case Deezer is asked instead.
    async fn load_cached(&self, track_id: u64) -> Option<Song> {
        let json = read_cache_file(&self.json_path(track_id)).await?;
        let audio = read_cache_file(&self.mp3_path(track_id)).await?;

        let track = serde_json::from_slice::<Track>(&json)
            .inspect_err(
                |error| tracing::warn!(%error, track_id, "ignoring an unreadable cached track"),
            )
            .ok()?;
        // A file for another track, or one with no title to reveal, is as
        // useless as a missing one.
        if track.id != track_id || track.title.trim().is_empty() {
            tracing::warn!(track_id, "ignoring a cached track that is not this track");
            return None;
        }
        let mp3 = Mp3::parse(audio)
            .inspect_err(
                |error| tracing::warn!(%error, track_id, "ignoring an unreadable cached preview"),
            )
            .ok()?;
        Some(Song::new(&track, mp3))
    }

    /// Writes both cache files. The audio goes first and the JSON last, and
    /// loading needs both, so an interrupted store leaves nothing that looks
    /// complete.
    async fn store(&self, track_id: u64, track: &Track, audio: &[u8]) -> io::Result<()> {
        tokio::fs::create_dir_all(&self.audio_dir).await?;
        // `Track` leaves its preview URL out when serialized.
        let json = serde_json::to_vec_pretty(track)?;
        write_atomically(&self.mp3_path(track_id), audio).await?;
        write_atomically(&self.json_path(track_id), &json).await
    }
}

/// The answer goes to the log, which only the operator reads. It is how the
/// developer knows what to guess.
fn log_loaded(track_id: u64, song: &Song, source: &str) {
    tracing::info!(
        track_id,
        title = %song.answer.title,
        artist = %song.answer.artist,
        frames = song.mp3.frame_count(),
        duration_ms = song.mp3.duration_ms(),
        sample_rate = song.mp3.sample_rate(),
        "loaded the song from {source}"
    );
}

/// The file's contents, or `None` if it cannot be read. A missing file is the
/// normal first-run case and is not worth a log line.
async fn read_cache_file(path: &Path) -> Option<Vec<u8>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(%error, path = %path.display(), "could not read a cache file");
            }
            None
        }
    }
}

/// Writes to a temporary file and renames it into place, so a server killed
/// mid-write (bacon does that on every source change) never leaves half a
/// file under the real name.
async fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    tokio::fs::write(&temporary, contents).await?;
    tokio::fs::rename(&temporary, path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{MockDeezer, MockTrack, Preview, synthetic_mp3};
    use jiff::civil::date;

    const DAY: Date = date(2026, 10, 1);
    const ID: u64 = 4242;

    fn answer_track() -> MockTrack {
        MockTrack::new(ID, "Zanzibar Nights (Remastered 2011)", "The Answers")
            .title_short("Zanzibar Nights")
            .album("Night Album")
    }

    async fn mock() -> MockDeezer {
        MockDeezer::start(vec![
            answer_track(),
            MockTrack::new(7, "Not A Song", "Nobody").preview(Preview::Html),
            MockTrack::new(8, "Withdrawn", "Nobody").preview(Preview::None),
        ])
        .await
    }

    #[tokio::test]
    async fn loads_from_deezer_and_caches_on_disk() {
        let deezer = mock().await;
        let dir = tempfile::tempdir().unwrap();
        let daily = Daily::new(deezer.client(), dir.path(), ID);

        let song = daily.song_for(DAY).await.unwrap();
        assert_eq!(song.answer.title, "Zanzibar Nights (Remastered 2011)");
        assert_eq!(song.answer.artist, "The Answers");
        assert_eq!(song.answer.album, "Night Album");
        assert_eq!(
            song.answer.link,
            format!("https://www.deezer.com/track/{ID}")
        );
        assert!(
            song.answer.cover.contains("500x500"),
            "{}",
            song.answer.cover
        );
        assert_eq!(song.meta.title_short, "Zanzibar Nights");
        assert_eq!(song.mp3.frame_count(), 1148);
        assert_eq!(deezer.api_hits(), 1);

        // In memory now: asking again, even for another day, costs nothing.
        daily.song_for(DAY).await.unwrap();
        daily.song_for(date(2026, 10, 2)).await.unwrap();
        assert_eq!(deezer.api_hits(), 1);

        let audio = std::fs::read(dir.path().join("audio/4242.mp3")).unwrap();
        assert_eq!(audio, synthetic_mp3(1148));
        let json = std::fs::read_to_string(dir.path().join("audio/4242.json")).unwrap();
        assert!(json.contains("Zanzibar Nights"), "{json}");
        // The signed preview URL is never stored.
        assert!(!json.contains("preview"), "{json}");
        assert!(!dir.path().join("audio/4242.mp3.tmp").exists());
    }

    #[tokio::test]
    async fn a_restart_loads_from_disk_without_deezer() {
        let deezer = mock().await;
        let dir = tempfile::tempdir().unwrap();
        let first = Daily::new(deezer.client(), dir.path(), ID);
        let loaded = first.song_for(DAY).await.unwrap();
        assert_eq!(deezer.api_hits(), 1);

        deezer.set_failing(true);
        let restarted = Daily::new(deezer.client(), dir.path(), ID);
        let song = restarted.song_for(DAY).await.unwrap();
        assert_eq!(deezer.api_hits(), 1);
        assert_eq!(song.answer, loaded.answer);
        assert_eq!(song.meta, loaded.meta);
        assert_eq!(song.mp3.audio(), loaded.mp3.audio());
    }

    #[tokio::test]
    async fn a_preview_that_is_not_an_mp3_fails_and_is_not_cached() {
        let deezer = mock().await;
        let dir = tempfile::tempdir().unwrap();
        let daily = Daily::new(deezer.client(), dir.path(), 7);

        let error = daily.song_for(DAY).await.unwrap_err();
        assert!(
            matches!(error, DailyError::BadAudio { track_id: 7, .. }),
            "{error}"
        );
        assert!(!dir.path().join("audio/7.mp3").exists());
        assert!(!dir.path().join("audio/7.json").exists());
    }

    #[tokio::test]
    async fn an_unplayable_or_unknown_track_is_an_error() {
        let deezer = mock().await;
        let dir = tempfile::tempdir().unwrap();

        let withdrawn = Daily::new(deezer.client(), dir.path(), 8);
        assert!(matches!(
            withdrawn.song_for(DAY).await,
            Err(DailyError::Unplayable(8))
        ));

        let unknown = Daily::new(deezer.client(), dir.path(), 999);
        assert!(matches!(
            unknown.song_for(DAY).await,
            Err(DailyError::Lookup {
                source: DeezerError::NotFound,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_failed_load_is_not_retried_at_once_but_is_retried_later() {
        let deezer = mock().await;
        let dir = tempfile::tempdir().unwrap();
        deezer.set_failing(true);

        let patient = Daily::new(deezer.client(), dir.path(), ID);
        assert!(matches!(
            patient.song_for(DAY).await,
            Err(DailyError::Lookup { .. })
        ));
        assert_eq!(deezer.api_hits(), 1);
        // Deezer is back, but the pause is not over: no request goes out.
        deezer.set_failing(false);
        assert!(matches!(
            patient.song_for(DAY).await,
            Err(DailyError::CoolingDown(_))
        ));
        assert_eq!(deezer.api_hits(), 1);

        // With no pause, the next call after a failure simply tries again.
        deezer.set_failing(true);
        let eager = Daily::new(deezer.client(), dir.path(), ID).with_retry_after(Duration::ZERO);
        assert!(eager.song_for(DAY).await.is_err());
        deezer.set_failing(false);
        let song = eager.song_for(DAY).await.unwrap();
        assert_eq!(song.answer.artist, "The Answers");
    }

    #[tokio::test]
    async fn a_corrupt_cache_is_replaced_from_deezer() {
        let deezer = mock().await;
        let dir = tempfile::tempdir().unwrap();
        let audio_dir = dir.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        std::fs::write(audio_dir.join("4242.mp3"), b"<html>expired</html>").unwrap();
        std::fs::write(
            audio_dir.join("4242.json"),
            br#"{"id":4242,"title":"Stale"}"#,
        )
        .unwrap();

        let daily = Daily::new(deezer.client(), dir.path(), ID);
        let song = daily.song_for(DAY).await.unwrap();
        assert_eq!(song.answer.title, "Zanzibar Nights (Remastered 2011)");
        assert_eq!(deezer.api_hits(), 1);
        assert_eq!(
            std::fs::read(audio_dir.join("4242.mp3")).unwrap(),
            synthetic_mp3(1148)
        );
    }

    #[tokio::test]
    async fn half_a_cache_is_no_cache() {
        let deezer = mock().await;
        let dir = tempfile::tempdir().unwrap();
        let audio_dir = dir.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        // The audio without its JSON: what an interrupted store leaves behind.
        std::fs::write(audio_dir.join("4242.mp3"), synthetic_mp3(1148)).unwrap();

        let daily = Daily::new(deezer.client(), dir.path(), ID);
        daily.song_for(DAY).await.unwrap();
        assert_eq!(deezer.api_hits(), 1);
        assert!(audio_dir.join("4242.json").exists());
    }

    #[test]
    fn the_answer_falls_back_to_smaller_covers_and_a_built_link() {
        let mp3 = Mp3::parse(synthetic_mp3(10)).unwrap();
        let mut track: Track = serde_json::from_value(answer_track().json("http://mock")).unwrap();

        track.album.cover_big.clear();
        assert!(
            Song::new(&track, mp3.clone())
                .answer
                .cover
                .contains("250x250")
        );
        track.album.cover_medium.clear();
        assert!(
            Song::new(&track, mp3.clone())
                .answer
                .cover
                .contains("56x56")
        );
        track.album.cover_small.clear();
        track.link.clear();
        let answer = Song::new(&track, mp3).answer;
        assert_eq!(answer.cover, "");
        assert_eq!(answer.link, "https://www.deezer.com/track/4242");
    }
}
