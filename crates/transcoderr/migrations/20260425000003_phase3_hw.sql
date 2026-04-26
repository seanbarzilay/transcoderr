-- Phase 3: hardware capabilities snapshot (single-row config table)
CREATE TABLE IF NOT EXISTS hw_capabilities (
  id           INTEGER PRIMARY KEY,   -- always 1 (singleton)
  probed_at    INTEGER NOT NULL,
  devices_json TEXT NOT NULL          -- serialized HwCaps JSON
);
