CREATE TABLE personal_profile_auto_updates (
  id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
  entry_id TEXT NOT NULL CHECK (length(trim(entry_id)) > 0),
  pinyin TEXT NOT NULL CHECK (length(trim(pinyin)) > 0),
  text TEXT NOT NULL CHECK (length(trim(text)) > 0),
  friction_count INTEGER NOT NULL CHECK (friction_count > 0),
  runner_up_friction_count INTEGER NOT NULL CHECK (runner_up_friction_count >= 0),
  latest_friction_at_ms INTEGER NOT NULL CHECK (latest_friction_at_ms >= 0),
  applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0),
  last_refreshed_at_ms INTEGER NOT NULL CHECK (last_refreshed_at_ms >= 0),
  status TEXT NOT NULL CHECK (status IN ('active', 'decayed', 'revoked', 'blacklisted')),
  status_changed_at_ms INTEGER NOT NULL CHECK (status_changed_at_ms >= 0),
  baseline_entry_json TEXT,
  applied_entry_json TEXT NOT NULL,
  prediction_eligible INTEGER NOT NULL DEFAULT 1 CHECK (prediction_eligible IN (0, 1)),
  UNIQUE (pinyin, text)
);

CREATE INDEX idx_personal_profile_auto_updates_status
  ON personal_profile_auto_updates(status, pinyin, text);
