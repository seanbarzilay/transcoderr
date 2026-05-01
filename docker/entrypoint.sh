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

# Optional: install extra runtimes declared via TRANSCODERR_RUNTIMES.
# Operators set this when a plugin in their catalog declares runtimes
# (e.g. python3, nodejs) the base image doesn't ship. Format is
# comma- or space-separated apt package names. The install is repeated
# on every boot -- no persistence -- so a freshly-pulled image always
# matches the env-var declaration.
#
# Trade-off: ~10-60s added to every container start depending on which
# runtimes you ask for. Empty / unset = no-op, default behavior.
#
# Names are passed verbatim to `apt-get install` and validated against
# a conservative pattern (alphanum + dash + dot + plus) to keep someone
# who controls TRANSCODERR_RUNTIMES from also controlling apt's
# argv (e.g. injecting "-c" or shell metachars).
if [ -n "${TRANSCODERR_RUNTIMES:-}" ]; then
  pkgs=$(echo "$TRANSCODERR_RUNTIMES" | tr ',' ' ')
  for p in $pkgs; do
    case "$p" in
      *[!a-zA-Z0-9.+-]*|"")
        echo "transcoderr: refusing to install runtime $p: invalid package name" >&2
        exit 1
        ;;
    esac
  done
  echo "transcoderr: installing runtimes from TRANSCODERR_RUNTIMES: $pkgs"
  apt-get update -y
  # shellcheck disable=SC2086  # intentional word-splitting on $pkgs
  apt-get install -y --no-install-recommends $pkgs
  rm -rf /var/lib/apt/lists/*
  echo "transcoderr: runtimes installed"
fi

exec /usr/local/bin/transcoderr "$@"
