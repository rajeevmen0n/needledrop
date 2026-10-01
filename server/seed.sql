-- Needledrop song pool: schema and the six seed songs (two per genre).
-- Only Deezer track IDs and display metadata are stored. No audio and no
-- preview URL: previews are signed, expire in minutes, and are downloaded
-- when a song is picked for a day (cached under data/audio/).
-- Build a database from this file with any SQLite client.

CREATE TABLE IF NOT EXISTS songs (
  track_id          INTEGER PRIMARY KEY,           -- Deezer track ID
  title             TEXT NOT NULL,
  title_short       TEXT NOT NULL,
  artist            TEXT NOT NULL,
  album             TEXT NOT NULL,
  added_at          TEXT NOT NULL DEFAULT (datetime('now')),
  preview_failed_on TEXT                            -- UTC date of the last failed daily check, if any
);

-- Zero or more genre tags per song. Every song is in the General pool.
CREATE TABLE IF NOT EXISTS song_genres (
  track_id INTEGER NOT NULL REFERENCES songs(track_id) ON DELETE CASCADE,
  genre    TEXT NOT NULL CHECK (genre IN ('pop', 'rock', 'hip-hop')),
  PRIMARY KEY (track_id, genre)
);

INSERT OR IGNORE INTO songs (track_id, title, title_short, artist, album) VALUES (4603408, 'Billie Jean', 'Billie Jean', 'Michael Jackson', 'Michael Jackson''s This Is It');
INSERT OR IGNORE INTO song_genres (track_id, genre) VALUES (4603408, 'pop');
INSERT OR IGNORE INTO songs (track_id, title, title_short, artist, album) VALUES (15391618, 'Toxic', 'Toxic', 'Britney Spears', 'In The Zone');
INSERT OR IGNORE INTO song_genres (track_id, genre) VALUES (15391618, 'pop');
INSERT OR IGNORE INTO songs (track_id, title, title_short, artist, album) VALUES (4091937401, 'Bohemian Rhapsody', 'Bohemian Rhapsody', 'Queen', 'A Night At The Opera');
INSERT OR IGNORE INTO song_genres (track_id, genre) VALUES (4091937401, 'rock');
INSERT OR IGNORE INTO songs (track_id, title, title_short, artist, album) VALUES (92720046, 'Back In Black', 'Back In Black', 'AC/DC', 'Back In Black');
INSERT OR IGNORE INTO song_genres (track_id, genre) VALUES (92720046, 'rock');
INSERT OR IGNORE INTO songs (track_id, title, title_short, artist, album) VALUES (1109731, 'Lose Yourself', 'Lose Yourself', 'Eminem', 'Curtain Call: The Hits');
INSERT OR IGNORE INTO song_genres (track_id, genre) VALUES (1109731, 'hip-hop');
INSERT OR IGNORE INTO songs (track_id, title, title_short, artist, album) VALUES (3616616, 'Juicy', 'Juicy', 'The Notorious B.I.G.', 'Greatest Hits');
INSERT OR IGNORE INTO song_genres (track_id, genre) VALUES (3616616, 'hip-hop');
