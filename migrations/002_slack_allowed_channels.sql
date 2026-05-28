CREATE TABLE IF NOT EXISTS slack_allowed_channels (
  id INTEGER PRIMARY KEY,
  channel_id TEXT NOT NULL UNIQUE,
  channel_name TEXT NOT NULL,
  channel_type TEXT NOT NULL CHECK (channel_type IN ('public', 'private', 'im')),
  slack_user_id TEXT,
  slack_username TEXT,
  display_name TEXT,
  team_id TEXT,
  active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_slack_allowed_channels_active ON slack_allowed_channels(active);
CREATE INDEX IF NOT EXISTS idx_slack_allowed_channels_name ON slack_allowed_channels(channel_name);
CREATE INDEX IF NOT EXISTS idx_slack_allowed_channels_user ON slack_allowed_channels(slack_user_id);
