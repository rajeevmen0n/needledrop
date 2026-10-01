-- Schema version 1: the song pool.
--
-- Only Deezer track IDs and display metadata are stored. No audio and no
-- preview URL: previews are signed, expire in minutes, and are downloaded
-- when a song is picked for a day (cached under data/audio/).
--
-- IF NOT EXISTS is deliberate, and only this first migration has it. Before
-- the server created its own database, one was built by hand from a seed file
-- with these two tables and no schema version; this adopts it as it is.

CREATE TABLE IF NOT EXISTS songs (
  track_id          INTEGER PRIMARY KEY,           -- Deezer track ID
  title             TEXT NOT NULL,
  title_short       TEXT NOT NULL,
  artist            TEXT NOT NULL,
  album             TEXT NOT NULL,
  added_at          TEXT NOT NULL DEFAULT (datetime('now')),  -- UTC; for a person reading the file, the server does not use it
  preview_failed_on TEXT                            -- UTC date (YYYY-MM-DD) of the last failed daily check, if any
);

-- Zero or more genre tags per song. Every song is in the General pool.
CREATE TABLE IF NOT EXISTS song_genres (
  track_id INTEGER NOT NULL REFERENCES songs(track_id) ON DELETE CASCADE,
  genre    TEXT NOT NULL CHECK (genre IN ('pop', 'rock', 'hip-hop')),
  PRIMARY KEY (track_id, genre)
);
