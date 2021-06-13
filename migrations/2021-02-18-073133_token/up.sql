CREATE TABLE IF NOT EXISTS token (
  id INTEGER PRIMARY KEY NOT NULL,
  path TEXT NOT NULL,
  status TEXT CHECK(status in ("FRESH", "USED", "EXPIRED", "DELETED")) NOT NULL,
  max_size INTEGER,
  created_at DATETIME NOT NULL DEFAULT (datetime('now')),
  token_expires_at DATETIME NOT NULL,
  content_expires_at DATETIME,
  content_expires_after_hours INTEGER,
  deleted_at DATETIME
)
