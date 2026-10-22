ALTER TABLE project_runtime_settings ADD COLUMN context_model_ranking_enabled INTEGER NOT NULL DEFAULT 0 CHECK(context_model_ranking_enabled IN (0, 1));
