-- crates/transcoderr/migrations/20260502000001_workers.sql

CREATE TABLE workers (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL,            -- 'local' | 'remote'
    secret_token TEXT,                     -- NULL for the local worker row
    hw_caps_json TEXT,                     -- last register payload
    plugin_manifest_json TEXT,             -- last register payload
    enabled      INTEGER NOT NULL DEFAULT 1,
    last_seen_at INTEGER,                  -- unix seconds; NULL = never
    created_at   INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_workers_secret_token
    ON workers(secret_token)
    WHERE secret_token IS NOT NULL;

ALTER TABLE jobs       ADD COLUMN worker_id INTEGER REFERENCES workers(id);
ALTER TABLE run_events ADD COLUMN worker_id INTEGER;

INSERT INTO workers (name, kind, enabled, created_at)
VALUES ('local', 'local', 1, strftime('%s', 'now'));
