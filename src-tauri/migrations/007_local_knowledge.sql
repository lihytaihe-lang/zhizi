CREATE TABLE knowledge_sources (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  source_type TEXT NOT NULL CHECK (source_type IN ('local_path', 'notion')),
  authorized_scope TEXT NOT NULL,
  authorization TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('authorized', 'ready', 'error', 'revoked', 'deferred')),
  last_error TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);

CREATE TABLE knowledge_documents (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES knowledge_sources(id) ON DELETE CASCADE,
  relative_path TEXT NOT NULL,
  format TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  byte_count INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  UNIQUE(source_id, relative_path)
);

CREATE TABLE knowledge_chunks (
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL REFERENCES knowledge_documents(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  content TEXT NOT NULL,
  UNIQUE(document_id, ordinal)
);

CREATE TABLE knowledge_terms (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES knowledge_sources(id) ON DELETE CASCADE,
  chunk_id TEXT NOT NULL REFERENCES knowledge_chunks(id) ON DELETE CASCADE,
  text TEXT NOT NULL,
  pinyin TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL,
  relation TEXT NOT NULL DEFAULT '',
  confidence_millis INTEGER NOT NULL CHECK (confidence_millis BETWEEN 0 AND 1000),
  review_state TEXT NOT NULL CHECK (review_state IN ('pending', 'approved', 'rejected')),
  fixed INTEGER NOT NULL DEFAULT 0 CHECK (fixed IN (0, 1)),
  last_verified_ms INTEGER NOT NULL,
  UNIQUE(source_id, text)
);

CREATE INDEX idx_knowledge_documents_source ON knowledge_documents(source_id);
CREATE INDEX idx_knowledge_chunks_document ON knowledge_chunks(document_id);
CREATE INDEX idx_knowledge_terms_review ON knowledge_terms(review_state, text);
