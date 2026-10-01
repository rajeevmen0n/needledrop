//! The day and its songs: which day it is for the server, which track each section plays on it, and that track's preview, loaded and parsed.
//!
//! **The day.** The server's day is the real UTC date plus an offset in days
//! that is kept in the store ([`Daily::today`]). The offset is the admin's
//! clock: "Simulate next day" adds one to it and "Reset to day 1" sets it so
//! that today is the launch date. It moves every player at once, exactly as
//! midnight does. Handlers read the day once per request and pass it down.
//!
//! **The picks.** Each of the four sections plays one song a day. Which one
//! is decided by [`crate::pick::choose`], lazily, the first time a section's
//! song for that day is asked for, and written to the store; from then on
//! the stored pick is the day's song, so a restart changes nothing. Before a
//! section is picked, the sections ahead of it in [`PICK_ORDER`] are, so that
//! General always knows what the genres play and takes something else.
//!
//! **The check.** A pick is only stored once its song has been loaded, which
//! is the check that the track has a preview. When Deezer says it has none
//! (not readable, no preview URL, a preview that is not an MP3, or no such
//! track at all) the song is marked as failed for the day and another is
//! drawn. When Deezer does not answer, or the request budget is spent,
//! nothing is concluded about the song: the pick fails, nothing is stored,
//! and a later request tries again after a short pause.
//!
//! **The audio.** The preview and the track's metadata are cached on disk
//! under `<data_dir>/audio/` by track ID, so a restart does not go back to
//! Deezer (during development the server restarts on every source change),
//! and a song that is there can be picked and played while Deezer is down.
//! The songs in use are also kept in memory, parsed.
//!
//! **Changes.** A re-roll, the next day and a reset change what "today's
//! song" is while players are in the middle of a request. [`Daily::stands`]
//! is how a handler makes sure the song it is about to use is still the one.

use std::{
    collections::{BTreeSet, HashMap},
    io,
    path::{Path, PathBuf},
    sync::{Arc, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use jiff::civil::Date;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock, RwLockReadGuard};

use crate::{
    deezer::{Deezer, DeezerError, Track},
    game::TrackMeta,
    mp3::{Mp3, Mp3Error},
    pick::{self, PICK_ORDER},
    store::{Pick, Section, Store, StoreError},
};

/// How long a failed load keeps further attempts away from Deezer. Every
/// request needs a song, so without this a Deezer outage would turn each
/// incoming request into an outgoing one.
const RETRY_AFTER: Duration = Duration::from_secs(10);

/// Parsed previews kept in memory, about half a megabyte each. A day needs
/// four; the rest is room for the songs a re-roll replaced and for the day
/// before, so that midnight and the admin page do not push a song out while
/// a request still wants it. The least recently used one goes first, and a
/// song that went is read back from the disk cache.
const SONGS_IN_MEMORY: usize = 8;

/// Today's date in UTC.
pub fn today_utc() -> Date {
    jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date()
}

/// Where the real date comes from. The server reads the wall clock; a test
/// hands in a date of its own, so that nothing depends on when it runs (or
/// on midnight passing in the middle of it).
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> Date + Send + Sync>);

impl Clock {
    /// The wall clock: today's date in UTC.
    pub fn utc() -> Self {
        Self::new(today_utc)
    }

    /// A clock that asks `real_day` for the date every time it is read.
    pub fn new(real_day: impl Fn() -> Date + Send + Sync + 'static) -> Self {
        Self(Arc::new(real_day))
    }

    /// The real date, before the day offset.
    pub fn real_day(&self) -> Date {
        (self.0)()
    }
}

/// The server's day and how it comes about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Days {
    /// The real UTC date.
    pub real: Date,
    /// Days added to it; negative when the server's day is behind.
    pub offset: i64,
    /// The day everything is keyed to: `real` plus `offset`.
    pub today: Date,
}

/// `real` moved by `offset` days, or `None` when that is outside the
/// calendar.
fn shifted(real: Date, offset: i64) -> Option<Date> {
    let span = jiff::Span::new().try_days(offset).ok()?;
    real.checked_add(span).ok()
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

/// The song a section plays on a day, loaded.
#[derive(Debug, Clone)]
pub struct Playing {
    pub day: Date,
    pub section: Section,
    /// The pick: the Deezer track ID.
    pub track_id: u64,
    pub song: Arc<Song>,
}

/// Proof that a [`Playing`] is still the section's song, for as long as it is
/// held: no re-roll, next day or reset can happen in the meantime.
#[must_use = "the song only stands while this is held"]
pub struct Standing<'a> {
    _held: RwLockReadGuard<'a, ()>,
}

/// Why there is no song to play.
///
/// The messages name tracks, so they are for the log only and must never be
/// sent to a client.
#[derive(Debug, thiserror::Error)]
pub enum DailyError {
    /// Nothing in the section's pool can be played that day: the pool is
    /// empty, or what is in it plays in another section or failed the
    /// preview check today.
    #[error("the {section} section has no song to play on {day}")]
    NoSong { day: Date, section: Section },
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
    #[error("the last request to Deezer failed; trying again in {0:?}")]
    CoolingDown(Duration),
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("a day offset of {0} days leads outside the calendar")]
    Clock(i64),
}

impl DailyError {
    /// Whether this is Deezer's verdict on the track itself: it has no
    /// preview to play, today. Deezer said the track is not readable or gave
    /// no preview URL, served something that is not an MP3 as the preview,
    /// or does not know the track at all.
    ///
    /// Only these make the pick skip a song. Everything else (Deezer did not
    /// answer, the request budget is spent, the download broke off) says
    /// nothing about the song, and skipping on it would burn through the pool
    /// during an outage.
    fn condemns_the_song(&self) -> bool {
        matches!(
            self,
            Self::Unplayable(_)
                | Self::BadAudio { .. }
                | Self::Lookup {
                    source: DeezerError::NotFound,
                    ..
                }
        )
    }
}

