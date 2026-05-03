-- Per-worker path mapping rules (spec
-- 2026-05-03-worker-path-mappings-design.md). NULL = identity (current
-- behavior). Stores `[{"from": "...", "to": "..."}, ...]` for
-- kind='remote' rows; kind='local' rows must keep this NULL.
ALTER TABLE workers ADD COLUMN path_mappings_json TEXT;
