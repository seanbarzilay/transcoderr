# transcoderr

A push-driven, single-binary transcoder. Phase 1 ships a headless engine that:

- listens for Radarr download webhooks
- runs a linear `probe → transcode → output(replace)` flow against the file
- persists jobs and resumes from checkpoints across restarts

This is **Phase 1 of 5**. No web UI, no plugins, no GPU acceleration yet — see `docs/superpowers/specs/` for the full design and `docs/superpowers/plans/` for upcoming phases.

## Build

```
cargo build --release
```

## Configure

Copy `config.example.toml` to `config.toml` and edit. The Radarr bearer token must match the `Authorization: Bearer …` header your Radarr install will send (configure under Settings → Connect → Webhook).

## Run

```
./target/release/transcoderr serve --config config.toml
```

Then seed a flow into the DB (Phase 2 adds a CLI / UI for this) and POST a Radarr webhook at it.
