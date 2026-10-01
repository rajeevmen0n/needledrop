//! The in-memory backend: the store the tests run against, and `kind = "memory"` for a server that keeps nothing.
//!
//! It is also the plainest statement of what [`Store`] means. A real backend
//! is right when it behaves like this one, which is what the shared contract
//! suite checks.

use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard, PoisonError},
};

use async_trait::async_trait;
use jiff::civil::Date;

use super::{Genres, NewSong, Pick, PlayerId, PoolSong, Section, Store, StoreError};
use crate::game::GameState;

/// What identifies a stored game. Ordered by player, then section, then day,
/// so one player's games in one section sit together, oldest first: the
/// order [`Store::games`] promises.
type GameKey = (PlayerId, Section, Date);

/// A [`Store`] that lives and dies with the process.
#[derive(Debug, Default)]
pub struct MemoryStore {
    /// The pool by track ID, which is also the order [`Store::songs`] promises.
    songs: Mutex<BTreeMap<u64, PoolSong>>,
    /// Every game any player has made a move in.
    games: Mutex<BTreeMap<GameKey, GameState>>,
    /// The track picked for each day and section. Ordered by day, then
    /// section, which gives [`Store::picks_on`] and [`Store::pick_history`]
    /// their orders.
    picks: Mutex<BTreeMap<(Date, Section), u64>>,
    /// Days the server's day is ahead of the real date.
    day_offset: Mutex<i64>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Locks the pool, carrying on after a panic elsewhere: every update is a
    /// single map operation, so there is no half-finished state to find.
    fn pool(&self) -> MutexGuard<'_, BTreeMap<u64, PoolSong>> {
        self.songs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the games, on the same terms as [`pool`](Self::pool).
    fn played(&self) -> MutexGuard<'_, BTreeMap<GameKey, GameState>> {
        self.games.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the picks, on the same terms as [`pool`](Self::pool).
    fn picked(&self) -> MutexGuard<'_, BTreeMap<(Date, Section), u64>> {
        self.picks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the day offset, on the same terms as [`pool`](Self::pool).
    fn offset(&self) -> MutexGuard<'_, i64> {
        self.day_offset
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// A stored pick as the trait hands it out.
fn pick_of((&(day, section), &track_id): (&(Date, Section), &u64)) -> Pick {
    Pick {
        day,
        section,
        track_id,
    }
}

#[async_trait]
impl Store for MemoryStore {
    async fn songs(&self) -> Result<Vec<PoolSong>, StoreError> {
        Ok(self.pool().values().cloned().collect())
    }

    async fn song(&self, track_id: u64) -> Result<Option<PoolSong>, StoreError> {
        Ok(self.pool().get(&track_id).cloned())
    }

    async fn add_song(&self, song: NewSong, genres: Genres) -> Result<PoolSong, StoreError> {
        let mut pool = self.pool();
        let stored = pool
            .entry(song.track_id)
            .and_modify(|existing| existing.genres.clone_from(&genres))
            .or_insert_with(|| PoolSong::new(song, genres.clone()));
        Ok(stored.clone())
    }

    async fn set_song_genres(
        &self,
        track_id: u64,
        genres: Genres,
    ) -> Result<Option<PoolSong>, StoreError> {
        Ok(self.pool().get_mut(&track_id).map(|song| {
            song.genres = genres;
            song.clone()
        }))
    }

    async fn remove_song(&self, track_id: u64) -> Result<bool, StoreError> {
        Ok(self.pool().remove(&track_id).is_some())
    }

    async fn set_preview_failed_on(
        &self,
        track_id: u64,
        day: Option<Date>,
    ) -> Result<bool, StoreError> {
        Ok(self
            .pool()
            .get_mut(&track_id)
            .map(|song| song.preview_failed_on = day)
            .is_some())
    }

    async fn game(
        &self,
        player: &PlayerId,
        section: Section,
        day: Date,
    ) -> Result<Option<GameState>, StoreError> {
        Ok(self.played().get(&(player.clone(), section, day)).cloned())
    }

    async fn save_game(
        &self,
        player: &PlayerId,
        section: Section,
        game: &GameState,
    ) -> Result<(), StoreError> {
        self.played()
            .insert((player.clone(), section, game.day()), game.clone());
        Ok(())
    }

    async fn games(
        &self,
        player: &PlayerId,
        section: Section,
    ) -> Result<Vec<GameState>, StoreError> {
        Ok(self
            .played()
            .iter()
            .filter(|((of, in_section, _), _)| of == player && *in_section == section)
            .map(|(_, game)| game.clone())
            .collect())
    }

    async fn delete_player(&self, player: &PlayerId) -> Result<usize, StoreError> {
        let mut games = self.played();
        let before = games.len();
        games.retain(|(of, _, _), _| of != player);
        Ok(before - games.len())
    }

    async fn delete_games(&self, section: Section, day: Date) -> Result<usize, StoreError> {
        let mut games = self.played();
        let before = games.len();
        games.retain(|(_, in_section, on), _| !(*in_section == section && *on == day));
        Ok(before - games.len())
    }

    async fn picks_on(&self, day: Date) -> Result<Vec<Pick>, StoreError> {
        Ok(self
            .picked()
            .iter()
            .filter(|((on, _), _)| *on == day)
            .map(pick_of)
            .collect())
    }

    async fn pick_history(&self, section: Section) -> Result<Vec<Pick>, StoreError> {
        Ok(self
            .picked()
            .iter()
            .filter(|((_, of), _)| *of == section)
            .map(pick_of)
            .collect())
    }

    async fn save_pick(&self, pick: Pick) -> Result<Pick, StoreError> {
        let mut picks = self.picked();
        let track_id = *picks
            .entry((pick.day, pick.section))
            .or_insert(pick.track_id);
        Ok(Pick { track_id, ..pick })
    }

    async fn remove_pick(&self, day: Date, section: Section) -> Result<bool, StoreError> {
        Ok(self.picked().remove(&(day, section)).is_some())
    }

    async fn day_offset(&self) -> Result<i64, StoreError> {
        Ok(*self.offset())
    }

    async fn set_day_offset(&self, days: i64) -> Result<(), StoreError> {
        *self.offset() = days;
        Ok(())
    }

    async fn wipe_games_and_picks(&self) -> Result<(), StoreError> {
        // Games first: see the trait.
        self.played().clear();
        self.picked().clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::contract::{Fixture, contract_tests};

    contract_tests!(Fixture::new(MemoryStore::new()));

    #[tokio::test]
    async fn every_new_store_starts_empty() {
        let first = MemoryStore::new();
        first
            .add_song(
                NewSong {
                    track_id: 1,
                    title: "Song".to_owned(),
                    title_short: "Song".to_owned(),
                    artist: "Someone".to_owned(),
                    album: "LP".to_owned(),
                },
                Genres::new(),
            )
            .await
            .unwrap();
        let player = PlayerId::generate();
        let game = GameState::new(jiff::civil::date(2026, 10, 1));
        first
            .save_game(&player, Section::General, &game)
            .await
            .unwrap();

        // Nothing is shared between two stores, and nothing outlives one.
        let second = MemoryStore::new();
        assert!(second.songs().await.unwrap().is_empty());
        assert!(
            second
                .games(&player, Section::General)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(first.songs().await.unwrap().len(), 1);
        assert_eq!(
            first.games(&player, Section::General).await.unwrap(),
            vec![game]
        );
    }
}
