CREATE TABLE plugin_catalogs (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    url             TEXT NOT NULL,
    auth_header     TEXT,
    priority        INTEGER NOT NULL DEFAULT 0,
    last_fetched_at INTEGER,
    last_error      TEXT,
    created_at      INTEGER NOT NULL
);

ALTER TABLE plugins ADD COLUMN catalog_id INTEGER;
ALTER TABLE plugins ADD COLUMN tarball_sha256 TEXT;

INSERT INTO plugin_catalogs (name, url, priority, created_at)
VALUES (
    'transcoderr official',
    'https://raw.githubusercontent.com/seanbarzilay/transcoderr-plugins/main/index.json',
    0,
    strftime('%s', 'now')
);
