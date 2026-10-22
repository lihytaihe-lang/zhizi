CREATE TABLE learning_preferences (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  app_learning INTEGER NOT NULL DEFAULT 1 CHECK (app_learning IN (0,1)),
  project_learning INTEGER NOT NULL DEFAULT 1 CHECK (project_learning IN (0,1)),
  secure_input_learning INTEGER NOT NULL DEFAULT 0 CHECK (secure_input_learning = 0),
  updated_at_ms INTEGER NOT NULL
);

INSERT INTO learning_preferences (singleton, updated_at_ms) VALUES (1, 0);

CREATE TABLE learning_cleanup_batches (
  id TEXT PRIMARY KEY,
  entries_json TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  undone_at_ms INTEGER
);
