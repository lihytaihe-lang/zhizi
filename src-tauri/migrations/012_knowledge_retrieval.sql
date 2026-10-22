ALTER TABLE knowledge_sources
  ADD COLUMN project_id TEXT NOT NULL DEFAULT '';

ALTER TABLE knowledge_sources
  ADD COLUMN retrieval_status TEXT NOT NULL DEFAULT 'pending'
  CHECK (retrieval_status IN ('pending', 'approved'));

ALTER TABLE knowledge_sources
  ADD COLUMN sensitive_enabled INTEGER NOT NULL DEFAULT 1
  CHECK (sensitive_enabled IN (0, 1));

UPDATE knowledge_sources
SET project_id = id
WHERE length(trim(project_id)) = 0;

UPDATE knowledge_sources
SET retrieval_status = CASE
  WHEN status = 'ready' AND length(trim(authorized_scope)) > 0 THEN 'approved'
  ELSE 'pending'
END;

CREATE INDEX idx_knowledge_sources_retrieval
  ON knowledge_sources(project_id, retrieval_status, sensitive_enabled, status);

CREATE VIRTUAL TABLE knowledge_chunks_fts USING fts5(
  content,
  tokenize = 'unicode61'
);

CREATE TRIGGER knowledge_chunks_fts_insert
AFTER INSERT ON knowledge_chunks
BEGIN
  INSERT INTO knowledge_chunks_fts(rowid, content)
  VALUES (new.rowid, new.content);
END;

CREATE TRIGGER knowledge_chunks_fts_update
AFTER UPDATE OF content ON knowledge_chunks
BEGIN
  DELETE FROM knowledge_chunks_fts WHERE rowid = old.rowid;
  INSERT INTO knowledge_chunks_fts(rowid, content)
  VALUES (new.rowid, new.content);
END;

CREATE TRIGGER knowledge_chunks_fts_delete
AFTER DELETE ON knowledge_chunks
BEGIN
  DELETE FROM knowledge_chunks_fts WHERE rowid = old.rowid;
END;

INSERT INTO knowledge_chunks_fts(rowid, content)
SELECT rowid, content
FROM knowledge_chunks;
