CREATE TABLE sources (
  id            INTEGER PRIMARY KEY,
  kind          TEXT NOT NULL,           -- 'radarr'|'sonarr'|'lidarr'|'webhook'
  name          TEXT NOT NULL UNIQUE,
  config_json   TEXT NOT NULL,
  secret_token  TEXT NOT NULL
);

CREATE TABLE plugins (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  version       TEXT NOT NULL,
  kind          TEXT NOT NULL,           -- 'builtin'|'subprocess'
  path          TEXT,
  schema_json   TEXT NOT NULL,
  enabled       INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE notifiers (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  kind          TEXT NOT NULL,           -- 'discord'|'ntfy'|'webhook'
  config_json   TEXT NOT NULL
);

-- jobs table: add source_id FK (nullable for backfill, NOT NULL going forward)
ALTER TABLE jobs ADD COLUMN source_id INTEGER REFERENCES sources(id);
CREATE INDEX idx_jobs_dedup ON jobs(source_id, file_path, created_at);
