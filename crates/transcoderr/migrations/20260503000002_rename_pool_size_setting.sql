-- Rename `worker.pool_size` → `runs.max_concurrent`. The setting was
-- always poorly named: it caps the number of concurrent flow runs the
-- coordinator processes in parallel (one job per claim loop), not a
-- "pool" of workers. The new name matches `Device.max_concurrent`
-- terminology already used in the hardware-semaphore layer.
--
-- Preserves any operator-set value via UPDATE (rather than re-seeding,
-- which would clobber a non-default value). The INSERT OR IGNORE is a
-- defensive safety net for databases where the legacy row was removed.
UPDATE settings SET key = 'runs.max_concurrent' WHERE key = 'worker.pool_size';
INSERT OR IGNORE INTO settings (key, value) VALUES ('runs.max_concurrent', '2');
