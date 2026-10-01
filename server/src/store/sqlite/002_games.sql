-- Schema version 2: the players' games.
--
-- One row per player, section and day, written with the first move of that
-- game and replaced by every later one. A visit without a move stores nothing.
--
-- There is no table of players. A player is an anonymous ID that lives in an
-- encrypted cookie, and exists here only as the rows that carry it; the stats
-- are computed from those rows and are not stored.

CREATE TABLE games (
  player  TEXT NOT NULL,   -- anonymous player ID: 32 lowercase hexadecimal characters
  section TEXT NOT NULL CHECK (section IN ('general', 'pop', 'rock', 'hip-hop')),
  day     TEXT NOT NULL,   -- UTC date of the game (YYYY-MM-DD)
  state   TEXT NOT NULL,   -- the game as JSON: {"day":"…","attempts":[…],"status":"playing"|"won"|"lost"}
  PRIMARY KEY (player, section, day)
);
