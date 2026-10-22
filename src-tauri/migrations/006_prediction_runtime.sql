ALTER TABLE project_runtime_settings
  ADD COLUMN prediction_enabled INTEGER NOT NULL DEFAULT 0
  CHECK (prediction_enabled IN (0, 1));

ALTER TABLE hotword_items
  ADD COLUMN exposure_count INTEGER NOT NULL DEFAULT 0 CHECK (exposure_count >= 0);

ALTER TABLE hotword_items
  ADD COLUMN misselection_count INTEGER NOT NULL DEFAULT 0 CHECK (misselection_count >= 0);
