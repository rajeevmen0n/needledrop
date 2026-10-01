-- Schema version 4: the players' random games.
--
-- Random mode is one endless game per player: one song after another, with no
-- day and no section. One row per player, written when they start a session
-- and replaced by every move, every next song and every new session. A visit
-- stores nothing.
--
-- The row goes with the player's daily games when they clear their data, and
-- with everyone's on a reset to day 1.

CREATE TABLE random_games (
  player TEXT NOT NULL PRIMARY KEY,   -- anonymous player ID: 32 lowercase hexadecimal characters
  state  TEXT NOT NULL                -- the game as JSON: {"round":…,"track_id":…,"game":{"day":"…","attempts":[…],"status":"…"},"run":…,"played":…,"won":…,"best_run":…,"recent":[…]}
);
