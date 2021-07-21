CREATE TABLE IF NOT EXISTS file (
  id INTEGER PRIMARY KEY NOT NULL,
  token_id INTEGER NOT NULL,
  name TEXT,
  path TEXT NOT NULL,
  content_type TEXT,
  size_mib INTEGER,
  created_at DATETIME NOT NULL DEFAULT (datetime('now')),
  deleted_at DATETIME,
  file_upload_status TEXT CHECK(file_upload_status in ("STARTED", "COMPLETED")) NOT NULL,
  FOREIGN KEY(token_id) REFERENCES token(id)
)
