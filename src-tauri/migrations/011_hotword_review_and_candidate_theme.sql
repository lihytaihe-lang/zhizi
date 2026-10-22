ALTER TABLE knowledge_terms
  ADD COLUMN definition TEXT NOT NULL DEFAULT '';

ALTER TABLE knowledge_terms
  ADD COLUMN candidate_layer TEXT NOT NULL DEFAULT 'first'
  CHECK (candidate_layer IN ('first', 'secondary'));

ALTER TABLE project_runtime_settings
  ADD COLUMN candidate_theme TEXT NOT NULL DEFAULT 'system'
  CHECK (candidate_theme IN ('system', 'midnight'));

CREATE TABLE dynamic_sources_v11 (
  id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
  display_name TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
  source_type TEXT NOT NULL CHECK (source_type IN ('official_json', 'official_xml', 'local_text')),
  refresh_interval_minutes INTEGER NOT NULL CHECK (refresh_interval_minutes >= 5),
  authorization TEXT NOT NULL DEFAULT '',
  terms_url TEXT NOT NULL DEFAULT '',
  license TEXT NOT NULL DEFAULT '',
  cache_policy TEXT NOT NULL CHECK (length(trim(cache_policy)) > 0),
  ownership TEXT NOT NULL CHECK (ownership IN ('bundled', 'user_local', 'remote_cached', 'self-authored')),
  distribution TEXT NOT NULL CHECK (length(trim(distribution)) > 0),
  disable_policy TEXT NOT NULL CHECK (length(trim(disable_policy)) > 0),
  endpoint TEXT NOT NULL DEFAULT '',
  enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
  opt_in INTEGER NOT NULL DEFAULT 0 CHECK (opt_in IN (0, 1)),
  last_refresh_ms INTEGER,
  last_error TEXT,
  created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
  updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
);

INSERT INTO dynamic_sources_v11
SELECT id,display_name,source_type,refresh_interval_minutes,authorization,terms_url,license,
       cache_policy,ownership,distribution,disable_policy,endpoint,enabled,opt_in,last_refresh_ms,
       last_error,created_at_ms,updated_at_ms
FROM dynamic_sources;

CREATE TABLE hotword_item_sources_v11 (
  item_id TEXT NOT NULL REFERENCES hotword_items(id) ON DELETE CASCADE,
  source_id TEXT NOT NULL REFERENCES dynamic_sources_v11(id) ON DELETE CASCADE,
  source_record_id TEXT NOT NULL CHECK (length(trim(source_record_id)) > 0),
  fetched_at_ms INTEGER NOT NULL CHECK (fetched_at_ms >= 0),
  PRIMARY KEY (item_id, source_id, source_record_id)
);

INSERT INTO hotword_item_sources_v11
SELECT item_id,source_id,source_record_id,fetched_at_ms
FROM hotword_item_sources;

DROP TABLE hotword_item_sources;
DROP TABLE dynamic_sources;
ALTER TABLE dynamic_sources_v11 RENAME TO dynamic_sources;
ALTER TABLE hotword_item_sources_v11 RENAME TO hotword_item_sources;
CREATE INDEX idx_hotword_source ON hotword_item_sources(source_id);
