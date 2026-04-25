CREATE TABLE flows (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  enabled       INTEGER NOT NULL DEFAULT 1,
  yaml_source   TEXT NOT NULL,
  parsed_json   TEXT NOT NULL,
  version       INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

CREATE TABLE flow_versions (
  flow_id       INTEGER NOT NULL REFERENCES flows(id),
  version       INTEGER NOT NULL,
  yaml_source   TEXT NOT NULL,
  created_at    INTEGER NOT NULL,
  PRIMARY KEY (flow_id, version)
);

CREATE TABLE jobs (
  id                    INTEGER PRIMARY KEY,
  flow_id               INTEGER NOT NULL REFERENCES flows(id),
  flow_version          INTEGER NOT NULL,
  source_kind           TEXT NOT NULL,
  file_path             TEXT NOT NULL,
  trigger_payload_json  TEXT NOT NULL,
  status                TEXT NOT NULL,
  status_label          TEXT,
  priority              INTEGER NOT NULL DEFAULT 0,
  current_step          INTEGER,
  attempt               INTEGER NOT NULL DEFAULT 0,
  created_at            INTEGER NOT NULL,
  started_at            INTEGER,
  finished_at           INTEGER
);

CREATE INDEX idx_jobs_pending ON jobs(status, priority DESC, created_at)
  WHERE status='pending';

CREATE TABLE run_events (
  id            INTEGER PRIMARY KEY,
  job_id        INTEGER NOT NULL REFERENCES jobs(id),
  ts            INTEGER NOT NULL,
  step_id       TEXT,
  kind          TEXT NOT NULL,
  payload_json  TEXT,
  payload_path  TEXT
);

CREATE INDEX idx_run_events_job ON run_events(job_id, ts);

CREATE TABLE checkpoints (
  job_id                 INTEGER PRIMARY KEY REFERENCES jobs(id),
  step_index             INTEGER NOT NULL,
  context_snapshot_json  TEXT NOT NULL,
  updated_at             INTEGER NOT NULL
);