/// Locks `mutex`, carrying on after a panic elsewhere: what is behind these
/// locks is a cache and a note, with no invariant a half-finished update
/// could break.
fn lock<T>(mutex: &std::sync::Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the one task that is picking or loading knows.
#[derive(Default)]
struct Work {
    /// When a request to Deezer last failed without a verdict on a song.
    failed_at: Option<Instant>,
}

/// The day, its four songs, and the operations that change them.
pub struct Daily {
    deezer: Deezer,
    store: Arc<dyn Store>,
    /// `<data_dir>/audio`.
    audio_dir: PathBuf,
    clock: Clock,
    retry_after: Duration,
    /// The parsed previews in memory by track ID, least recently used first.
    songs: std::sync::Mutex<Vec<(u64, Arc<Song>)>>,
    /// The song a re-roll took away from a day and section, until the pick
    /// that replaces it is made, so that the replacement is another song
    /// even when it is drawn by a later request.
    replaced: std::sync::Mutex<HashMap<(Date, Section), u64>>,
    /// Locked for the whole of making picks and loading songs, so that
    /// requests arriving together wait for one download instead of each
    /// starting their own, and so that the day's picks are made one after
    /// the other, each knowing the ones before it.
    work: Mutex<Work>,
    /// Read-locked by a request while it uses a song together with the
    /// games played against it; write-locked by whatever changes which song
    /// that is. See [`stands`](Self::stands).
    standing: RwLock<()>,
}

impl Daily {
    /// Nothing is picked or loaded yet; the first [`song`](Self::song) of a
    /// day does that.
    pub fn new(deezer: Deezer, store: Arc<dyn Store>, data_dir: &Path, clock: Clock) -> Self {
        Self {
            deezer,
            store,
            audio_dir: data_dir.join("audio"),
            clock,
            retry_after: RETRY_AFTER,
            songs: std::sync::Mutex::new(Vec::new()),
            replaced: std::sync::Mutex::new(HashMap::new()),
            work: Mutex::new(Work::default()),
            standing: RwLock::new(()),
        }
    }

    /// Overrides [`RETRY_AFTER`].
    #[cfg(test)]
    pub fn with_retry_after(mut self, retry_after: Duration) -> Self {
        self.retry_after = retry_after;
        self
    }

    // --- the day ----------------------------------------------------------------

    /// The real date, the stored offset and the day they make.
    pub async fn days(&self) -> Result<Days, DailyError> {
        let real = self.clock.real_day();
        let offset = self.store.day_offset().await?;
        let today = shifted(real, offset).ok_or(DailyError::Clock(offset))?;
        Ok(Days {
            real,
            offset,
            today,
        })
    }

    /// The server's day: the one picks, games, streaks and the day number
    /// are keyed to. Read it once per request.
    pub async fn today(&self) -> Result<Date, DailyError> {
        Ok(self.days().await?.today)
    }

    // --- the day's songs ----------------------------------------------------------

    /// The song `section` plays on `day`, picking it first if the day has
    /// none yet and loading it if it is not in memory.
    ///
    /// The common case, a pick that is stored and a song that is in memory,
    /// is one read of the store. Otherwise the picks are made in
    /// [`PICK_ORDER`] up to this section, each verified by loading its song,
    /// which can take as long as a download.
    ///
    /// A failure is not fatal. It is logged and returned, and nothing is
    /// stored for the section that failed: the next call tries again, after a
    /// short pause ([`RETRY_AFTER`]) when it was Deezer that failed. A section
    /// behind one that failed waits with it, General behind all three genres;
    /// they share the one Deezer, so little is lost.
    pub async fn song(&self, day: Date, section: Section) -> Result<Playing, DailyError> {
        if let Some(playing) = self.ready(day, section).await? {
            return Ok(playing);
        }

        let mut work = self.work.lock().await;
        let outcome = self.settle(&mut work, day, section).await;
        match &outcome {
            Ok(_) => {}
            // Said once, by the pick that found nothing.
            Err(DailyError::NoSong { .. }) => {}
            Err(error @ DailyError::CoolingDown(_)) => {
                tracing::debug!(%day, %section, %error, "not asking Deezer yet");
            }
            Err(error) => {
                tracing::error!(%day, %section, %error, "could not get the section's song; will retry on a later request");
            }
        }
        outcome
    }

    /// Makes sure every section has had its turn at a pick for `day` and
    /// that the songs are loaded: what the server does at startup, so the
    /// first player does not wait for four downloads. Failures are logged by
    /// [`song`](Self::song) and tried again by the requests that follow.
    pub async fn warm(&self, day: Date) {
        for section in PICK_ORDER {
            let _ = self.song(day, section).await;
        }
    }

    /// Whether `playing` is still what its section plays on its day, and if
    /// so, a guard that keeps it so until it is dropped.
    ///
    /// The admin can replace a day's song (re-roll) or all of them (reset)
    /// while requests are under way, and each of those also deletes the
    /// games played against the old song. A request that combined a song it
    /// fetched before such a change with games it read after it would show
    /// or store nonsense: a finished game of the old song revealing the new
    /// answer, the old game's unlocked clip cut from the new song, a guess
    /// judged against the old song saved into the new game. The player's own
    /// move lock does not help, because the change is not that player's.
    ///
    /// So the changes take this lock's write side, and a request does its
    /// short, final part (read the games, judge the move, save, build the
    /// response) under the read side, after checking here that the stored
    /// pick is still the track it holds. Either the request's part comes
    /// first, and the change then deletes what it stored, or the change comes
    /// first, and the check fails: `None`, and the request starts over or
    /// tells the player. Fetching the song, which can be slow, stays outside
    /// the lock.
    pub async fn stands(&self, playing: &Playing) -> Result<Option<Standing<'_>>, StoreError> {
        let guard = self.standing.read().await;
        let picks = self.store.picks_on(playing.day).await?;
        let stands = picks
            .iter()
            .any(|pick| pick.section == playing.section && pick.track_id == playing.track_id);
        Ok(stands.then_some(Standing { _held: guard }))
    }

    /// The section's song when nothing has to be done for it: the pick is
    /// stored and its song is in memory.
    async fn ready(&self, day: Date, section: Section) -> Result<Option<Playing>, StoreError> {
        let picks = self.store.picks_on(day).await?;
        let Some(pick) = picks.iter().find(|pick| pick.section == section) else {
            return Ok(None);
        };
        Ok(self.in_memory(pick.track_id).map(|song| Playing {
            day,
            section,
            track_id: pick.track_id,
            song,
        }))
    }

    /// Gives every section ahead of `section` in [`PICK_ORDER`] its turn,
    /// then picks (or loads the pick of) `section` itself.
    async fn settle(
        &self,
        work: &mut Work,
        day: Date,
        section: Section,
    ) -> Result<Playing, DailyError> {
        let picks = self.store.picks_on(day).await?;
        let picked = |section: Section| picks.iter().find(|pick| pick.section == section);

        for earlier in PICK_ORDER
            .into_iter()
            .take_while(|earlier| *earlier != section)
        {
            if picked(earlier).is_none() {
                // `Ok(None)` is a turn too: that section has no song today,
                // and the ones after it need not wait for it.
                self.make_pick(work, day, earlier).await?;
            }
        }

        match picked(section) {
            Some(pick) => self.load_standing(work, *pick).await,
            None => self
                .make_pick(work, day, section)
                .await?
                .ok_or(DailyError::NoSong { day, section }),
        }
    }

    /// Draws a song for `section` on `day`, checks that it can be played and
    /// stores it as the pick. `Ok(None)`: nothing in the pool can be played
    /// there today.
    async fn make_pick(
        &self,
        work: &mut Work,
        day: Date,
        section: Section,
    ) -> Result<Option<Playing>, DailyError> {
        loop {
            // Read again on every round: the song that just failed the check
            // is marked in the pool, which is what takes it out of the draw.
            let pool = self.store.songs().await?;
            let history = self.store.pick_history(section).await?;
            let picks = self.store.picks_on(day).await?;
            if let Some(pick) = picks.iter().find(|pick| pick.section == section) {
                return self.load_standing(work, *pick).await.map(Some);
            }
            let taken: BTreeSet<u64> = picks.iter().map(|pick| pick.track_id).collect();
            let replaced = lock(&self.replaced).get(&(day, section)).copied();

            let drawn = {
                // In a block of its own: the generator must not be alive
                // across an `await`.
                let mut rng = rand::rng();
                pick::choose(section, day, &pool, &history, &taken, replaced, &mut rng)
            };
            let Some(track_id) = drawn else {
                tracing::info!(%day, %section, "no song to play in this section today");
                return Ok(None);
            };

            match self.load(work, track_id).await {
                Ok(song) => {
                    let pick = Pick {
                        day,
                        section,
                        track_id,
                    };
                    let stands = self.store.save_pick(pick).await?;
                    if stands != pick {
                        // Another server process on the same store drew first.
                        // Its pick is the day's song.
                        return self.load_standing(work, stands).await.map(Some);
                    }
                    lock(&self.replaced).remove(&(day, section));
                    // It plays, so an earlier failed check is history.
                    let marked = pool
                        .iter()
                        .any(|song| song.track_id == track_id && song.preview_failed_on.is_some());
                    if marked {
                        self.store.set_preview_failed_on(track_id, None).await?;
                    }
                    // The answer goes to the log, which only the operator reads.
                    tracing::info!(
                        %day, %section, track_id,
                        title = %song.answer.title,
                        artist = %song.answer.artist,
                        "picked the section's song for the day"
                    );
                    return Ok(Some(Playing {
                        day,
                        section,
                        track_id,
                        song,
                    }));
                }
                Err(error) if error.condemns_the_song() => {
                    tracing::warn!(%day, %section, %error, "skipping a song without a preview; drawing another");
                    self.store
                        .set_preview_failed_on(track_id, Some(day))
                        .await?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Loads the song of a pick that is already the day's song.
    async fn load_standing(&self, work: &mut Work, pick: Pick) -> Result<Playing, DailyError> {
        match self.load(work, pick.track_id).await {
            Ok(song) => Ok(Playing {
                day: pick.day,
                section: pick.section,
                track_id: pick.track_id,
                song,
            }),
            Err(error) => {
                if error.condemns_the_song() {
                    // It was checked when it was picked, so this takes the
                    // disk cache going missing and Deezer withdrawing the
                    // track on the same day. The pick is not replaced behind
                    // the players' backs: games were played against it.
                    tracing::error!(
                        day = %pick.day, section = %pick.section, %error,
                        "the day's song can no longer be played; re-roll the section"
                    );
                    self.store
                        .set_preview_failed_on(pick.track_id, Some(pick.day))
                        .await?;
                }
                Err(error)
            }
        }
    }

    // --- changing the day and its songs ---------------------------------------------

    /// Takes today's song away from each of `sections` and deletes every
    /// player's game there today. Returns the day it did that for.
    ///
    /// The new pick is not made here. It is made by the next request for the
    /// section, like any day's first pick, and it will be another song when
    /// there is another: the one taken away is remembered until then. The
    /// other sections' picks stand, so the new song is none of theirs. Making
    /// it here would mean a download under the lock that every player
    /// request waits for.
    pub async fn reroll(&self, sections: &[Section]) -> Result<Date, DailyError> {
        let _change = self.standing.write().await;
        let day = self.today().await?;
        let picks = self.store.picks_on(day).await?;
        for &section in sections {
            // The games first: a pick without games is a day nobody has
            // played, games without their pick are judged against nothing.
            let games = self.store.delete_games(section, day).await?;
            let old = picks.iter().find(|pick| pick.section == section);
            if let Some(old) = old {
                self.store.remove_pick(day, section).await?;
                let mut replaced = lock(&self.replaced);
                replaced.retain(|(of, _), _| *of == day);
                replaced.insert((day, section), old.track_id);
            }
            tracing::info!(
                %day, %section, games,
                replaced = old.map(|pick| pick.track_id),
                "re-rolling a section: its pick and its games for the day are gone"
            );
        }
        Ok(day)
    }

    /// Simulate next day: adds one to the day offset. Every player is on the
    /// new day from their next request on; nothing is deleted.
    pub async fn next_day(&self) -> Result<Days, DailyError> {
        let _change = self.standing.write().await;
        let before = self.days().await?;
        let offset = before
            .offset
            .checked_add(1)
            .filter(|offset| shifted(before.real, *offset).is_some())
            .ok_or(DailyError::Clock(before.offset))?;
        self.store.set_day_offset(offset).await?;
        let after = self.days().await?;
        tracing::info!(from = %before.today, to = %after.today, offset, "moved to the next day");
        Ok(after)
    }

    /// Reset to day 1: deletes every game and every pick, and sets the day
    /// offset so that today is `launch`. The song pool is kept.
    pub async fn reset(&self, launch: Date) -> Result<Days, DailyError> {
        let _change = self.standing.write().await;
        // Wipe first: interrupted after it, the day is unchanged and simply
        // has no games and no picks yet.
        self.store.wipe_games_and_picks().await?;
        // Subtracting civil dates yields a span in whole days and cannot fail.
        let offset = i64::from((launch - self.clock.real_day()).get_days());
        self.store.set_day_offset(offset).await?;
        lock(&self.replaced).clear();
        let after = self.days().await?;
        tracing::info!(day = %after.today, offset, "reset to day 1: every game and every pick is gone");
        Ok(after)
    }

    // --- loading ----------------------------------------------------------------------

    /// The song of `track_id`: from memory, else from the disk cache, else
    /// from Deezer, unless a request to Deezer failed a moment ago.
    async fn load(&self, work: &mut Work, track_id: u64) -> Result<Arc<Song>, DailyError> {
        if let Some(song) = self.in_memory(track_id) {
            return Ok(song);
        }
        if let Some(song) = self.load_cached(track_id).await {
            log_loaded(track_id, &song, "the disk cache");
            return Ok(self.keep(track_id, song));
        }

        if let Some(failed_at) = work.failed_at {
            let waited = failed_at.elapsed();
            if waited < self.retry_after {
                return Err(DailyError::CoolingDown(self.retry_after - waited));
            }
        }
        match self.fetch(track_id).await {
            Ok(song) => {
                work.failed_at = None;
                log_loaded(track_id, &song, "Deezer");
                Ok(self.keep(track_id, song))
            }
            Err(error) => {
                // A verdict on the song is an answer, not a failure: the
                // next candidate may be asked about at once.
                if !error.condemns_the_song() {
                    work.failed_at = Some(Instant::now());
                }
                Err(error)
            }
        }
    }

    /// Looks the track up on Deezer and downloads its preview.
    async fn fetch(&self, track_id: u64) -> Result<Song, DailyError> {
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

        if let Err(error) = self.cache(track_id, &track, &bytes).await {
            // The song is in memory, so the game works; only the next restart
            // has to download again.
            tracing::warn!(%error, dir = %self.audio_dir.display(), "could not cache the preview on disk");
        }
        Ok(Song::new(&track, mp3))
    }

    /// The song if it is in memory, counted as used just now.
    fn in_memory(&self, track_id: u64) -> Option<Arc<Song>> {
        let mut songs = lock(&self.songs);
        let index = songs.iter().position(|(id, _)| *id == track_id)?;
        let entry = songs.remove(index);
        let song = Arc::clone(&entry.1);
        songs.push(entry);
        Some(song)
    }

    /// Puts a loaded song into memory, dropping the least recently used ones
    /// beyond [`SONGS_IN_MEMORY`].
    fn keep(&self, track_id: u64, song: Song) -> Arc<Song> {
        let song = Arc::new(song);
        let mut songs = lock(&self.songs);
        songs.retain(|(id, _)| *id != track_id);
        songs.push((track_id, Arc::clone(&song)));
        let excess = songs.len().saturating_sub(SONGS_IN_MEMORY);
        songs.drain(..excess);
        song
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
    /// loading needs both, so an interrupted write leaves nothing that looks
    /// complete.
    async fn cache(&self, track_id: u64, track: &Track, audio: &[u8]) -> io::Result<()> {
        tokio::fs::create_dir_all(&self.audio_dir).await?;
        // `Track` leaves its preview URL out when serialized.
        let json = serde_json::to_vec_pretty(track)?;
        write_atomically(&self.mp3_path(track_id), audio).await?;
        write_atomically(&self.json_path(track_id), &json).await
    }
}

/// The answer goes to the log, which only the operator reads.
fn log_loaded(track_id: u64, song: &Song, source: &str) {
    tracing::info!(
        track_id,
        title = %song.answer.title,
        artist = %song.answer.artist,
        frames = song.mp3.frame_count(),
        duration_ms = song.mp3.duration_ms(),
        sample_rate = song.mp3.sample_rate(),
        "loaded a song from {source}"
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
    use crate::{
        store::{Genre, Genres, MemoryStore, NewSong, PlayerId},
        testutil::{MockDeezer, MockTrack, Preview, TestClock, playing_game, synthetic_mp3},
    };
    use jiff::civil::date;

    const DAY: Date = date(2026, 10, 3);
    const LAUNCH: Date = date(2026, 10, 1);
    const ID: u64 = 4242;
    const NOT_AN_MP3: u64 = 7;
    const WITHDRAWN: u64 = 8;
    const UNKNOWN: u64 = 999;

    const POP: Section = Section::Genre(Genre::Pop);
    const ROCK: Section = Section::Genre(Genre::Rock);
    const HIP_HOP: Section = Section::Genre(Genre::HipHop);

    fn answer_track() -> MockTrack {
        MockTrack::new(ID, "Zanzibar Nights (Remastered 2011)", "The Answers")
            .title_short("Zanzibar Nights")
            .album("Night Album")
    }

    /// A Deezer that knows the answer track, two tracks without a usable
    /// preview, and twenty ordinary ones (100 to 119).
    async fn mock() -> MockDeezer {
        let mut tracks = vec![
            answer_track(),
            MockTrack::new(NOT_AN_MP3, "Not A Song", "Nobody").preview(Preview::Html),
            MockTrack::new(WITHDRAWN, "Withdrawn", "Nobody").preview(Preview::None),
        ];
        for id in 100..120 {
            tracks.push(MockTrack::new(id, &format!("Song {id}"), "Someone"));
        }
        MockDeezer::start(tracks).await
    }

    /// A server's worth of state: the Deezer stand-in, a store, a data
    /// directory and a clock that says [`DAY`].
    struct Bench {
        deezer: MockDeezer,
        store: Arc<dyn Store>,
        dir: tempfile::TempDir,
        clock: TestClock,
    }

    impl Bench {
        /// `pool` is the song pool: track IDs with their genre tags.
        async fn new(pool: &[(u64, &[Genre])]) -> Self {
            let bench = Self {
                deezer: mock().await,
                store: Arc::new(MemoryStore::new()),
                dir: tempfile::tempdir().unwrap(),
                clock: TestClock::new(DAY),
            };
            for (track_id, genres) in pool {
                bench.add(*track_id, genres).await;
            }
            bench
        }

        async fn add(&self, track_id: u64, genres: &[Genre]) {
            let song = NewSong {
                track_id,
                title: format!("Song {track_id}"),
                title_short: format!("Song {track_id}"),
                artist: "Someone".to_owned(),
                album: "LP".to_owned(),
            };
            let genres: Genres = genres.iter().copied().collect();
            self.store.add_song(song, genres).await.unwrap();
        }

        /// A server process: nothing in memory, a pause after a failure.
        fn daily(&self) -> Daily {
            Daily::new(
                self.deezer.client(),
                Arc::clone(&self.store),
                self.dir.path(),
                self.clock.clock(),
            )
        }

        /// The same, retrying at once.
        fn eager(&self) -> Daily {
            self.daily().with_retry_after(Duration::ZERO)
        }

        async fn pick(&self, day: Date, section: Section) -> Option<u64> {
            self.store
                .picks_on(day)
                .await
                .unwrap()
                .iter()
                .find(|pick| pick.section == section)
                .map(|pick| pick.track_id)
        }

        async fn failed_on(&self, track_id: u64) -> Option<Date> {
            self.store
                .song(track_id)
                .await
                .unwrap()
                .unwrap()
                .preview_failed_on
        }

        fn cached(&self, track_id: u64) -> bool {
            let audio = self.dir.path().join("audio");
            audio.join(format!("{track_id}.mp3")).exists()
                && audio.join(format!("{track_id}.json")).exists()
        }
    }

    // --- loading and the disk cache ---------------------------------------------

    #[tokio::test]
    async fn the_first_request_picks_loads_from_deezer_and_caches_on_disk() {
        let bench = Bench::new(&[(ID, &[])]).await;
        let daily = bench.daily();

        let playing = daily.song(DAY, Section::General).await.unwrap();
        assert_eq!(playing.track_id, ID);
        assert_eq!(playing.day, DAY);
        assert_eq!(playing.section, Section::General);
        let song = &playing.song;
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
        assert_eq!(bench.deezer.api_hits(), 1);
        // The pick is in the store.
        assert_eq!(bench.pick(DAY, Section::General).await, Some(ID));

        // In memory now: asking again costs Deezer nothing.
        let again = daily.song(DAY, Section::General).await.unwrap();
        assert!(Arc::ptr_eq(&again.song, &playing.song));
        assert_eq!(bench.deezer.api_hits(), 1);

        let dir = bench.dir.path();
        let audio = std::fs::read(dir.join("audio/4242.mp3")).unwrap();
        assert_eq!(audio, synthetic_mp3(1148));
        let json = std::fs::read_to_string(dir.join("audio/4242.json")).unwrap();
        assert!(json.contains("Zanzibar Nights"), "{json}");
        // The signed preview URL is never stored.
        assert!(!json.contains("preview"), "{json}");
        assert!(!dir.join("audio/4242.mp3.tmp").exists());
    }

    #[tokio::test]
    async fn a_restart_keeps_the_days_songs_and_loads_them_from_disk_without_deezer() {
        let bench = Bench::new(&[
            (100, &[Genre::Pop]),
            (101, &[Genre::Pop]),
            (102, &[Genre::Rock]),
            (103, &[Genre::Rock]),
            (104, &[Genre::HipHop]),
            (105, &[Genre::HipHop]),
        ])
        .await;
        let first = bench.daily();
        let mut before = Vec::new();
        for section in Section::ALL {
            before.push(first.song(DAY, section).await.unwrap());
        }
        assert_eq!(bench.deezer.api_hits(), 4);
        drop(first);

        // A new process over the same store and data directory, with Deezer
        // gone: the same four songs, from the stored picks and the disk.
        bench.deezer.set_failing(true);
        let restarted = bench.daily();
        for old in &before {
            let new = restarted.song(DAY, old.section).await.unwrap();
            assert_eq!(new.track_id, old.track_id, "{}", old.section);
            assert_eq!(new.song.answer, old.song.answer);
            assert_eq!(new.song.meta, old.song.meta);
            assert_eq!(new.song.mp3.audio(), old.song.mp3.audio());
        }
        assert_eq!(bench.deezer.api_hits(), 4);
    }

    #[tokio::test]
    async fn a_corrupt_cache_is_replaced_from_deezer() {
        let bench = Bench::new(&[(ID, &[])]).await;
        let audio_dir = bench.dir.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        std::fs::write(audio_dir.join("4242.mp3"), b"<html>expired</html>").unwrap();
        std::fs::write(
            audio_dir.join("4242.json"),
            br#"{"id":4242,"title":"Stale"}"#,
        )
        .unwrap();

        let playing = bench.daily().song(DAY, Section::General).await.unwrap();
        assert_eq!(
            playing.song.answer.title,
            "Zanzibar Nights (Remastered 2011)"
        );
        assert_eq!(bench.deezer.api_hits(), 1);
        assert_eq!(
            std::fs::read(audio_dir.join("4242.mp3")).unwrap(),
            synthetic_mp3(1148)
        );
    }

    #[tokio::test]
    async fn half_a_cache_is_no_cache() {
        let bench = Bench::new(&[(ID, &[])]).await;
        let audio_dir = bench.dir.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        // The audio without its JSON: what an interrupted write leaves behind.
        std::fs::write(audio_dir.join("4242.mp3"), synthetic_mp3(1148)).unwrap();

        bench.daily().song(DAY, Section::General).await.unwrap();
        assert_eq!(bench.deezer.api_hits(), 1);
        assert!(audio_dir.join("4242.json").exists());
    }

    #[tokio::test]
    async fn only_so_many_songs_are_kept_in_memory() {
        let bench = Bench::new(&[]).await;
        let daily = bench.daily();
        let mut work = Work::default();
        for track_id in 100..100 + SONGS_IN_MEMORY as u64 + 3 {
            daily.load(&mut work, track_id).await.unwrap();
            // The first one is asked for again every time, so it stays.
            assert!(daily.in_memory(100).is_some());
        }

        let kept: Vec<u64> = lock(&daily.songs).iter().map(|(id, _)| *id).collect();
        assert_eq!(kept.len(), SONGS_IN_MEMORY);
        assert!(kept.contains(&100));
        // The least recently used ones went: the second to the fourth.
        for gone in 101..104 {
            assert!(!kept.contains(&gone), "{kept:?}");
        }
        // A song that went comes back from the disk, not from Deezer.
        let hits = bench.deezer.api_hits();
        daily.load(&mut work, 101).await.unwrap();
        assert_eq!(bench.deezer.api_hits(), hits);
    }

    // --- the day's four picks ---------------------------------------------------

    #[tokio::test]
    async fn the_four_sections_play_four_different_songs_from_their_own_pools() {
        let bench = Bench::new(&[
            (100, &[Genre::Pop]),
            (101, &[Genre::Pop]),
            (102, &[Genre::Rock]),
            (103, &[Genre::Rock]),
            (104, &[Genre::HipHop]),
            (105, &[Genre::HipHop]),
        ])
        .await;
        let daily = bench.daily();

        // General is asked first and still picks last: the genres get their
        // turn before it.
        let general = daily.song(DAY, Section::General).await.unwrap().track_id;
        let pop = bench.pick(DAY, POP).await.unwrap();
        let rock = bench.pick(DAY, ROCK).await.unwrap();
        let hip_hop = bench.pick(DAY, HIP_HOP).await.unwrap();
        assert!([100, 101].contains(&pop));
        assert!([102, 103].contains(&rock));
        assert!([104, 105].contains(&hip_hop));
        let distinct = BTreeSet::from([general, pop, rock, hip_hop]);
        assert_eq!(distinct.len(), 4);
        assert_eq!(bench.deezer.api_hits(), 4);

        // Asking for the genres now changes nothing and costs nothing.
        for (section, track_id) in [(POP, pop), (ROCK, rock), (HIP_HOP, hip_hop)] {
            assert_eq!(daily.song(DAY, section).await.unwrap().track_id, track_id);
        }
        assert_eq!(bench.deezer.api_hits(), 4);
        assert_eq!(bench.store.picks_on(DAY).await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn a_genre_asked_for_alone_does_not_pick_the_sections_after_it() {
        let bench = Bench::new(&[
            (100, &[Genre::Pop]),
            (102, &[Genre::Rock]),
            (104, &[Genre::HipHop]),
            (110, &[]),
        ])
        .await;
        let daily = bench.daily();

        daily.song(DAY, ROCK).await.unwrap();
        // Pop is ahead of Rock in the order and had its turn; the two after
        // it wait until somebody asks.
        assert_eq!(bench.pick(DAY, POP).await, Some(100));
        assert_eq!(bench.pick(DAY, ROCK).await, Some(102));
        assert_eq!(bench.pick(DAY, HIP_HOP).await, None);
        assert_eq!(bench.pick(DAY, Section::General).await, None);
        assert_eq!(bench.deezer.api_hits(), 2);
    }

    #[tokio::test]
    async fn requests_arriving_together_agree_on_one_pick_and_one_download() {
        let pool: Vec<(u64, &[Genre])> = (100..110).map(|id| (id, &[] as &[Genre])).collect();
        let bench = Bench::new(&pool).await;
        let daily = bench.daily();

        let (a, b, c) = tokio::join!(
            daily.song(DAY, Section::General),
            daily.song(DAY, Section::General),
            daily.song(DAY, Section::General),
        );
        let (a, b, c) = (a.unwrap(), b.unwrap(), c.unwrap());
        assert_eq!(a.track_id, b.track_id);
        assert_eq!(a.track_id, c.track_id);
        assert_eq!(bench.deezer.api_hits(), 1);
    }

    #[tokio::test]
    async fn a_section_does_not_repeat_a_song_from_day_to_day_until_its_pool_is_used_up() {
        let bench = Bench::new(&[(100, &[]), (101, &[]), (102, &[])]).await;
        let daily = bench.daily();

        let mut played = Vec::new();
        let mut day = DAY;
        for _ in 0..6 {
            played.push(daily.song(day, Section::General).await.unwrap().track_id);
            day = day.tomorrow().unwrap();
        }
        for round in played.chunks(3) {
            let distinct: BTreeSet<u64> = round.iter().copied().collect();
            assert_eq!(distinct.len(), 3, "{played:?}");
        }
        assert_ne!(played[2], played[3], "{played:?}");
        // Each song was fetched once; the second round came from memory.
        assert_eq!(bench.deezer.api_hits(), 3);
    }

    #[tokio::test]
    async fn a_section_with_nothing_to_play_has_no_song_and_does_not_hold_up_the_others() {
        // Nothing is tagged rock, and Hip-hop's only song has no preview.
        let bench = Bench::new(&[
            (100, &[Genre::Pop]),
            (WITHDRAWN, &[Genre::HipHop]),
            (110, &[]),
        ])
        .await;
        let daily = bench.daily();

        let general = daily.song(DAY, Section::General).await.unwrap();
        assert_eq!(general.track_id, 110);
        for section in [ROCK, HIP_HOP] {
            let error = daily.song(DAY, section).await.unwrap_err();
            assert!(
                matches!(error, DailyError::NoSong { day, section: of } if day == DAY && of == section),
                "{error}"
            );
            assert_eq!(bench.pick(DAY, section).await, None);
        }
        assert_eq!(daily.song(DAY, POP).await.unwrap().track_id, 100);

        // A song added later in the day gives the section one after all.
        bench.add(102, &[Genre::Rock]).await;
        assert_eq!(daily.song(DAY, ROCK).await.unwrap().track_id, 102);
    }

    #[tokio::test]
    async fn general_has_no_song_when_the_genres_took_everything() {
        let bench = Bench::new(&[(100, &[Genre::Pop]), (102, &[Genre::Rock])]).await;
        let daily = bench.daily();

        let error = daily.song(DAY, Section::General).await.unwrap_err();
        assert!(matches!(error, DailyError::NoSong { .. }), "{error}");
        assert_eq!(bench.pick(DAY, POP).await, Some(100));
        assert_eq!(bench.pick(DAY, ROCK).await, Some(102));
        assert_eq!(bench.pick(DAY, Section::General).await, None);
    }

    // --- the preview check --------------------------------------------------------

    #[tokio::test]
    async fn a_song_without_a_preview_is_skipped_marked_and_another_is_drawn() {
        let bench = Bench::new(&[
            (WITHDRAWN, &[Genre::Pop]),
            (NOT_AN_MP3, &[Genre::Pop]),
            (UNKNOWN, &[Genre::Pop]),
            (100, &[Genre::Pop]),
        ])
        .await;
        let daily = bench.daily();

        // Whatever is drawn first, the one song that plays is what is picked.
        let playing = daily.song(DAY, POP).await.unwrap();
        assert_eq!(playing.track_id, 100);
        assert_eq!(bench.pick(DAY, POP).await, Some(100));
        assert_eq!(bench.failed_on(100).await, None);
        // Nothing that is not an MP3 was written to the cache.
        for track_id in [WITHDRAWN, NOT_AN_MP3, UNKNOWN] {
            assert!(!bench.cached(track_id));
        }
        assert!(bench.cached(100));

        // The others stay in the pool. Those that were drawn before it are
        // marked with today; General, drawing from what is left, finds out
        // about the rest and ends up with nothing.
        let error = daily.song(DAY, Section::General).await.unwrap_err();
        assert!(matches!(error, DailyError::NoSong { .. }), "{error}");
        for track_id in [WITHDRAWN, NOT_AN_MP3, UNKNOWN] {
            assert_eq!(bench.failed_on(track_id).await, Some(DAY), "{track_id}");
        }
        assert_eq!(bench.store.songs().await.unwrap().len(), 4);

        // Marked songs are not asked about again today.
        let hits = bench.deezer.api_hits();
        assert!(daily.song(DAY, Section::General).await.is_err());
        assert_eq!(bench.deezer.api_hits(), hits);
    }

    #[tokio::test]
    async fn a_skipped_song_is_tried_again_the_next_day_and_unmarked_when_it_plays() {
        let bench = Bench::new(&[(100, &[])]).await;
        // It failed yesterday, by the record.
        let yesterday = DAY.yesterday().unwrap();
        bench
            .store
            .set_preview_failed_on(100, Some(yesterday))
            .await
            .unwrap();

        let playing = bench.daily().song(DAY, Section::General).await.unwrap();
        assert_eq!(playing.track_id, 100);
        assert_eq!(bench.failed_on(100).await, None);
    }

    #[tokio::test]
    async fn a_deezer_failure_is_no_verdict_on_a_song() {
        let bench = Bench::new(&[(100, &[]), (101, &[]), (102, &[])]).await;
        bench.deezer.set_failing(true);

        let patient = bench.daily();
        let error = patient.song(DAY, Section::General).await.unwrap_err();
        assert!(matches!(error, DailyError::Lookup { .. }), "{error}");
        assert_eq!(bench.deezer.api_hits(), 1);
        // Nothing was picked and no song was blamed.
        assert_eq!(bench.store.picks_on(DAY).await.unwrap(), Vec::new());
        for track_id in [100, 101, 102] {
            assert_eq!(bench.failed_on(track_id).await, None);
        }

        // Deezer is back, but the pause is not over: no request goes out,
        // for this section or any other.
        bench.deezer.set_failing(false);
        for section in [Section::General, POP] {
            let error = patient.song(DAY, section).await.unwrap_err();
            assert!(
                matches!(
                    error,
                    DailyError::CoolingDown(_) | DailyError::NoSong { .. }
                ),
                "{error}"
            );
        }
        assert!(matches!(
            patient.song(DAY, Section::General).await,
            Err(DailyError::CoolingDown(_))
        ));
        assert_eq!(bench.deezer.api_hits(), 1);

        // With no pause, the next call after a failure simply tries again,
        // and the whole pool is still there to draw from.
        bench.deezer.set_failing(true);
        let eager = bench.eager();
        assert!(eager.song(DAY, Section::General).await.is_err());
        assert!(eager.song(DAY, Section::General).await.is_err());
        bench.deezer.set_failing(false);
        let playing = eager.song(DAY, Section::General).await.unwrap();
        assert!([100, 101, 102].contains(&playing.track_id));
        for track_id in [100, 101, 102] {
            assert_eq!(bench.failed_on(track_id).await, None);
        }
    }

    #[tokio::test]
    async fn a_song_in_the_disk_cache_is_picked_without_asking_deezer() {
        let bench = Bench::new(&[(100, &[])]).await;
        // Yesterday's server played it and cached it.
        bench
            .daily()
            .song(DAY.yesterday().unwrap(), Section::General)
            .await
            .unwrap();
        assert_eq!(bench.deezer.api_hits(), 1);

        bench.deezer.set_failing(true);
        let playing = bench.daily().song(DAY, Section::General).await.unwrap();
        assert_eq!(playing.track_id, 100);
        assert_eq!(bench.deezer.api_hits(), 1);
    }

    #[tokio::test]
    async fn general_waits_for_a_genre_that_is_stuck_on_deezer() {
        let bench = Bench::new(&[(100, &[Genre::Pop]), (110, &[])]).await;
        // General's own candidate is on disk; Pop's is not.
        let warm = bench.eager();
        warm.load(&mut Work::default(), 110).await.unwrap();
        drop(warm);
        bench.deezer.set_failing(true);

        let daily = bench.eager();
        // Not "no song", and not the cached song either: if General took 110
        // or 100 now, it could not know what Pop will play.
        let error = daily.song(DAY, Section::General).await.unwrap_err();
        assert!(matches!(error, DailyError::Lookup { .. }), "{error}");
        assert_eq!(bench.store.picks_on(DAY).await.unwrap(), Vec::new());

        bench.deezer.set_failing(false);
        assert_eq!(
            daily.song(DAY, Section::General).await.unwrap().track_id,
            110
        );
        assert_eq!(bench.pick(DAY, POP).await, Some(100));
    }

    #[tokio::test]
    async fn a_standing_pick_that_cannot_be_loaded_stays_the_days_song() {
        let bench = Bench::new(&[(100, &[]), (101, &[])]).await;
        let picked = bench
            .daily()
            .song(DAY, Section::General)
            .await
            .unwrap()
            .track_id;

        // A restart with the disk cache gone and Deezer down.
        std::fs::remove_dir_all(bench.dir.path().join("audio")).unwrap();
        bench.deezer.set_failing(true);
        let daily = bench.eager();
        let error = daily.song(DAY, Section::General).await.unwrap_err();
        assert!(matches!(error, DailyError::Lookup { .. }), "{error}");
        // The pick is not thrown away for the other song.
        assert_eq!(bench.pick(DAY, Section::General).await, Some(picked));

        bench.deezer.set_failing(false);
        assert_eq!(
            daily.song(DAY, Section::General).await.unwrap().track_id,
            picked
        );
    }

    // --- the day --------------------------------------------------------------------

    #[tokio::test]
    async fn the_day_is_the_real_date_plus_the_stored_offset() {
        let bench = Bench::new(&[]).await;
        let daily = bench.daily();
        assert_eq!(
            daily.days().await.unwrap(),
            Days {
                real: DAY,
                offset: 0,
                today: DAY
            }
        );

        bench.store.set_day_offset(30).await.unwrap();
        assert_eq!(daily.today().await.unwrap(), date(2026, 11, 2));
        bench.store.set_day_offset(-3).await.unwrap();
        assert_eq!(daily.today().await.unwrap(), date(2026, 9, 30));

        // The real date moving on moves the day with it.
        bench.clock.set(date(2026, 12, 31));
        assert_eq!(
            daily.days().await.unwrap(),
            Days {
                real: date(2026, 12, 31),
                offset: -3,
                today: date(2026, 12, 28)
            }
        );

        // An offset no calendar reaches is an error, not a panic.
        bench.store.set_day_offset(i64::MAX).await.unwrap();
        assert!(matches!(
            daily.today().await,
            Err(DailyError::Clock(i64::MAX))
        ));
    }

    #[tokio::test]
    async fn the_next_day_adds_one_to_the_offset_and_survives_a_restart() {
        let bench = Bench::new(&[]).await;
        let daily = bench.daily();

        let after = daily.next_day().await.unwrap();
        assert_eq!(after.offset, 1);
        assert_eq!(after.today, DAY.tomorrow().unwrap());
        assert_eq!(after.real, DAY);
        daily.next_day().await.unwrap();
        assert_eq!(bench.store.day_offset().await.unwrap(), 2);

        // The offset is in the store, not in the process.
        assert_eq!(bench.daily().today().await.unwrap(), date(2026, 10, 5));

        // The last day of the calendar has no next day.
        bench.store.set_day_offset(i64::MAX).await.unwrap();
        assert!(daily.next_day().await.is_err());
        assert_eq!(bench.store.day_offset().await.unwrap(), i64::MAX);
    }

    #[tokio::test]
    async fn a_reset_goes_back_to_the_launch_day_and_wipes_games_and_picks_but_not_the_pool() {
        let bench = Bench::new(&[(100, &[]), (101, &[Genre::Pop])]).await;
        let daily = bench.daily();
        daily.next_day().await.unwrap();
        let day = daily.today().await.unwrap();
        daily.song(day, Section::General).await.unwrap();
        let player = PlayerId::generate();
        bench
            .store
            .save_game(&player, Section::General, &playing_game(day, 2))
            .await
            .unwrap();
        bench
            .store
            .set_preview_failed_on(100, Some(DAY))
            .await
            .unwrap();
        let pool = bench.store.songs().await.unwrap();

        let after = daily.reset(LAUNCH).await.unwrap();
        assert_eq!(
            after,
            Days {
                real: DAY,
                offset: -2,
                today: LAUNCH
            }
        );
        assert_eq!(bench.store.picks_on(day).await.unwrap(), Vec::new());
        assert_eq!(
            bench.store.pick_history(Section::General).await.unwrap(),
            Vec::new()
        );
        assert_eq!(
            bench.store.games(&player, Section::General).await.unwrap(),
            Vec::new()
        );
        assert_eq!(bench.store.songs().await.unwrap(), pool);

        // A launch date ahead of the real date works the same way.
        let after = daily.reset(date(2026, 10, 10)).await.unwrap();
        assert_eq!(after.offset, 7);
        assert_eq!(after.today, date(2026, 10, 10));
    }

    // --- re-rolls -------------------------------------------------------------------

    #[tokio::test]
    async fn a_reroll_takes_the_pick_and_the_games_away_and_the_next_request_draws_another() {
        let bench = Bench::new(&[
            (100, &[Genre::Pop]),
            (101, &[Genre::Pop]),
            (102, &[Genre::Rock]),
        ])
        .await;
        let daily = bench.daily();
        let old_pop = daily.song(DAY, POP).await.unwrap().track_id;
        let rock = daily.song(DAY, ROCK).await.unwrap().track_id;
        let player = PlayerId::generate();
        let yesterday = DAY.yesterday().unwrap();
        for (section, day) in [(POP, DAY), (ROCK, DAY), (POP, yesterday)] {
            bench
                .store
                .save_game(&player, section, &playing_game(day, 1))
                .await
                .unwrap();
        }

        assert_eq!(daily.reroll(&[POP]).await.unwrap(), DAY);
        assert_eq!(bench.pick(DAY, POP).await, None);
        assert_eq!(bench.pick(DAY, ROCK).await, Some(rock));
        // Only that day's games in that section went.
        assert_eq!(
            bench.store.games(&player, POP).await.unwrap(),
            vec![playing_game(yesterday, 1)]
        );
        assert_eq!(bench.store.games(&player, ROCK).await.unwrap().len(), 1);

        // The next request makes the new pick: the other song.
        let new_pop = daily.song(DAY, POP).await.unwrap().track_id;
        assert_ne!(new_pop, old_pop);
        assert_eq!(bench.pick(DAY, POP).await, Some(new_pop));

        // And back again: with two songs a re-roll always changes the song.
        daily.reroll(&[POP]).await.unwrap();
        assert_eq!(daily.song(DAY, POP).await.unwrap().track_id, old_pop);
    }

    #[tokio::test]
    async fn a_reroll_of_the_only_song_draws_it_again() {
        let bench = Bench::new(&[(100, &[Genre::Pop])]).await;
        let daily = bench.daily();
        assert_eq!(daily.song(DAY, POP).await.unwrap().track_id, 100);

        daily.reroll(&[POP]).await.unwrap();
        assert_eq!(bench.pick(DAY, POP).await, None);
        assert_eq!(daily.song(DAY, POP).await.unwrap().track_id, 100);
    }

    #[tokio::test]
    async fn a_reroll_of_every_section_is_a_new_day_of_picks_with_no_song_twice() {
        let bench = Bench::new(&[
            (100, &[Genre::Pop]),
            (101, &[Genre::Pop]),
            (102, &[Genre::Rock]),
            (103, &[Genre::Rock]),
            (104, &[Genre::HipHop]),
            (105, &[Genre::HipHop]),
        ])
        .await;
        let daily = bench.daily();
        daily.warm(DAY).await;
        let before = bench.store.picks_on(DAY).await.unwrap();
        assert_eq!(before.len(), 4);

        daily.reroll(&PICK_ORDER).await.unwrap();
        assert_eq!(bench.store.picks_on(DAY).await.unwrap(), Vec::new());
        daily.warm(DAY).await;

        let after = bench.store.picks_on(DAY).await.unwrap();
        assert_eq!(after.len(), 4);
        let distinct: BTreeSet<u64> = after.iter().map(|pick| pick.track_id).collect();
        assert_eq!(distinct.len(), 4);
        // Every genre has two songs, so each plays its other one now. General
        // drew from the three the genres left, which are the three they had
        // before, so its old song was not among them.
        for (old, new) in before.iter().zip(&after) {
            assert_eq!(old.section, new.section);
            assert_ne!(old.track_id, new.track_id, "{}", old.section);
        }
    }

    #[tokio::test]
    async fn a_song_stands_until_its_section_is_rerolled_or_everything_is_reset() {
        let bench = Bench::new(&[
            (100, &[Genre::Pop]),
            (101, &[Genre::Pop]),
            (102, &[Genre::Rock]),
        ])
        .await;
        let daily = bench.daily();
        let pop = daily.song(DAY, POP).await.unwrap();
        let rock = daily.song(DAY, ROCK).await.unwrap();
        assert!(daily.stands(&pop).await.unwrap().is_some());

        daily.reroll(&[POP]).await.unwrap();
        // Gone, and still not the day's song once another has been drawn.
        assert!(daily.stands(&pop).await.unwrap().is_none());
        daily.song(DAY, POP).await.unwrap();
        assert!(daily.stands(&pop).await.unwrap().is_none());
        // The section that was not re-rolled is untouched.
        assert!(daily.stands(&rock).await.unwrap().is_some());

        // The next day changes no pick: yesterday's song is still yesterday's.
        daily.next_day().await.unwrap();
        assert!(daily.stands(&rock).await.unwrap().is_some());
        daily.reset(LAUNCH).await.unwrap();
        assert!(daily.stands(&rock).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_change_waits_for_the_requests_that_hold_a_standing_song() {
        let bench = Bench::new(&[(100, &[]), (101, &[])]).await;
        let daily = bench.daily();
        let playing = daily.song(DAY, Section::General).await.unwrap();

        let held = daily.stands(&playing).await.unwrap().unwrap();
        let brief = Duration::from_millis(20);
        // While a request holds its song, none of the three changes happens.
        assert!(
            tokio::time::timeout(brief, daily.reroll(&[Section::General]))
                .await
                .is_err()
        );
        assert!(tokio::time::timeout(brief, daily.next_day()).await.is_err());
        assert!(
            tokio::time::timeout(brief, daily.reset(LAUNCH))
                .await
                .is_err()
        );
        assert_eq!(
            bench.pick(DAY, Section::General).await,
            Some(playing.track_id)
        );
        assert_eq!(bench.store.day_offset().await.unwrap(), 0);

        // Once it lets go, they do.
        drop(held);
        daily.reroll(&[Section::General]).await.unwrap();
        assert_eq!(bench.pick(DAY, Section::General).await, None);
    }

    // --- pure pieces ------------------------------------------------------------------

    #[test]
    fn only_deezers_word_on_the_track_condemns_a_song() {
        let verdicts = [
            DailyError::Unplayable(1),
            DailyError::BadAudio {
                track_id: 1,
                source: Mp3Error::NoAudioFrames,
            },
            DailyError::Lookup {
                track_id: 1,
                source: DeezerError::NotFound,
            },
        ];
        for error in verdicts {
            assert!(error.condemns_the_song(), "{error}");
        }

        let no_verdicts = [
            DailyError::Lookup {
                track_id: 1,
                source: DeezerError::Throttled,
            },
            DailyError::Lookup {
                track_id: 1,
                source: DeezerError::Api {
                    code: 4,
                    message: "Quota limit exceeded".to_owned(),
                },
            },
            DailyError::Download {
                track_id: 1,
                source: DeezerError::Throttled,
            },
            DailyError::CoolingDown(Duration::from_secs(3)),
            DailyError::NoSong {
                day: DAY,
                section: POP,
            },
            DailyError::Store(StoreError::new("reading", "disk full")),
            DailyError::Clock(1),
        ];
        for error in no_verdicts {
            assert!(!error.condemns_the_song(), "{error}");
        }
    }

    #[test]
    fn a_day_is_shifted_by_whole_days_in_either_direction() {
        assert_eq!(shifted(DAY, 0), Some(DAY));
        assert_eq!(shifted(DAY, 1), Some(date(2026, 10, 4)));
        assert_eq!(shifted(DAY, -3), Some(date(2026, 9, 30)));
        assert_eq!(shifted(DAY, 365), Some(date(2027, 10, 3)));
        assert_eq!(shifted(DAY, i64::MAX), None);
        assert_eq!(shifted(DAY, i64::MIN), None);
        assert_eq!(shifted(date(9999, 12, 31), 1), None);
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
