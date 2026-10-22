CREATE TABLE IF NOT EXISTS history (
  id TEXT PRIMARY KEY,
  raw_input TEXT NOT NULL,
  mode_id TEXT NOT NULL,
  target_tool TEXT,
  output TEXT NOT NULL,
  favorite INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  position INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_history_position ON history(position);
CREATE INDEX IF NOT EXISTS idx_history_created_at ON history(created_at);

CREATE TABLE IF NOT EXISTS user_rules (
  id TEXT PRIMARY KEY,
  text TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  position INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_user_rules_position ON user_rules(position);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
