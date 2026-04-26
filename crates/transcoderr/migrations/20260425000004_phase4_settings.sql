CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT INTO settings (key, value) VALUES
  ('auth.enabled', 'false'),
  ('auth.password_hash', ''),
  ('worker.pool_size', '2'),
  ('retention.events_days', '30'),
  ('retention.jobs_days', '90'),
  ('dedup.window_seconds', '300');

CREATE TABLE sessions (
  id          TEXT PRIMARY KEY,
  created_at  INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL
);
