ALTER TABLE lexicon_entries
  ADD COLUMN demoted INTEGER NOT NULL DEFAULT 0 CHECK (demoted IN (0, 1));

CREATE TABLE IF NOT EXISTS lexicon_import_batches (
  id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
  created_at_ms INTEGER NOT NULL,
  undone_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS lexicon_import_batch_entries (
  batch_id TEXT NOT NULL REFERENCES lexicon_import_batches(id) ON DELETE CASCADE,
  entry_id TEXT NOT NULL REFERENCES lexicon_entries(id) ON DELETE CASCADE,
  PRIMARY KEY (batch_id, entry_id)
);
