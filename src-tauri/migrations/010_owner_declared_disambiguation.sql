ALTER TABLE personal_profile_auto_updates RENAME TO personal_profile_auto_updates_v9;

CREATE TABLE personal_profile_auto_updates (
  id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
  entry_id TEXT NOT NULL CHECK (length(trim(entry_id)) > 0),
  pinyin TEXT NOT NULL CHECK (length(trim(pinyin)) > 0),
  text TEXT NOT NULL CHECK (length(trim(text)) > 0),
  friction_count INTEGER NOT NULL CHECK (friction_count >= 0),
  runner_up_friction_count INTEGER NOT NULL CHECK (runner_up_friction_count >= 0),
  latest_friction_at_ms INTEGER NOT NULL CHECK (latest_friction_at_ms >= 0),
  applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0),
  last_refreshed_at_ms INTEGER NOT NULL CHECK (last_refreshed_at_ms >= 0),
  status TEXT NOT NULL CHECK (status IN ('active', 'decayed', 'revoked', 'blacklisted')),
  status_changed_at_ms INTEGER NOT NULL CHECK (status_changed_at_ms >= 0),
  baseline_entry_json TEXT,
  applied_entry_json TEXT NOT NULL,
  prediction_eligible INTEGER NOT NULL DEFAULT 1 CHECK (prediction_eligible IN (0, 1)),
  evidence_kind TEXT NOT NULL DEFAULT 'friction' CHECK (
    evidence_kind IN ('friction', 'owner_declared_disambiguation')
  ),
  evidence_json TEXT,
  CHECK (
    (evidence_kind = 'friction' AND friction_count > 0 AND
      friction_count >= runner_up_friction_count * 2 AND evidence_json IS NULL)
    OR
    (evidence_kind = 'owner_declared_disambiguation' AND friction_count = 0 AND
      runner_up_friction_count = 0 AND evidence_json IS NOT NULL)
  ),
  UNIQUE (pinyin, text)
);

INSERT INTO personal_profile_auto_updates (
  id, entry_id, pinyin, text, friction_count, runner_up_friction_count,
  latest_friction_at_ms, applied_at_ms, last_refreshed_at_ms, status,
  status_changed_at_ms, baseline_entry_json, applied_entry_json,
  prediction_eligible, evidence_kind, evidence_json
)
SELECT
  id, entry_id, pinyin, text, friction_count, runner_up_friction_count,
  latest_friction_at_ms, applied_at_ms, last_refreshed_at_ms, status,
  status_changed_at_ms, baseline_entry_json, applied_entry_json,
  prediction_eligible, 'friction', NULL
FROM personal_profile_auto_updates_v9;

DROP TABLE personal_profile_auto_updates_v9;

CREATE INDEX idx_personal_profile_auto_updates_status
  ON personal_profile_auto_updates(status, pinyin, text);
