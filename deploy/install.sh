#!/usr/bin/env bash
# Runs on the Pi (deploy.sh calls it): sudo bash upload/deploy/install.sh server web sensors units
# Safe to run again: the first time it also sets up the Pi, later it only updates the given parts.
set -euo pipefail
UP=$(cd "$(dirname "$0")/.." && pwd)
APP=/opt/garden

# --- one-time setup (each step is skipped if already done) ---
command -v sqlite3 >/dev/null || { apt-get update && apt-get install -y sqlite3; }
command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh
id garden >/dev/null 2>&1 || useradd --system --home-dir $APP --shell /usr/sbin/nologin garden
usermod -aG gpio,i2c garden # lets it use the sensors
mkdir -p $APP && chown garden:garden $APP
systemctl enable --quiet systemd-time-wait-sync # docs/adr/0001-wait-for-clock-sync.md

# --- the parts ---
restart=no
for part in "$@"; do
  case $part in
    server)
      # install, not cp: cp can't overwrite a program that is running
      install -o garden -g garden -m 755 "$UP/server/target/aarch64-unknown-linux-musl/release/garden" $APP/garden
      restart=yes ;;
    web)
      rm -rf $APP/web/dist && mkdir -p $APP/web && cp -r "$UP/web/dist" $APP/web/
      chown -R garden:garden $APP/web ;; # served straight from disk, no restart needed
    sensors)
      cp -r "$UP/sensors" $APP/ && chown -R garden:garden $APP/sensors
      (cd $APP/sensors && sudo -u garden -H uv sync)
      restart=yes ;;
    units)
      cp "$UP"/deploy/garden.service "$UP"/deploy/garden-backup.service "$UP"/deploy/garden-backup.timer /etc/systemd/system/
      systemctl daemon-reload
      systemctl enable --quiet garden.service garden-backup.timer
      systemctl start garden-backup.timer
      restart=yes ;;
    *) echo "unknown part: $part"; exit 1 ;;
  esac
done

[ $restart = yes ] && systemctl restart garden.service

# The app only listens on the Pi itself; Tailscale makes it reachable (docs/adr/0002-localhost-behind-tailscale-serve.md).
if command -v tailscale >/dev/null; then
  tailscale serve --bg 8080 >/dev/null || echo "warning: 'tailscale serve' failed; is Tailscale logged in? (sudo tailscale up)"
else
  echo "warning: Tailscale is not installed; see deploy/setup.md"
fi
echo "installed: $*"
