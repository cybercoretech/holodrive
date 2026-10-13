#!/usr/bin/env bash
# Holodrive, user side: build, install ~/.local/bin/holodrive and the
# holodrive.service user unit, write a starter config (first run), set the
# password (first run), start it, and publish it to your tailnet.
#
#   ./packaging/install.sh            # port 8768
#   PORT=8769 ./packaging/install.sh  # another port (e.g. next to an old file server)
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $EUID -ne 0 ]] || { echo "Run this as your user, not root." >&2; exit 1; }
PORT="${PORT:-8768}"
CONF="${XDG_CONFIG_HOME:-$HOME/.config}/holodrive/config.toml"

cargo build --release --locked
target_dir="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import sys,json;print(json.load(sys.stdin)["target_directory"])')"
install -Dm755 "$target_dir/release/holodrive" "$HOME/.local/bin/holodrive"

if [[ ! -f "$CONF" ]]; then
  ts_name=""
  if command -v tailscale >/dev/null; then
    ts_name="$(tailscale status --json 2>/dev/null | python3 -c 'import sys,json;print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))' 2>/dev/null || true)"
  fi
  args=()
  [[ -n "$ts_name" ]] && args+=(--host "$ts_name:$PORT")
  "$HOME/.local/bin/holodrive" init "${args[@]}"
  sed -i "s/^listen = .*/listen = \"127.0.0.1:$PORT\"/" "$CONF"
fi
if ! grep -q '^password_hash = "\$argon2' "$CONF"; then
  "$HOME/.local/bin/holodrive" passwd
fi

install -Dm644 packaging/holodrive.service "$HOME/.config/systemd/user/holodrive.service"
systemctl --user daemon-reload
systemctl --user enable --now holodrive.service
systemctl --user restart holodrive.service

if command -v tailscale >/dev/null && [[ -n "${ts_name:-}" || -n "$(grep -o '\.ts\.net:[0-9]*' "$CONF" || true)" ]]; then
  tailscale serve --bg --http="$PORT" "http://127.0.0.1:$PORT" >/dev/null
  host="$(grep -o '[a-z0-9.-]*\.ts\.net' "$CONF" | head -1)"
  echo
  echo "Holodrive is up: http://$host:$PORT/  (tailnet only)"
else
  echo
  echo "Holodrive is up: http://127.0.0.1:$PORT/"
fi
echo "Config: $CONF  ·  logs: journalctl --user -u holodrive -f"
