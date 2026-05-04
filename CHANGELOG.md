# Changelog

Notable, operator-facing changes only. For everything else, the git log is canonical.

## v0.39.2 — 2026-05-03

- **Renamed** `worker.pool_size` setting to `runs.max_concurrent`. Migration preserves any operator-customised value; no manual action needed.

## v0.39.1 — 2026-05-03

- **Workers page** now shows the actual hardware (NVENC, VAAPI, etc.) for every worker, not just "software only".
- The local worker can no longer be disabled from the UI — toggling it off would silently halt all coordinator-side processing.

## v0.39.0 — 2026-05-03

- **Per-worker path mappings.** Workers page gains an **Edit mappings** modal per remote worker. Configure prefix pairs (e.g. coordinator `/mnt/movies` → worker `/data/media/movies`) and the dispatcher rewrites paths in both directions on the wire. Workers and plugins are unchanged. Homogeneous-mount setups continue to work unchanged because the default (no mappings) is identity translation.

## v0.38.0 — 2026-05-03

- **Worker auto-discovery (mDNS).** A worker started with no `worker.toml` discovers the coordinator via mDNS on `_transcoderr._tcp.local.`, enrolls itself for a fresh token, persists `/var/lib/transcoderr/worker.toml`, and connects. One command on the worker host. Same-LAN only; manual `worker.toml` remains the option for WAN / VPN / Docker default-bridge deployments.
- New env var `TRANSCODERR_DISCOVERY=disabled` skips the responder on the coordinator side.

## v0.37.0 — 2026-05-02

- **Cancel propagation.** Operator-initiated job cancel now flows from the coordinator UI all the way to the remote worker's ffmpeg child within ~1 second.
- Closes the 6-piece distributed-transcoding roadmap (issue #79). The dispatcher routes per-step work between local and remote workers; plugin steps marked `run_on: any-worker` are eligible for remote dispatch; cancel + plugin push + per-step routing all work end-to-end.

## v0.32.0 — v0.36.0

- Distributed-transcoding pieces 1–5 (wire protocol, local-worker abstraction, per-step routing for built-ins, plugin push, plugin steps remote-eligible). The connection layer was added in v0.31; routing landed across this range. Operators upgrading from any of these versions can skip straight to v0.39.x — the wire schema is forward-compatible and the migrations chain cleanly.
