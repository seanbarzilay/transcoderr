# transcoderr

A push-driven, single-binary transcoder. A leaner replacement for tdarr aimed at homelab self-hosters.

- **Webhook-driven.** Radarr / Sonarr / Lidarr / generic JSON webhooks create jobs.
- **Configurable flows.** YAML source-of-truth, visual mirror, dry-run testing, plugin host.
- **Single binary.** Embedded SQLite, embedded SPA. One image, one volume.
- **Hardware aware.** NVENC / QSV / VAAPI / VideoToolbox probing, per-device concurrency limits, runtime CPU fallback.
- **Observable.** Per-run live logs, structured event timeline, Prometheus `/metrics`.

## Quickstart with Docker

```yaml
# docker-compose.yml
services:
  transcoderr:
    image: ghcr.io/your-org/transcoderr:cpu-latest
    restart: unless-stopped
    ports: ["8080:8080"]
    volumes:
      - ./data:/data
      - /mnt/movies:/mnt/movies
```

Then open `http://localhost:8080`. Add a source under Sources → Add, point Radarr's webhook at `http://your-host:8080/webhook/radarr` with the bearer token.

For NVIDIA / Intel acceleration, see [`docs/deploy.md`](docs/deploy.md).

## Build from source

```
npm --prefix web ci && npm --prefix web run build
cargo build --release
```

The binary at `target/release/transcoderr` embeds the compiled SPA.

## Documentation

- [Design spec](docs/superpowers/specs/2026-04-25-transcoderr-design.md)
- [Implementation plans (5 phases)](docs/superpowers/plans/)
- [Deploy guide](docs/deploy.md)

## License

(TBD by the project owner.)
