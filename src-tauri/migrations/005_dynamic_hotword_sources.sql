CREATE TABLE IF NOT EXISTS dynamic_sources (
  id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
  display_name TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
  source_type TEXT NOT NULL CHECK (source_type IN ('official_json', 'official_xml', 'local_text')),
  refresh_interval_minutes INTEGER NOT NULL CHECK (refresh_interval_minutes >= 5),
  authorization TEXT NOT NULL DEFAULT '',
  terms_url TEXT NOT NULL DEFAULT '',
  license TEXT NOT NULL DEFAULT '',
  cache_policy TEXT NOT NULL CHECK (length(trim(cache_policy)) > 0),
  ownership TEXT NOT NULL CHECK (ownership IN ('bundled', 'user_local', 'remote_cached')),
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

CREATE TABLE IF NOT EXISTS hotword_items (
  id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
  text TEXT NOT NULL CHECK (length(trim(text)) > 0),
  pinyin TEXT NOT NULL DEFAULT '',
  confidence_millis INTEGER NOT NULL CHECK (confidence_millis BETWEEN 0 AND 1000),
  freshness_millis INTEGER NOT NULL CHECK (freshness_millis BETWEEN 0 AND 1000),
  domain TEXT NOT NULL DEFAULT '',
  expires_at_ms INTEGER,
  review_required INTEGER NOT NULL DEFAULT 1 CHECK (review_required IN (0, 1)),
  state TEXT NOT NULL DEFAULT 'staged' CHECK (state IN ('staged', 'review', 'expired', 'blocked')),
  created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
  updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
  UNIQUE(text, pinyin)
);

CREATE TABLE IF NOT EXISTS hotword_item_sources (
  item_id TEXT NOT NULL REFERENCES hotword_items(id) ON DELETE CASCADE,
  source_id TEXT NOT NULL REFERENCES dynamic_sources(id) ON DELETE CASCADE,
  source_record_id TEXT NOT NULL CHECK (length(trim(source_record_id)) > 0),
  fetched_at_ms INTEGER NOT NULL CHECK (fetched_at_ms >= 0),
  PRIMARY KEY (item_id, source_id, source_record_id)
);

CREATE INDEX IF NOT EXISTS idx_hotword_source ON hotword_item_sources(source_id);
CREATE INDEX IF NOT EXISTS idx_hotword_state ON hotword_items(state, expires_at_ms);
