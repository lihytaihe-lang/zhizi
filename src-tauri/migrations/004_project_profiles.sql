CREATE TABLE IF NOT EXISTS project_profiles (
  id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
  name TEXT NOT NULL CHECK (length(trim(name)) > 0),
  project_domain TEXT NOT NULL CHECK (length(trim(project_domain)) > 0),
  agent_context TEXT NOT NULL DEFAULT '',
  created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
  updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE TABLE IF NOT EXISTS project_profile_packs (
  profile_id TEXT NOT NULL REFERENCES project_profiles(id) ON DELETE CASCADE,
  pack_id TEXT NOT NULL REFERENCES professional_packs(id) ON DELETE CASCADE,
  PRIMARY KEY (profile_id, pack_id)
);

CREATE TABLE IF NOT EXISTS project_runtime_settings (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  active_profile_id TEXT REFERENCES project_profiles(id) ON DELETE SET NULL,
  secondary_layer_enabled INTEGER NOT NULL DEFAULT 1
    CHECK (secondary_layer_enabled IN (0, 1))
);

INSERT INTO project_runtime_settings (singleton, active_profile_id, secondary_layer_enabled)
VALUES (1, NULL, 1)
ON CONFLICT(singleton) DO NOTHING;
