#!/usr/bin/env bash
# Build on your PC, copy to the Pi, install, and check it answers.
# Run from the project folder (Git Bash on Windows):
#   deploy/deploy.sh pi@<pi>                  # everything
#   deploy/deploy.sh pi@<pi> server web       # only some parts: server, web, sensors, units
set -euo pipefail
cd "$(dirname "$0")/.."

PI=${1:?usage: deploy/deploy.sh pi@<pi> [server] [web] [sensors] [units]}
shift
PARTS=("$@")
if [ ${#PARTS[@]} -eq 0 ]; then
  PARTS=(server sensors units)
  if [ -f web/package.json ]; then PARTS+=(web); else echo "no web/ yet, skipping the website"; fi
fi

FILES=(deploy)
for part in "${PARTS[@]}"; do
  case $part in
    server)
      (cd server && cargo zigbuild --release --target aarch64-unknown-linux-musl)
      FILES+=(server/target/aarch64-unknown-linux-musl/release/garden) ;;
    web)
      (cd web && bun install && bun run build)
      FILES+=(web/dist) ;;
    sensors) FILES+=(sensors) ;;
    units) ;; # the service files are in deploy/, which is always sent
    *) echo "unknown part: $part (use server, web, sensors, units)"; exit 1 ;;
  esac
done

echo "copying to $PI ..."
# .venv only works on the PC it was made on, so it never goes to the Pi.
tar -cf - --exclude=.venv --exclude=__pycache__ "${FILES[@]}" \
  | ssh "$PI" 'rm -rf upload && mkdir upload && tar -xf - -C upload'

ssh -t "$PI" sudo bash upload/deploy/install.sh "${PARTS[@]}"

echo "checking the app answers ..."
if ssh "$PI" 'sleep 3; curl -fsS localhost:8080/api/now'; then
  echo; echo "done"
else
  echo; echo "the app did not answer; look at the log: ssh $PI journalctl -u garden -n 50"; exit 1
fi
