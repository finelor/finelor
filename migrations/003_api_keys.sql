-- API keys for public v1 endpoints.

CREATE TABLE IF NOT EXISTS api_keys (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  token TEXT NOT NULL UNIQUE,
  key_prefix TEXT NOT NULL,
  key_hash TEXT NOT NULL UNIQUE,
  created_by_user_id INTEGER NOT NULL REFERENCES users(id),
  last_used_at TEXT,
  revoked_at TEXT,
  hidden_at TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_api_keys_visible ON api_keys(hidden_at, revoked_at, created_at DESC);
