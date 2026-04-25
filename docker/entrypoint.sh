#!/bin/sh
set -eu

# On first boot, seed /data/config.toml from the bundled example so the
# container starts cleanly with just `-v <host>:/data` and no extra setup.
if [ ! -f /data/config.toml ]; then
  if [ -f /app/config.example.toml ]; then
    mkdir -p /data
    sed 's|^data_dir = .*|data_dir = "/data"|' /app/config.example.toml > /data/config.toml
    echo "transcoderr: seeded /data/config.toml from example"
  else
    echo "transcoderr: no /data/config.toml and no example bundled; refusing to start" >&2
    exit 1
  fi
fi

exec /usr/local/bin/transcoderr "$@"
