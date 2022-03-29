-- keep things simple here, user/key id and phc string
-- ignore more complex auth states with reset & stuff
CREATE TABLE IF NOT EXISTS auth (
  id TEXT PRIMARY KEY NOT NULL,
  phc TEXT NOT NULL
)
