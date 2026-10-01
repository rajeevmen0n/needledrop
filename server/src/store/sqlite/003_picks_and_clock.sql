-- Schema version 3: the daily picks and the day clock.
--
-- A pick is the track a section plays on a day. It is written once, the first
-- time that day is asked for, and from then on it is the day's song: a restart
-- reads it back instead of drawing again. The rows of earlier days are the
-- history that keeps a section from repeating a song.
--
-- track_id deliberately has no foreign key to songs. A song can be removed
-- from the pool after it was played; the day it was played on, and the games
-- of that day, stay.

CREATE TABLE picks (
  day      TEXT NOT NULL,   -- the server's day (YYYY-MM-DD): the UTC date plus the day offset
  section  TEXT NOT NULL CHECK (section IN ('general', 'pop', 'rock', 'hip-hop')),
  track_id INTEGER NOT NULL,   -- Deezer track ID
  PRIMARY KEY (day, section)
);

-- One row at most: how many days the server's day is ahead of the real UTC
-- date (negative: behind). No row means 0. The admin's "Simulate next day"
-- adds one to it and "Reset to day 1" sets it to whatever makes today the
-- launch date.

CREATE TABLE clock (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  day_offset INTEGER NOT NULL
);
