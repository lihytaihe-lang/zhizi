CREATE TABLE IF NOT EXISTS lexicon_entries (
  id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
  text TEXT NOT NULL CHECK (length(trim(text)) > 0),
  pinyin TEXT NOT NULL CHECK (length(trim(pinyin)) > 0),
  base_weight INTEGER NOT NULL DEFAULT 0,
  fixed INTEGER NOT NULL DEFAULT 0 CHECK (fixed IN (0, 1)),
  disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  UNIQUE (text, pinyin)
);

CREATE INDEX IF NOT EXISTS idx_lexicon_entries_text_pinyin
  ON lexicon_entries(text, pinyin);

CREATE TABLE IF NOT EXISTS lexicon_aliases (
  entry_id TEXT NOT NULL REFERENCES lexicon_entries(id) ON DELETE CASCADE,
  alias TEXT NOT NULL CHECK (length(trim(alias)) > 0),
  PRIMARY KEY (entry_id, alias)
);

CREATE TABLE IF NOT EXISTS professional_packs (
  id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
  name TEXT NOT NULL UNIQUE CHECK (length(trim(name)) > 0)
);

CREATE TABLE IF NOT EXISTS professional_pack_entries (
  pack_id TEXT NOT NULL REFERENCES professional_packs(id) ON DELETE CASCADE,
  entry_id TEXT NOT NULL REFERENCES lexicon_entries(id) ON DELETE CASCADE,
  PRIMARY KEY (pack_id, entry_id)
);

CREATE INDEX IF NOT EXISTS idx_professional_pack_entries_entry
  ON professional_pack_entries(entry_id);

CREATE TABLE IF NOT EXISTS project_domains (
  id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
  name TEXT NOT NULL UNIQUE CHECK (length(trim(name)) > 0)
);

CREATE TABLE IF NOT EXISTS lexicon_entry_domains (
  entry_id TEXT NOT NULL REFERENCES lexicon_entries(id) ON DELETE CASCADE,
  domain_id TEXT NOT NULL REFERENCES project_domains(id) ON DELETE CASCADE,
  PRIMARY KEY (entry_id, domain_id)
);

CREATE INDEX IF NOT EXISTS idx_lexicon_entry_domains_domain
  ON lexicon_entry_domains(domain_id);

CREATE TABLE IF NOT EXISTS lexicon_sources (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  entry_id TEXT NOT NULL REFERENCES lexicon_entries(id) ON DELETE CASCADE,
  source_kind TEXT NOT NULL CHECK (length(trim(source_kind)) > 0),
  source TEXT NOT NULL CHECK (length(trim(source)) > 0),
  license TEXT NOT NULL CHECK (length(trim(license)) > 0),
  confidence_millis INTEGER NOT NULL DEFAULT 1000
    CHECK (confidence_millis BETWEEN 0 AND 1000),
  UNIQUE (entry_id, source_kind, source, license)
);

CREATE INDEX IF NOT EXISTS idx_lexicon_sources_entry
  ON lexicon_sources(entry_id);
