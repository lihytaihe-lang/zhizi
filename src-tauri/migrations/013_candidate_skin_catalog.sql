CREATE TABLE project_runtime_settings_v13 (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  active_profile_id TEXT REFERENCES project_profiles(id) ON DELETE SET NULL,
  secondary_layer_enabled INTEGER NOT NULL DEFAULT 1
    CHECK (secondary_layer_enabled IN (0, 1)),
  prediction_enabled INTEGER NOT NULL DEFAULT 0
    CHECK (prediction_enabled IN (0, 1)),
  candidate_theme TEXT NOT NULL DEFAULT 'system'
);

INSERT INTO project_runtime_settings_v13 (
  singleton,
  active_profile_id,
  secondary_layer_enabled,
  prediction_enabled,
  candidate_theme
)
SELECT
  singleton,
  active_profile_id,
  secondary_layer_enabled,
  prediction_enabled,
  candidate_theme
FROM project_runtime_settings;

DROP TABLE project_runtime_settings;
ALTER TABLE project_runtime_settings_v13 RENAME TO project_runtime_settings;
